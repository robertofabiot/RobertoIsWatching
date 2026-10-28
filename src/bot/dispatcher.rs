use crate::bot::commands::{AdminCommand, Command};
use crate::bot::handlers::admin::handle_admin;
use crate::bot::handlers::assignments::handle_pendientes;
use crate::bot::handlers::auth::{handle_login, handle_logout};
use crate::bot::handlers::courses::handle_cursos;
use crate::bot::handlers::grades::handle_notas;
use crate::bot::handlers::start::{handle_ayuda, handle_start};
use crate::bot::handlers::summary::handle_resumen;
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

    let student_handler = teloxide::filter_command::<Command, _>()
        .branch(dptree::case![Command::Start].endpoint(handle_start))
        .branch(dptree::case![Command::Ayuda].endpoint(handle_ayuda))
        .branch(dptree::case![Command::Login(args)].endpoint(handle_login))
        .branch(dptree::case![Command::Cursos].endpoint(handle_cursos))
        .branch(dptree::case![Command::Pendientes].endpoint(handle_pendientes))
        .branch(dptree::case![Command::Notas].endpoint(handle_notas))
        .branch(dptree::case![Command::Resumen].endpoint(handle_resumen))
        .branch(dptree::case![Command::Logout].endpoint(handle_logout));

    let admin_handler = teloxide::filter_command::<AdminCommand, _>().endpoint(handle_admin);

    let handler = Update::filter_message()
        .branch(student_handler)
        .branch(admin_handler);

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![pool, moodle, config, rate_limiter])
        .enable_ctrlc_handler()
        .build()
}
