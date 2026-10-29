use robertos_watching_bot::app;
use robertos_watching_bot::bot;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::moodle::{self, MoodleClient};
use robertos_watching_bot::{storage, telemetry};
use std::sync::Arc;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let config = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error crítico de configuración: {err}");
            std::process::exit(1);
        }
    };

    telemetry::init(&config.rust_log);

    let pool = match storage::init(&config).await {
        Ok(pool) => {
            tracing::info!("Capa de persistencia SQLite inicializada con éxito.");
            pool
        }
        Err(err) => {
            tracing::error!("Error crítico al inicializar la base de datos: {err}");
            std::process::exit(1);
        }
    };

    tracing::info!(
        admin_chat_id = config.admin_chat_id,
        tz = %config.bot_tz,
        max_users = config.max_users_default,
        database_url = %config.database_url,
        "Iniciando Roberto's Watching Bot..."
    );

    let bot = bot::create_bot(&config.bot_token);

    if let Err(err) = bot::register_commands(&bot).await {
        tracing::warn!(error = %err, "No se pudo registrar el menú de comandos en Telegram");
    }

    let moodle = MoodleClient::new(moodle::DEFAULT_BASE_URL);
    let config_arc = Arc::new(config);

    let app = app::build(pool, bot, moodle, config_arc);
    let mut dispatcher = app.dispatcher;
    // cableado en el siguiente commit
    let _ = app.scanner;
    let _ = app.deadline_worker;

    tracing::info!(
        "Dispatcher de Telegram iniciado. Escuchando actualizaciones vía Long Polling..."
    );
    dispatcher.dispatch().await;

    tracing::info!("Apagado limpio completado. Hasta pronto.");
}
