//! Servicio de despacho e idempotencia de alertas (PR #6, RF-09, A-14, A-15).
//! Implementa el patrón atómico claim -> send -> release-on-fail.

use crate::bot::Bot;
use crate::storage::{AlertsRepository, UsersRepository};
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use teloxide::{ApiError, RequestError};

/// Calcula un hash FNV-1a de 64 bits en formato hexadecimal estable (RF-09).
pub fn fnv1a_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// Servicio encargado del despacho idempotente de alertas por Telegram.
pub struct AlertService;

impl AlertService {
    /// Despacha una alerta aplicando el patrón claim -> send -> release (RF-09, A-14, A-15).
    /// Retorna `Ok(true)` si la alerta fue reclamada y enviada exitosamente.
    /// Retorna `Ok(false)` si ya había sido enviada previamente (idempotencia) o falló el envío.
    pub async fn dispatch(
        pool: &SqlitePool,
        bot: &Bot,
        alert_key: &str,
        alert_type: &str,
        chat_id: i64,
        html_message: &str,
    ) -> Result<bool, anyhow::Error> {
        // 1. Intento de reclamo atómico en SQLite (INSERT OR IGNORE)
        let claimed =
            AlertsRepository::try_claim(pool, alert_key, alert_type, Some(chat_id)).await?;
        if !claimed {
            tracing::debug!(
                alert_key = alert_key,
                "Alerta ya reclamada previamente; deduplicación activada (A-14)"
            );
            return Ok(false);
        }

        // 2. Envío del mensaje por Telegram con ParseMode::Html
        match bot
            .send_message(ChatId(chat_id), html_message)
            .parse_mode(ParseMode::Html)
            .await
        {
            Ok(_) => {
                tracing::info!(
                    alert_key = alert_key,
                    chat_id = chat_id,
                    alert_type = alert_type,
                    "Alerta despachada exitosamente"
                );
                Ok(true)
            }
            Err(err) => {
                tracing::warn!(
                    alert_key = alert_key,
                    chat_id = chat_id,
                    error = %err,
                    "Error al enviar alerta por Telegram"
                );

                // 3. Clasificación de errores de Telegram (A-15, R-10):
                let is_blocked = match &err {
                    RequestError::Api(
                        ApiError::BotBlocked | ApiError::UserDeactivated | ApiError::ChatNotFound,
                    ) => true,
                    _ => {
                        let err_lower = err.to_string().to_lowercase();
                        err_lower.contains("bot was blocked by the user")
                            || err_lower.contains("user is deactivated")
                            || err_lower.contains("chat not found")
                    }
                };

                if is_blocked {
                    tracing::warn!(
                        chat_id = chat_id,
                        "Usuario bloqueó el bot o cuenta desactivada. Marcando inactivo."
                    );
                    let _ = UsersRepository::mark_inactive(pool, chat_id, "bot_blocked").await;
                } else {
                    // Error transitorio de red: liberar el claim para permitir reintento en el próximo ciclo
                    let _ = AlertsRepository::release(pool, alert_key).await;
                }

                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::create_bot_with_url;
    use crate::config::Config;
    use crate::storage::init;
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_fnv1a_hash_determinism() {
        let h1 = fnv1a_hash("95/100|Excelente trabajo");
        let h2 = fnv1a_hash("95/100|Excelente trabajo");
        let h3 = fnv1a_hash("90/100|Excelente trabajo");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 16);
    }

    #[tokio::test]
    async fn test_alert_dispatcher_claim_send_and_dedup() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 1,
                    "date": 1700000000,
                    "chat": { "id": 1234, "type": "private" },
                    "text": "test"
                }
            })))
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_alerts_disp_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 123456789,
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

        let key = "new_assign:1234:55";
        let msg = "<b>Nueva tarea</b>";

        // Primer envío: exitoso y reclamado
        let sent = AlertService::dispatch(&pool, &bot, key, "new_assignment", 1234, msg)
            .await
            .unwrap();
        assert!(sent);
        assert!(AlertsRepository::is_claimed(&pool, key).await.unwrap());

        // Segundo intento con la misma clave: debe ser suprimido (RF-09, A-14)
        let sent_again = AlertService::dispatch(&pool, &bot, key, "new_assignment", 1234, msg)
            .await
            .unwrap();
        assert!(!sent_again);
    }

    #[tokio::test]
    async fn test_alert_dispatcher_bot_blocked_marks_inactive() {
        let server = MockServer::start().await;

        // Mock Telegram devolviendo 403 Forbidden: bot was blocked by the user
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
        let db_path = std::env::temp_dir().join(format!("test_alerts_blocked_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 123456789,
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

        // Crear usuario activo
        UsersRepository::upsert_login(
            &pool,
            4030,
            "24014030",
            4030,
            "Blocked User",
            "token_blocked",
        )
        .await
        .unwrap();

        let key = "new_assign:4030:10";
        let res = AlertService::dispatch(&pool, &bot, key, "new_assignment", 4030, "Aviso")
            .await
            .unwrap();
        assert!(!res);

        // Verificar que el usuario fue marcado como inactivo por 'bot_blocked' (A-15)
        let user = UsersRepository::get(&pool, 4030).await.unwrap().unwrap();
        assert!(!user.is_active);
        assert_eq!(user.inactive_reason.as_deref(), Some("bot_blocked"));
    }

    #[tokio::test]
    async fn test_alert_dispatcher_transient_error_releases_claim() {
        let server = MockServer::start().await;

        // Mock Telegram devolviendo error 500 transitorio
        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "ok": false,
                "error_code": 500,
                "description": "Internal Server Error"
            })))
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_alerts_transient_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 123456789,
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

        let key = "grade:5000:20:abcd";

        // Dispatch falla por error 500
        let res = AlertService::dispatch(&pool, &bot, key, "grade", 5000, "Nota")
            .await
            .unwrap();
        assert!(!res);

        // La clave debe haber sido liberada (release) para reintento en el próximo ciclo (A-15)
        let claimed = AlertsRepository::is_claimed(&pool, key).await.unwrap();
        assert!(
            !claimed,
            "La alerta debe liberarse tras un error transitorio"
        );
    }

    #[tokio::test]
    async fn test_alert_dispatcher_user_deactivated_marks_inactive() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "ok": false,
                "error_code": 403,
                "description": "Forbidden: user is deactivated"
            })))
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_alerts_deactivated_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 123456789,
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

        UsersRepository::upsert_login(
            &pool,
            4031,
            "24014031",
            4031,
            "Deactivated User",
            "token_deact",
        )
        .await
        .unwrap();

        let key = "new_assign:4031:11";
        let res = AlertService::dispatch(&pool, &bot, key, "new_assignment", 4031, "Aviso")
            .await
            .unwrap();
        assert!(!res);

        let user = UsersRepository::get(&pool, 4031).await.unwrap().unwrap();
        assert!(!user.is_active);
        assert_eq!(user.inactive_reason.as_deref(), Some("bot_blocked"));
    }
}
