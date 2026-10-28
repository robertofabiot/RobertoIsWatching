use crate::bot::Bot;
use crate::config::Config;
use crate::moodle::MoodleClient;
use sqlx::SqlitePool;
use std::sync::Arc;
use teloxide::dispatching::{Dispatcher, UpdateFilterExt};
use teloxide::dptree;
use teloxide::types::Update;

pub fn create_dispatcher(
    bot: Bot,
    pool: SqlitePool,
    moodle: MoodleClient,
    config: Arc<Config>,
) -> Dispatcher<Bot, anyhow::Error, teloxide::dispatching::DefaultKey> {
    let handler = Update::filter_message().endpoint(|_bot: Bot| async move {
        // Esqueleto básico de despacho
        Ok::<(), anyhow::Error>(())
    });

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![pool, moodle, config])
        .enable_ctrlc_handler()
        .build()
}
