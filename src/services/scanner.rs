//! Motor de escaneo compartido en segundo plano (PR #6, Plan Maestro §4).
//! Ejecuta un worker loop en Tokio con periodicidad configurable y parada limpia.

use crate::bot::Bot;
use crate::config::Config;
use crate::moodle::MoodleClient;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

pub const DEFAULT_INITIAL_DELAY_SECS: u64 = 5;

/// Resumen de estadísticas de ejecución de un ciclo de escaneo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanCycleSummary {
    pub courses_scanned: usize,
    pub courses_failed: usize,
    pub new_assignments: usize,
    pub new_materials: usize,
    pub submissions_checked: usize,
    pub alerts_dispatched: usize,
}

/// Worker asíncrono que ejecuta periódicamente el escaneo compartido (RF-02, RF-08).
pub struct ScannerWorker {
    #[allow(dead_code)]
    pub(crate) pool: SqlitePool,
    #[allow(dead_code)]
    pub(crate) moodle: MoodleClient,
    #[allow(dead_code)]
    pub(crate) bot: Bot,
    #[allow(dead_code)]
    pub(crate) config: Arc<Config>,
    pub(crate) interval: Duration,
    pub(crate) initial_delay: Duration,
}

impl ScannerWorker {
    pub fn new(pool: SqlitePool, moodle: MoodleClient, bot: Bot, config: Arc<Config>) -> Self {
        let interval = config.scan_interval();
        Self {
            pool,
            moodle,
            bot,
            config,
            interval,
            initial_delay: Duration::from_secs(DEFAULT_INITIAL_DELAY_SECS),
        }
    }

    /// Permite personalizar el intervalo y el retardo inicial (esencial para tests).
    pub fn with_delays(mut self, interval: Duration, initial_delay: Duration) -> Self {
        self.interval = interval;
        self.initial_delay = initial_delay;
        self
    }

    /// Ejecuta un único ciclo de escaneo compartido.
    pub async fn run_scan_cycle(&self) -> Result<ScanCycleSummary, anyhow::Error> {
        tracing::info!("Iniciando ciclo de escaneo compartido de Moodle...");
        let summary = ScanCycleSummary::default();
        tracing::info!(
            courses = summary.courses_scanned,
            alerts = summary.alerts_dispatched,
            "Ciclo de escaneo compartido finalizado."
        );
        Ok(summary)
    }

    /// Inicia el bucle periódico de escaneo cancelable mediante `shutdown_rx`.
    pub async fn run_loop(self, mut shutdown_rx: watch::Receiver<bool>) {
        tracing::info!(
            interval_mins = self.interval.as_secs() / 60,
            initial_delay_secs = self.initial_delay.as_secs(),
            "ScannerWorker loop iniciado."
        );

        // 1. Retardo corto inicial antes del primer escaneo
        tokio::select! {
            _ = tokio::time::sleep(self.initial_delay) => {}
            _ = shutdown_rx.changed() => {
                tracing::info!("ScannerWorker apagado durante retardo inicial.");
                return;
            }
        }

        // Primer ciclo tras el retardo inicial
        if let Err(err) = self.run_scan_cycle().await {
            tracing::error!(error = %err, "Error en ciclo inicial de escaneo.");
        }

        // Bucle continuo cada `interval`
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    tracing::info!("ScannerWorker recibió señal de apagado. Deteniendo worker...");
                    break;
                }
                _ = tokio::time::sleep(self.interval) => {
                    if let Err(err) = self.run_scan_cycle().await {
                        tracing::error!(error = %err, "Error durante ciclo periódico de escaneo.");
                    }
                }
            }
        }

        tracing::info!("ScannerWorker detenido limpiamente.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scanner_worker_shutdown() {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

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
        let moodle = MoodleClient::new("https://mock.uam.ni");

        let worker = ScannerWorker::new(pool, moodle, bot, config)
            .with_delays(Duration::from_millis(50), Duration::from_millis(10));

        let handle = tokio::spawn(async move {
            worker.run_loop(shutdown_rx).await;
        });

        // Esperar a que pase el retardo inicial y corra el primer ciclo
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Enviar señal de apagado
        shutdown_tx.send(true).unwrap();

        // El worker debe terminar prontamente
        let res = tokio::time::timeout(Duration::from_millis(500), handle).await;
        assert!(res.is_ok(), "Worker no respondió a shutdown a tiempo");
    }
}
