use crate::bot::{copy, Bot};
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Maneja el comando `/start` enviando la bienvenida canónica.
pub async fn handle_start(bot: Bot, msg: Message) -> Result<(), anyhow::Error> {
    bot.send_message(msg.chat.id, copy::START_MESSAGE)
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

/// Maneja el comando `/ayuda` enviando el catálogo de ayuda.
pub async fn handle_ayuda(bot: Bot, msg: Message) -> Result<(), anyhow::Error> {
    bot.send_message(msg.chat.id, copy::HELP_MESSAGE)
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}
