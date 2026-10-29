//! Ensamblado y configuración de componentes centrales de la aplicación (PR-A).
//! Centraliza la construcción del dispatcher del bot, el scanner y el worker de plazos,
//! así como el lanzamiento y apagado ordenado de workers en segundo plano.

use crate::bot::dispatcher::create_dispatcher;
use crate::bot::Bot;
use crate::config::Config;
use crate::moodle::MoodleClient;
use crate::scheduler::deadlines::{DeadlineScheduler, DeadlineWorker};
use crate::services::scanner::ScannerWorker;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
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

impl Running {
    /// Envía la señal de apagado a los workers y espera a que ambos JoinHandles terminen
    /// con un tiempo límite independiente para cada uno.
    /// Retorna `true` si ambos workers cerraron limpiamente sin timeout ni errores de pánico.
    pub async fn shutdown_and_wait(self, timeout_per_worker: Duration) -> bool {
        let _ = self.shutdown_tx.send(true);
        let mut clean = true;

        match tokio::time::timeout(timeout_per_worker, self.scanner_handle).await {
            Ok(Ok(())) => {
                tracing::info!("ScannerWorker finalizado exitosamente.");
            }
            Ok(Err(panic_err)) => {
                clean = false;
                tracing::error!(error = %panic_err, "ScannerWorker terminó con error o pánico");
            }
            Err(_) => {
                clean = false;
                tracing::warn!(
                    timeout_secs = timeout_per_worker.as_secs(),
                    "Timeout esperando que ScannerWorker se detenga"
                );
            }
        }

        match tokio::time::timeout(timeout_per_worker, self.deadline_handle).await {
            Ok(Ok(())) => {
                tracing::info!("DeadlineWorker finalizado exitosamente.");
            }
            Ok(Err(panic_err)) => {
                clean = false;
                tracing::error!(error = %panic_err, "DeadlineWorker terminó con error o pánico");
            }
            Err(_) => {
                clean = false;
                tracing::warn!(
                    timeout_secs = timeout_per_worker.as_secs(),
                    "Timeout esperando que DeadlineWorker se detenga"
                );
            }
        }

        clean
    }
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

/// Configura una tarea en segundo plano que escucha la señal SIGTERM (UNIX)
/// y solicita el apagado ordenado del dispatcher de Telegram mediante su ShutdownToken.
#[allow(unused_variables)]
pub fn setup_sigterm_handler(dispatcher: &BotDispatcher) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let shutdown_token = dispatcher.shutdown_token();
        tokio::spawn(async move {
            match signal(SignalKind::terminate()) {
                Ok(mut sigterm) => {
                    sigterm.recv().await;
                    tracing::info!("Señal SIGTERM recibida. Solicitando apagado del dispatcher...");
                    let _ = shutdown_token.shutdown();
                }
                Err(err) => {
                    tracing::error!(error = %err, "Error al registrar el manejador de señal SIGTERM");
                }
            }
        });
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

        // Apagar y esperar a que ambos workers finalicen de forma limpia
        let clean = running.shutdown_and_wait(Duration::from_millis(500)).await;
        assert!(clean, "El apagado de workers debió ser limpio");
    }

    #[tokio::test]
    async fn test_setup_sigterm_handler_does_not_panic() {
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
        setup_sigterm_handler(&app.dispatcher);
    }

    #[tokio::test]
    async fn test_shutdown_and_wait_timeout() {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        // Simular worker que tarda más del timeout
        let scanner_handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
        });
        let deadline_handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
        });

        let running = Running {
            shutdown_tx,
            scanner_handle,
            deadline_handle,
        };

        let clean = running.shutdown_and_wait(Duration::from_millis(50)).await;
        assert!(!clean, "El apagado con timeout debió retornar false");
    }

    #[tokio::test]
    async fn test_dispatcher_and_workers_graceful_shutdown() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let telegram_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*getme"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "id": 123456789,
                    "is_bot": true,
                    "first_name": "TestBot",
                    "username": "test_bot",
                    "can_join_groups": true,
                    "can_read_all_group_messages": false,
                    "supports_inline_queries": false,
                    "has_main_web_app": false
                }
            })))
            .mount(&telegram_server)
            .await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*getwebhookinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "url": "",
                    "has_custom_certificate": false,
                    "pending_update_count": 0
                }
            })))
            .mount(&telegram_server)
            .await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*deletewebhook"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": true
            })))
            .mount(&telegram_server)
            .await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*getupdates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": []
            })))
            .mount(&telegram_server)
            .await;

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
        let bot = crate::bot::create_bot_with_url(
            &config.bot_token,
            reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
        );
        let moodle = MoodleClient::new("http://127.0.0.1:8080");

        let app = build(pool, bot, moodle, config);
        let mut dispatcher = app.dispatcher;
        let running = spawn_workers(app.scanner, app.deadline_worker);

        let shutdown_token = dispatcher.shutdown_token();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = shutdown_token.shutdown();
        });

        dispatcher.dispatch().await;

        let clean = running.shutdown_and_wait(Duration::from_secs(1)).await;
        assert!(clean, "El apagado coordinado debió ser limpio");
    }
}
