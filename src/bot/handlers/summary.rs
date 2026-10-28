use crate::bot::render::{render_daily_summary, send_chunked_html};
use crate::bot::Bot;
use crate::config::Config;
use crate::storage::UsersRepository;
use sqlx::SqlitePool;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Maneja el comando `/resumen` enviando el digest del día del alumno (RF-08, PR #5, PR #8).
pub async fn handle_resumen(
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
             Escribe <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code> para activar tus resúmenes.",
        )
        .parse_mode(ParseMode::Html)
        .await?;
        return Ok(());
    }

    let (header, items) = render_daily_summary(&pool, chat_id, config.bot_tz).await?;

    if items.is_empty() {
        bot.send_message(msg.chat.id, header)
            .parse_mode(ParseMode::Html)
            .await?;
    } else {
        send_chunked_html(&bot, msg.chat.id, &header, &items).await?;
    }

    Ok(())
}
