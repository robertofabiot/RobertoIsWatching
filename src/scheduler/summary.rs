//! Worker en segundo plano para el despacho de resúmenes diarios (PR #8, A-18).
//! Despierta alineado al límite de cada minuto (:00.000) en la zona horaria `BOT_TZ`.

use crate::bot::render::{render_daily_summary, send_chunked_html};
use crate::bot::Bot;
use crate::config::Config;
use crate::storage::{AlertsRepository, UsersRepository};
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

        if let Err(e) = self.boot_lookback().await {
            tracing::error!(error = %e, "Error al ejecutar boot lookback en SummaryWorker");
        }

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

    /// Ejecuta la política de recuperación tras reinicio (lookback de hasta 10 minutos).
    /// Evalúa cada minuto en el rango [now - 10 min .. now] para despachar resúmenes pendientes
    /// generados durante una breve interrupción, suprimiendo los anteriores a 10 min para evitar aluviones.
    pub async fn boot_lookback(&self) -> Result<(), anyhow::Error> {
        let now = Utc::now().with_timezone(&self.config.bot_tz);
        let now_floored = match now.with_second(0).and_then(|t| t.with_nanosecond(0)) {
            Some(f) => f,
            None => now,
        };

        tracing::info!(
            boot_time = %now_floored,
            "Ejecutando boot lookahead de 10 minutos para resúmenes diarios"
        );

        for minute_offset in (0..=10).rev() {
            let eval_time = now_floored - chrono::Duration::minutes(minute_offset);
            self.evaluate_minute(&eval_time).await;
        }

        tracing::info!("Boot lookahead de resúmenes diarios completado");
        Ok(())
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

    /// Evalúa el tick de un minuto específico seleccionando los usuarios correspondientes y enviando sus resúmenes
    /// bajo el patrón atómico de idempotencia claim -> send -> release-on-fail.
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
            let key = format!("summary:{}:{}:{}", user.chat_id, date_ymd, time_hhmm);

            match AlertsRepository::try_claim(&self.pool, &key, "summary", Some(user.chat_id)).await
            {
                Ok(true) => {
                    let send_res = self.dispatch_summary_to_user(user.chat_id).await;
                    match send_res {
                        Ok(()) => {
                            tracing::info!(
                                chat_id = user.chat_id,
                                key = %key,
                                "Resumen diario despachado exitosamente"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                chat_id = user.chat_id,
                                key = %key,
                                error = %err,
                                "Error al enviar resumen diario por Telegram"
                            );
                            let err_lower = err.to_string().to_lowercase();
                            if err_lower.contains("bot was blocked by the user")
                                || err_lower.contains("user is deactivated")
                                || err_lower.contains("chat not found")
                            {
                                tracing::warn!(
                                    chat_id = user.chat_id,
                                    "Usuario bloqueó el bot o cuenta desactivada. Marcando inactivo."
                                );
                                let _ = UsersRepository::mark_inactive(
                                    &self.pool,
                                    user.chat_id,
                                    "bot_blocked",
                                )
                                .await;
                            } else {
                                // Error transitorio de red: liberar el claim para permitir reintento
                                let _ = AlertsRepository::release(&self.pool, &key).await;
                            }
                        }
                    }
                }
                Ok(false) => {
                    tracing::debug!(
                        chat_id = user.chat_id,
                        key = %key,
                        "Resumen ya reclamado o enviado previamente; deduplicación activada"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        chat_id = user.chat_id,
                        key = %key,
                        error = %e,
                        "Error al intentar reclamar clave de idempotencia en sent_alerts"
                    );
                }
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

    #[tokio::test]
    async fn test_evaluate_minute_idempotency_and_claim() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 102,
                    "date": 1700000000,
                    "chat": { "id": 6666, "type": "private" },
                    "text": "test"
                }
            })))
            .expect(1) // Debe ser llamado exactamente UNA vez
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_summary_idem_{rand_id}.db"));
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

        let user =
            UsersRepository::upsert_login(&pool, 6666, "CIF_6666", 1, "Estudiante Idem", "tok")
                .await
                .unwrap();
        UsersRepository::update_summary_config(
            &pool,
            user.chat_id,
            true,
            r#"["07:30"]"#,
            r#"[5]"#, // Viernes
        )
        .await
        .unwrap();

        let worker = SummaryWorker::new(pool.clone(), bot, config.clone());

        // Viernes 2026-10-30 a las 07:30 en Managua
        let dt = config
            .bot_tz
            .with_ymd_and_hms(2026, 10, 30, 7, 30, 0)
            .unwrap();

        // Primera evaluación: reclama clave y envía mensaje
        worker.evaluate_minute(&dt).await;

        let key = "summary:6666:2026-10-30:07:30";
        assert!(AlertsRepository::is_claimed(&pool, key).await.unwrap());

        // Segunda evaluación para el mismo minuto: omitido por idempotencia
        worker.evaluate_minute(&dt).await;

        drop(pool);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }

    #[tokio::test]
    async fn test_evaluate_minute_blocked_bot_marks_inactive() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "ok": false,
                "error_code": 403,
                "description": "Forbidden: bot was blocked by the user"
            })))
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_summary_block_{rand_id}.db"));
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

        let user =
            UsersRepository::upsert_login(&pool, 7777, "CIF_7777", 1, "Estudiante Block", "tok")
                .await
                .unwrap();
        UsersRepository::update_summary_config(&pool, user.chat_id, true, r#"["07:30"]"#, r#"[5]"#)
            .await
            .unwrap();

        let worker = SummaryWorker::new(pool.clone(), bot, config.clone());
        let dt = config
            .bot_tz
            .with_ymd_and_hms(2026, 10, 30, 7, 30, 0)
            .unwrap();

        worker.evaluate_minute(&dt).await;

        let updated_user = UsersRepository::get(&pool, 7777).await.unwrap().unwrap();
        assert!(!updated_user.is_active);
        assert_eq!(updated_user.inactive_reason.as_deref(), Some("bot_blocked"));

        drop(pool);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    }
}
