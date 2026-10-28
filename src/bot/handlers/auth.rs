use crate::bot::rate_limit::LoginRateLimiter;
use crate::bot::{copy, Bot};
use crate::moodle::{MoodleClient, MoodleError, Priority};
use crate::storage::{AdmissionResult, AlertsRepository, CoursesRepository, UsersRepository};
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use zeroize::Zeroizing;

/// Maneja el comando `/login <CIF> <CONTRASEÑA>` con borrado inmediato y seguridad estricta (A-11, A-12, RF-01).
pub async fn handle_login(
    bot: Bot,
    msg: Message,
    args: String,
    pool: SqlitePool,
    moodle: MoodleClient,
    rate_limiter: LoginRateLimiter,
) -> Result<(), anyhow::Error> {
    // 1. ELIMINACIÓN INMEDIATA DEL MENSAJE (Primera instrucción obligatoria por seguridad) (RF-01, A-11)
    let _ = bot.delete_message(msg.chat.id, msg.id).await;

    // 2. Restricción estricta a chats privados (A-11)
    if !msg.chat.is_private() {
        bot.send_message(msg.chat.id, copy::ONLY_PRIVATE_CHAT)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    let chat_id = msg.chat.id.0;

    // 3. Verificación de Rate Limit (A-11: máx 3 intentos / 10 min por chat)
    if rate_limiter.is_rate_limited(chat_id) {
        bot.send_message(msg.chat.id, copy::LOGIN_RATE_LIMITED)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    // 4. Parseo seguro de argumentos: cif = 1er token, pass = resto de la línea
    let raw = args.trim();
    let mut parts = raw.splitn(2, |c: char| c.is_whitespace());
    let cif = parts.next().unwrap_or("").trim();
    let pass_raw = parts.next().unwrap_or("").trim();

    if cif.is_empty() || pass_raw.is_empty() {
        bot.send_message(msg.chat.id, copy::LOGIN_USAGE)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    // Envolver inmediatamente la contraseña en un buffer Zeroizing (RF-01, A-09)
    let pass_zeroizing = Zeroizing::new(pass_raw.to_string());

    // Registro seguro en tracing (PROHIBIDO registrar la contraseña o el texto del mensaje)
    tracing::info!(chat_id = chat_id, cif = %cif, "Procesando solicitud de inicio de sesión");

    // 5. Autenticación en Moodle intercambiando credenciales por wstoken
    let login_info = match moodle.login(cif, pass_zeroizing).await {
        Ok(info) => {
            rate_limiter.clear(chat_id);
            info
        }
        Err(err) => {
            match err {
                MoodleError::InvalidLogin => {
                    rate_limiter.record_failure(chat_id);
                    bot.send_message(msg.chat.id, copy::LOGIN_INVALID_CREDENTIALS)
                        .parse_mode(ParseMode::Html)
                        .await?;
                }
                MoodleError::Maintenance => {
                    bot.send_message(msg.chat.id, copy::UAM_MAINTENANCE)
                        .parse_mode(ParseMode::Html)
                        .await?;
                }
                other => {
                    tracing::warn!(chat_id = chat_id, error = %other, "Error durante login en Moodle");
                    bot.send_message(
                        msg.chat.id,
                        format!("⚠️ Error de comunicación con Moodle: {other}"),
                    )
                    .await?;
                }
            }
            return Ok(());
        }
    };

    // 6. Intento de admisión atómica con control de cupos (A-12)
    let admission = UsersRepository::try_admit(
        &pool,
        chat_id,
        cif,
        login_info.userid,
        &login_info.fullname,
        &login_info.token,
    )
    .await?;

    match admission {
        AdmissionResult::Closed | AdmissionResult::Full => {
            tracing::info!(chat_id = chat_id, "Registro rechazado por cupo lleno o cerrado");
            bot.send_message(msg.chat.id, copy::WAITLIST_FULL)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        AdmissionResult::Admitted(user) | AdmissionResult::AlreadyMember(user) => {
            // 7. Sincronizar y registrar los cursos activos del alumno
            let courses = moodle
                .user_courses(&user.wstoken, user.uam_user_id, Priority::Interactive)
                .await
                .unwrap_or_default();

            for c in &courses {
                let _ =
                    CoursesRepository::upsert_course(&pool, c.id, &c.fullname, c.shortname.as_deref())
                        .await;
                let _ = CoursesRepository::link_user(&pool, user.chat_id, c.id).await;
            }

            tracing::info!(
                chat_id = chat_id,
                cif = %cif,
                courses = courses.len(),
                "Usuario admitido y cursos vinculados exitosamente"
            );

            let welcome_msg = format!(
                "{}<b>{}</b>.\n\n👁️ Se han vinculado <b>{} cursos activos</b> a tu radar.\n\
                 Escribe <code>/pendientes</code> para ver tus tareas o <code>/cursos</code> para ver tus asignaturas.",
                copy::LOGIN_SUCCESS_PREFIX,
                user.fullname,
                courses.len()
            );

            bot.send_message(msg.chat.id, welcome_msg)
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}

/// Maneja el comando `/logout` eliminando al usuario y purgando sus alertas pendientes (A-14).
pub async fn handle_logout(
    bot: Bot,
    msg: Message,
    pool: SqlitePool,
) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let deleted = UsersRepository::delete(&pool, chat_id).await?;
    let _ = AlertsRepository::purge_chat(&pool, chat_id).await;

    if deleted {
        tracing::info!(chat_id = chat_id, "Sesión de usuario purgada exitosamente (/logout)");
        bot.send_message(msg.chat.id, copy::LOGOUT_SUCCESS)
            .parse_mode(ParseMode::Html)
            .await?;
    } else {
        bot.send_message(msg.chat.id, copy::LOGOUT_NOT_LOGGED_IN)
            .parse_mode(ParseMode::Html)
            .await?;
    }

    Ok(())
}
