use crate::bot::commands::AdminCommand;
use crate::bot::Bot;
use crate::config::Config;
use crate::storage::{CoursesRepository, SettingsRepository, UsersRepository};
use sqlx::SqlitePool;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Maneja comandos exclusivos del administrador (`ADMIN_CHAT_ID`) (RF-10).
/// Si un chat no autorizado intenta invocarlos, se ignora en silencio sin revelar su existencia.
pub async fn handle_admin(
    bot: Bot,
    msg: Message,
    cmd: AdminCommand,
    pool: SqlitePool,
    config: Arc<Config>,
) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    if chat_id != config.admin_chat_id {
        tracing::warn!(
            chat_id = chat_id,
            "Intento no autorizado de ejecutar comando de administración ignorado silenciosamente"
        );
        return Ok(());
    }

    match cmd {
        AdminCommand::AdminStats => {
            let total_users = UsersRepository::count(&pool).await?;
            let active_users = UsersRepository::count_active(&pool).await?;
            let expired_users = UsersRepository::count_expired(&pool).await?;
            let unique_courses = CoursesRepository::unique_active_courses(&pool).await?.len();
            let max_users = SettingsRepository::max_users(&pool).await?;
            let reg_open = SettingsRepository::registration_open(&pool).await?;

            let reg_str = if reg_open {
                "🟢 Abierto"
            } else {
                "🔴 Cerrado"
            };

            let text = format!(
                "📊 <b>Estado del Sistema — Roberto's Watching</b>\n\n\
                 👥 <b>Usuarios totales:</b> {}\n\
                 ✅ <b>Usuarios activos:</b> {}\n\
                 ⚠️ <b>Tokens expirados:</b> {}\n\
                 📚 <b>Cursos activos:</b> {}\n\
                 🪑 <b>Cupos:</b> {} / {}\n\
                 🚪 <b>Registro:</b> {}",
                total_users,
                active_users,
                expired_users,
                unique_courses,
                total_users,
                max_users,
                reg_str
            );

            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        AdminCommand::ToggleRegistro => {
            let next_state = SettingsRepository::toggle_registration_open(&pool).await?;
            let state_str = if next_state {
                "abierto 🟢"
            } else {
                "cerrado 🔴"
            };
            let text = format!(
                "🚪 El registro general de alumnos ahora está <b>{}</b>.",
                state_str
            );

            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        AdminCommand::SetMaxUsers(n) => {
            if n == 0 {
                bot.send_message(msg.chat.id, "⚠️ El límite de cupos debe ser mayor a 0.")
                    .parse_mode(ParseMode::Html)
                    .await?;
                return Ok(());
            }

            SettingsRepository::set_max_users(&pool, n as i64).await?;
            let text = format!("🪑 Límite de cupos actualizado a <b>{}</b> estudiantes.", n);

            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}
