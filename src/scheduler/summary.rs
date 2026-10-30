//! Worker en segundo plano para el despacho de resúmenes diarios (PR #8, A-18).
//! Despierta alineado al límite de cada minuto (:00.000) en la zona horaria `BOT_TZ`.

use crate::bot::Bot;
use crate::config::Config;
use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Calcula la duración restante hasta el siguiente límite de minuto (:00.000)
/// sumando un margen de 20ms para garantizar que el reloj ha cruzado el minuto.
pub fn duration_until_next_minute(now: DateTime<Utc>) -> Duration {
    let seconds = now.second();
    let millis = now.timestamp_subsec_millis();
    let remaining_millis = if seconds >= 60 {
        1000u64.saturating_sub(millis as u64)
    } else {
        (60 - seconds as u64) * 1000 - (millis as u64)
    };
    Duration::from_millis(remaining_millis + 20)
}

/// Worker responsable de orquestar y enviar los resúmenes diarios programados.
pub struct SummaryWorker {
    pool: SqlitePool,
    bot: Bot,
    config: Arc<Config>,
}

impl SummaryWorker {
    /// Construye una nueva instancia de `SummaryWorker`.
    pub fn new(pool: SqlitePool, bot: Bot, config: Arc<Config>) -> Self {
        Self { pool, bot, config }
    }

    /// Referencia al pool de conexiones de la base de datos.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Referencia al cliente del bot de Telegram.
    pub fn bot(&self) -> &Bot {
        &self.bot
    }

    /// Zona horaria configurada para el bot (`BOT_TZ`).
    pub fn tz(&self) -> Tz {
        self.config.bot_tz
    }

    /// Bucle principal del worker que ejecuta ticks alineados a cada minuto.
    pub async fn run_loop(self, mut shutdown_rx: watch::Receiver<bool>) {
        tracing::info!(
            tz = %self.config.bot_tz,
            "SummaryWorker iniciado con ticks alineados por minuto."
        );

        loop {
            let now = Utc::now();
            let sleep_duration = duration_until_next_minute(now);

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    tracing::info!("SummaryWorker apagado por señal de shutdown.");
                    break;
                }
                _ = tokio::time::sleep(sleep_duration) => {
                    let tick_time = Utc::now().with_timezone(&self.config.bot_tz);
                    tracing::debug!(tick = %tick_time, "SummaryWorker tick de minuto alcanzado");
                    self.evaluate_minute(&tick_time).await;
                }
            }
        }

        tracing::info!("SummaryWorker finalizado limpiamente.");
    }

    /// Evalúa el tick de un minuto específico.
    pub async fn evaluate_minute(&self, dt: &DateTime<Tz>) {
        let time_hhmm = dt.format("%H:%M").to_string();
        let weekday_iso = dt.weekday().number_from_monday();
        let date_ymd = dt.format("%Y-%m-%d").to_string();
        tracing::debug!(
            time = %time_hhmm,
            weekday = weekday_iso,
            date = %date_ymd,
            "Evaluando resúmenes para el minuto"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_duration_until_next_minute_mid_minute() {
        let dt = Utc.with_ymd_and_hms(2026, 10, 30, 12, 30, 15).unwrap();
        let duration = duration_until_next_minute(dt);
        // (60 - 15) * 1000 + 20 = 45020 ms
        assert_eq!(duration.as_millis(), 45020);
    }

    #[test]
    fn test_duration_until_next_minute_near_boundary() {
        let dt = Utc
            .with_ymd_and_hms(2026, 10, 30, 12, 30, 59)
            .unwrap()
            .checked_add_signed(chrono::Duration::milliseconds(950))
            .unwrap();
        let duration = duration_until_next_minute(dt);
        // (60 - 59) * 1000 - 950 + 20 = 50 + 20 = 70 ms
        assert_eq!(duration.as_millis(), 70);
    }

    #[test]
    fn test_timezone_conversion_to_managua_utc_minus_6() {
        let tz: Tz = "America/Managua".parse().unwrap();
        // 13:30 UTC -> 07:30 Managua
        let utc = Utc.with_ymd_and_hms(2026, 10, 30, 13, 30, 0).unwrap();
        let managua = utc.with_timezone(&tz);

        assert_eq!(managua.format("%H:%M").to_string(), "07:30");
        assert_eq!(managua.format("%Y-%m-%d").to_string(), "2026-10-30");
        // 2026-10-30 is Friday (5)
        assert_eq!(managua.weekday().number_from_monday(), 5);
    }
}
