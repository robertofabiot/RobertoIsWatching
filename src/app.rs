//! Ensamblado y configuración de componentes centrales de la aplicación (PR-A, Prompt A1 & A2).
//! Centraliza la construcción del dispatcher del bot, el scanner y el worker de plazos
//! para facilitar la inicialización y el lanzamiento de workers en segundo plano.

use crate::bot::dispatcher::create_dispatcher;
use crate::bot::Bot;
use crate::config::Config;
use crate::moodle::MoodleClient;
use crate::scheduler::deadlines::{DeadlineScheduler, DeadlineWorker};
use crate::services::scanner::ScannerWorker;
use sqlx::SqlitePool;
use std::sync::Arc;
use teloxide::dispatching::Dispatcher;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Alias para el Dispatcher configurado del bot de Telegram.
pub type BotDispatcher = Dispatcher<Bot, anyhow::Error, teloxide::dispatching::DefaultKey>;

/// Estructura contenedora de los componentes principales ensamblados.
pub struct App {
    pub dispatcher: BotDispatcher,
    pub scanner: ScannerWorker,
    pub deadline_worker: DeadlineWorker,
}

/// Estructura contenedora de los handles de los workers en ejecución y el canal de apagado.
pub struct Running {
    pub shutdown_tx: watch::Sender<bool>,
    pub scanner_handle: JoinHandle<()>,
    pub deadline_handle: JoinHandle<()>,
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

/// Lanza los workers de fondo (scanner y planificador de plazos) en tareas de Tokio
/// asociadas a un canal de apagado coordinado.
pub fn spawn_workers(scanner: ScannerWorker, deadline_worker: DeadlineWorker) -> Running {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let scanner_handle = tokio::spawn(scanner.run_loop(shutdown_rx.clone()));
    let deadline_handle = tokio::spawn(deadline_worker.run_loop(shutdown_rx.clone()));

    tracing::info!("Workers de fondo lanzados: scanner y planificador de plazos");

    Running {
        shutdown_tx,
        scanner_handle,
        deadline_handle,
    }
}

impl App {
    /// Lanza los workers de fondo de la aplicación y retorna el dispatcher junto con los handles de ejecución.
    pub fn spawn_workers(self) -> (BotDispatcher, Running) {
        let running = spawn_workers(self.scanner, self.deadline_worker);
        (self.dispatcher, running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    #[tokio::test]
    async fn test_spawn_workers_and_shutdown() {
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
        let running = spawn_workers(app.scanner, app.deadline_worker);

        // Enviar señal de apagado a los workers
        running.shutdown_tx.send(true).unwrap();

        // Asegurar que ambos workers finalicen de forma limpia dentro del timeout
        let scanner_res =
            tokio::time::timeout(Duration::from_millis(500), running.scanner_handle).await;
        assert!(scanner_res.is_ok(), "scanner_handle no terminó a tiempo");

        let deadline_res =
            tokio::time::timeout(Duration::from_millis(500), running.deadline_handle).await;
        assert!(deadline_res.is_ok(), "deadline_handle no terminó a tiempo");
    }
}
