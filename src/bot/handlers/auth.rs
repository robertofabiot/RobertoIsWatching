use crate::bot::rate_limit::LoginRateLimiter;
use crate::bot::{copy, Bot};
use crate::moodle::{MoodleClient, MoodleError, Priority};
use crate::scheduler::deadlines::DeadlineScheduler;
use crate::storage::{AdmissionResult, AlertsRepository, UsersRepository};
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

    // 3. Verificación de Rate Limit inicial por chat_id (A-11)
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

    // Comprobación y reserva atómica de intento por chat_id y por cif (R-12)
    if rate_limiter.check_and_record(chat_id, cif).is_err() {
        bot.send_message(msg.chat.id, copy::LOGIN_RATE_LIMITED)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    // Envolver la contraseña en un buffer Zeroizing (RF-01, A-09).
    // Nota (R-15): El borrado de memoria es best-effort ya que copias en texto claro
    // pueden existir transitoriamente en `args: String` o en la estructura `Message` de teloxide.
    let pass_zeroizing = Zeroizing::new(pass_raw.to_string());

    // Registro seguro en tracing (PROHIBIDO registrar la contraseña o el texto del mensaje)
    tracing::info!(chat_id = chat_id, cif = %cif, "Procesando solicitud de inicio de sesión");

    // 5. Autenticación en Moodle intercambiando credenciales por wstoken
    let login_info = match moodle.login(cif, pass_zeroizing).await {
        Ok(info) => {
            rate_limiter.clear_both(chat_id, cif);
            info
        }
        Err(err) => {
            match err {
                MoodleError::InvalidLogin => {
                    // El intento fallido ya fue registrado de forma atómica en check_and_record
                    bot.send_message(msg.chat.id, copy::LOGIN_INVALID_CREDENTIALS)
                        .parse_mode(ParseMode::Html)
                        .await?;
                }
                MoodleError::Maintenance => {
                    rate_limiter.rollback(chat_id, cif);
                    bot.send_message(msg.chat.id, copy::UAM_MAINTENANCE)
                        .parse_mode(ParseMode::Html)
                        .await?;
                }
                other => {
                    rate_limiter.rollback(chat_id, cif);
                    tracing::warn!(chat_id = chat_id, error = %other, "Error interno durante login en Moodle");
                    bot.send_message(msg.chat.id, copy::MOODLE_COMMUNICATION_ERROR)
                        .parse_mode(ParseMode::Html)
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
            tracing::info!(
                chat_id = chat_id,
                "Registro rechazado por cupo lleno o cerrado"
            );
            bot.send_message(msg.chat.id, copy::WAITLIST_FULL)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        AdmissionResult::Admitted(user) | AdmissionResult::AlreadyMember(user) => {
            // 7. Sincronizar y sembrar en silencio cursos, tareas y entregas del alumno (A-03, A-04)
            let refresh_summary = match crate::services::sync::refresh_user(
                &pool,
                &moodle,
                user.chat_id,
                Priority::Interactive,
            )
            .await
            {
                Ok(summary) => summary,
                Err(err) => {
                    tracing::warn!(
                        chat_id = chat_id,
                        error = %err,
                        "Advertencia durante siembra silenciosa en login de usuario"
                    );
                    crate::services::sync::RefreshSummary::default()
                }
            };

            tracing::info!(
                chat_id = chat_id,
                cif = %cif,
                courses = refresh_summary.courses_count,
                assignments = refresh_summary.assignments_count,
                "Usuario admitido y sembrado exitosamente en silencio"
            );

            let welcome_msg = format!(
                "{}<b>{}</b>.\n\n👁️ Se han vinculado <b>{} cursos activos</b> a tu radar.\n\
                 Escribe <code>/pendientes</code> para ver tus tareas o <code>/cursos</code> para ver tus asignaturas.",
                copy::LOGIN_SUCCESS_PREFIX,
                user.fullname,
                refresh_summary.courses_count
            );

            bot.send_message(msg.chat.id, welcome_msg)
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}

static GLOBAL_SCHEDULER: std::sync::RwLock<Option<DeadlineScheduler>> =
    std::sync::RwLock::new(None);

/// Registra la instancia activa de DeadlineScheduler para uso del bot en callbacks (R-15).
pub fn set_global_scheduler(sched: DeadlineScheduler) {
    if let Ok(mut lock) = GLOBAL_SCHEDULER.write() {
        *lock = Some(sched);
    }
}

/// Obtiene la instancia activa de DeadlineScheduler si fue registrada.
pub fn get_global_scheduler() -> Option<DeadlineScheduler> {
    GLOBAL_SCHEDULER.read().ok().and_then(|lock| lock.clone())
}

/// Realiza el proceso unificado de logout: elimina el usuario en BD, purga alertas en sent_alerts
/// y cancela sus jobs activos en el planificador de plazos (A-14, A-16, R-15).
pub async fn execute_logout(
    pool: &SqlitePool,
    scheduler: Option<&DeadlineScheduler>,
    chat_id: i64,
) -> Result<bool, anyhow::Error> {
    let deleted = UsersRepository::delete(pool, chat_id).await?;
    let _ = AlertsRepository::purge_chat(pool, chat_id).await;

    let sched_opt = scheduler.cloned().or_else(get_global_scheduler);
    if let Some(ref sched) = sched_opt {
        let _ = sched.cancel_for_user(chat_id).await;
    }

    Ok(deleted)
}

/// Maneja el comando `/logout` eliminando al usuario, purgando sus alertas y cancelando sus jobs en el scheduler (A-14, A-16, R-15).
pub async fn handle_logout(
    bot: Bot,
    msg: Message,
    pool: SqlitePool,
    scheduler: Option<DeadlineScheduler>,
) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let deleted = execute_logout(&pool, scheduler.as_ref(), chat_id).await?;

    if deleted {
        tracing::info!(
            chat_id = chat_id,
            "Sesión de usuario purgada exitosamente (/logout)"
        );
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
