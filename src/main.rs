#![allow(dead_code, unused_imports)]

use robertos_watching_bot::config::Config;
use robertos_watching_bot::{bot, config, moodle, scheduler, services, storage, telemetry};

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

    let _pool = match storage::init(&config).await {
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
    tracing::debug!("Configuración inicial: {:?}", config);

    wait_for_shutdown().await;

    tracing::info!("Apagado limpio completado. Hasta pronto.");
}

#[cfg(unix)]
async fn wait_for_shutdown() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm =
        signal(SignalKind::terminate()).expect("Fallo al registrar manejador de señal SIGTERM");

    tokio::select! {
        res = tokio::signal::ctrl_c() => {
            if let Err(err) = res {
                tracing::error!("Error al escuchar señal SIGINT (Ctrl+C): {err}");
            } else {
                tracing::info!("Señal SIGINT recibida (Ctrl+C). Iniciando apagado ordenado...");
            }
        }
        _ = sigterm.recv() => {
            tracing::info!("Señal SIGTERM recibida (systemd). Iniciando apagado ordenado...");
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::error!("Error al escuchar señal de interrupción: {err}");
    } else {
        tracing::info!("Señal de interrupción recibida. Iniciando apagado ordenado...");
    }
}
