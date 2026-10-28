use crate::bot::commands::Command;
use crate::bot::handlers::auth::{handle_login, handle_logout};
use crate::bot::handlers::start::{handle_ayuda, handle_start};
use crate::bot::rate_limit::LoginRateLimiter;
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
    let rate_limiter = LoginRateLimiter::new();

    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(dptree::case![Command::Start].endpoint(handle_start))
        .branch(dptree::case![Command::Ayuda].endpoint(handle_ayuda))
        .branch(dptree::case![Command::Login(args)].endpoint(handle_login))
        .branch(dptree::case![Command::Logout].endpoint(handle_logout));

    let handler = Update::filter_message().branch(command_handler);

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![pool, moodle, config, rate_limiter])
        .enable_ctrlc_handler()
        .build()
}
