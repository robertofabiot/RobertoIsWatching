use crate::bot::{copy, Bot};
use crate::moodle::gate::Priority;
use crate::moodle::MoodleClient;
use crate::services::{session, sync};
use crate::storage::{CoursesRepository, UsersRepository};
use chrono::Utc;
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Cooldown mínimo de 10 minutos entre escaneos interactivos a Moodle (RF-08).
pub const SYNC_COOLDOWN_SECONDS: i64 = 600;

/// Maneja el comando `/sync` forzando sincronización con Moodle respetando el cooldown anti-ansiedad (RF-08).
pub async fn handle_sync(
    bot: Bot,
    msg: Message,
    pool: SqlitePool,
    moodle: MoodleClient,
) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let user = UsersRepository::get(&pool, chat_id).await?;

    if user.as_ref().map(|u| !u.is_active).unwrap_or(true) {
        bot.send_message(
            msg.chat.id,
            "ℹ️ No tienes ninguna sesión activa en Roberto's Watching.\n\
             Escribe <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code> para activar la sincronización.",
        )
        .parse_mode(ParseMode::Html)
        .await?;
        return Ok(());
    }

    // 1. Comprobar cooldown anti-ansiedad de 10 minutos desde SQLite (A-08, RF-08)
    let latest_scan = CoursesRepository::latest_scan_for_user(&pool, chat_id).await?;

    if let Some(last_dt) = latest_scan {
        let now_utc = Utc::now().naive_utc();
        let elapsed = now_utc.signed_duration_since(last_dt);

        if elapsed.num_seconds() >= 0 && elapsed.num_seconds() < SYNC_COOLDOWN_SECONDS {
            let minutes_ago = elapsed.num_minutes().max(1);
            let cooldown_msg = copy::sync_cooldown_message(minutes_ago);
            bot.send_message(msg.chat.id, cooldown_msg)
                .parse_mode(ParseMode::Html)
                .await?;
            return Ok(());
        }
    }

    // 2. Cooldown superado o primer escaneo: Enviar mensaje de búsqueda
    bot.send_message(msg.chat.id, copy::SYNC_SCANNING)
        .parse_mode(ParseMode::Html)
        .await?;

    // 3. Ejecutar pipeline de refresco interactivo
    match sync::refresh_user(&pool, &moodle, chat_id, Priority::Interactive).await {
        Ok(summary) => {
            if !summary.has_changes() {
                bot.send_message(msg.chat.id, copy::SYNC_NO_CHANGES)
                    .parse_mode(ParseMode::Html)
                    .await?;
            } else {
                let report = format!(
                    "✅ <b>Sincronización completada:</b>\n\
                     • <b>{}</b> tareas nuevas detectadas\n\
                     • <b>{}</b> plazos actualizados\n\
                     • <b>{}</b> calificaciones nuevas registradas\n\n\
                     Escribe <code>/pendientes</code> para ver tus entregas.",
                    summary.new_assignments, summary.updated_assignments, summary.new_grades
                );
                bot.send_message(msg.chat.id, report)
                    .parse_mode(ParseMode::Html)
                    .await?;
            }
        }
        Err(err) => {
            tracing::warn!(chat_id = chat_id, error = %err, "Error al sincronizar con Moodle vía /sync");

            if let Some(moodle_err) = err.downcast_ref::<crate::moodle::error::MoodleError>() {
                if moodle_err.is_token_error() {
                    let _ = session::expire_user(&pool, &bot, chat_id).await;
                    return Ok(());
                }
            }

            bot.send_message(msg.chat.id, copy::UAM_MAINTENANCE)
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}
