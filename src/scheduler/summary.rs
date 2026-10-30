//! Worker en segundo plano para el despacho de resúmenes diarios (PR #8, A-18).
//! Despierta alineado al límite de cada minuto (:00.000) en la zona horaria `BOT_TZ`.

use crate::bot::render::{render_daily_summary, send_chunked_html};
use crate::bot::Bot;
use crate::config::Config;
use crate::storage::users::UsersRepository;
use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
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

    /// Despacha el resumen diario a un usuario específico reutilizando `render_daily_summary` (PR #8, A-13).
    /// Si no hay pendientes, envía el mensaje único con el copy calmado `PENDIENTES_ALL_CLEAR`.
    /// Si hay entregas pendientes, envía mensajes segmentados bajo 4000 caracteres con `send_chunked_html`.
    pub async fn dispatch_summary_to_user(&self, chat_id: i64) -> Result<(), anyhow::Error> {
        let (header, items) = render_daily_summary(&self.pool, chat_id, self.config.bot_tz).await?;

        if items.is_empty() {
            self.bot
                .send_message(ChatId(chat_id), header)
                .parse_mode(ParseMode::Html)
                .await?;
        } else {
            send_chunked_html(&self.bot, ChatId(chat_id), &header, &items).await?;
        }

        Ok(())
    }

    /// Evalúa el tick de un minuto específico seleccionando los usuarios correspondientes y enviando sus resúmenes.
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

        let users =
            match UsersRepository::users_due_for_summary(&self.pool, &time_hhmm, weekday_iso).await
            {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        time = %time_hhmm,
                        "Error al consultar usuarios programados para resumen"
                    );
                    return;
                }
            };

        for user in users {
            if let Err(e) = self.dispatch_summary_to_user(user.chat_id).await {
                tracing::error!(
                    chat_id = user.chat_id,
                    error = %e,
                    "Error al despachar resumen diario al usuario"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::create_bot_with_url;
    use crate::storage::init;
    use chrono::TimeZone;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    #[tokio::test]
    async fn test_dispatch_summary_to_user_empty_fallback() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 101,
                    "date": 1700000000,
                    "chat": { "id": 5555, "type": "private" },
                    "text": "test"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_summary_worker_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 1125347763,
            database_url,
            max_users_default: 300,
            bot_tz: chrono_tz::America::Managua,
            rust_log: "info".to_string(),
            scan_interval_min: 30,
        });

        let pool = init(&config).await.unwrap();
        let bot = create_bot_with_url(
            &config.bot_token,
            reqwest::Url::parse(&format!("{}/", server.uri())).unwrap(),
        );

        UsersRepository::upsert_login(&pool, 5555, "CIF_5555", 1, "Estudiante Test", "tok")
            .await
            .unwrap();

        let worker = SummaryWorker::new(pool.clone(), bot, config);
        worker.dispatch_summary_to_user(5555).await.unwrap();

        // Limpieza
        drop(pool);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
