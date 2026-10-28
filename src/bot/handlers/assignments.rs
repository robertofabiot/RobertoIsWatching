use crate::bot::render::send_chunked_html;
use crate::bot::{copy, Bot};
use crate::config::Config;
use crate::storage::{AssignmentsRepository, UsersRepository};
use sqlx::SqlitePool;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Maneja el comando `/pendientes` mostrando las entregas no vencidas y pendientes del alumno (RF-08).
pub async fn handle_pendientes(
    bot: Bot,
    msg: Message,
    pool: SqlitePool,
    config: Arc<Config>,
) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let user = UsersRepository::get(&pool, chat_id).await?;

    if user.as_ref().map(|u| !u.is_active).unwrap_or(true) {
        bot.send_message(
            msg.chat.id,
            "ℹ️ No tienes ninguna sesión activa en Roberto's Watching.\n\
             Escribe <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code> para ver tus entregas pendientes.",
        )
        .parse_mode(ParseMode::Html)
        .await?;
        return Ok(());
    }

    let pending = AssignmentsRepository::pending_detailed_for_user(&pool, chat_id, None).await?;

    if pending.is_empty() {
        bot.send_message(msg.chat.id, copy::PENDIENTES_ALL_CLEAR)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    let header = copy::pendientes_header(pending.len());
    let items: Vec<String> = pending
        .into_iter()
        .map(|item| {
            let due_text = match item.effective_duedate() {
                Some(epoch) if epoch > 0 => {
                    let formatted = copy::format_epoch_local(epoch, config.bot_tz);
                    if item.is_extended() {
                        format!("{} <i>(extensión de plazo)</i>", formatted)
                    } else {
                        formatted
                    }
                }
                _ => "Sin fecha límite asignada".to_string(),
            };

            format!(
                "📝 <b>{}</b>\n   📚 <i>{}</i>\n   ⏰ Vence: <b>{}</b>",
                copy::escape_html(&item.name),
                copy::escape_html(&item.course_name),
                due_text
            )
        })
        .collect();

    send_chunked_html(&bot, msg.chat.id, &header, &items).await?;
    Ok(())
}
