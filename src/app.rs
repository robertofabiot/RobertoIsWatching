//! Ensamblado y configuración de componentes centrales de la aplicación (PR-A, Prompt A1).
//! Centraliza la construcción del dispatcher del bot, el scanner y el worker de plazos
//! para facilitar la inicialización y pruebas sin ejecutar tareas en segundo plano.

use crate::bot::dispatcher::create_dispatcher;
use crate::bot::Bot;
use crate::config::Config;
use crate::moodle::MoodleClient;
use crate::scheduler::deadlines::{DeadlineScheduler, DeadlineWorker};
use crate::services::scanner::ScannerWorker;
use sqlx::SqlitePool;
use std::sync::Arc;
use teloxide::dispatching::Dispatcher;

/// Alias para el Dispatcher configurado del bot de Telegram.
pub type BotDispatcher = Dispatcher<Bot, anyhow::Error, teloxide::dispatching::DefaultKey>;

/// Estructura contenedora de los componentes principales ensamblados.
pub struct App {
    pub dispatcher: BotDispatcher,
    pub scanner: ScannerWorker,
    pub deadline_worker: DeadlineWorker,
}

/// Ensambla los componentes de la aplicación sin lanzar ninguna tarea en segundo plano.
pub fn build(pool: SqlitePool, bot: Bot, moodle: MoodleClient, config: Arc<Config>) -> App {
    let (scheduler, deadline_worker) =
        DeadlineScheduler::new(pool.clone(), bot.clone(), Arc::clone(&config));

    let scanner = ScannerWorker::new(
        pool.clone(),
        moodle.clone(),
        bot.clone(),
        Arc::clone(&config),
    )
    .with_scheduler(scheduler.clone());

    let dispatcher = create_dispatcher(bot, pool, moodle, config, Some(scheduler));

    App {
        dispatcher,
        scanner,
        deadline_worker,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_app_build_assembles_components() {
        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 1125347763,
            database_url: "sqlite::memory:".to_string(),
            max_users_default: 300,
            bot_tz: chrono_tz::America::Managua,
            rust_log: "info".to_string(),
            scan_interval_min: 30,
        });

        let pool = crate::storage::init(&config).await.unwrap();
        let bot = crate::bot::create_bot(&config.bot_token);
        let moodle = MoodleClient::new("http://127.0.0.1:8080");

        let app = build(pool, bot, moodle, config);

        assert_eq!(app.deadline_worker.len(), 0);
    }
}
