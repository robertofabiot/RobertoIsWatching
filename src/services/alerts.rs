//! Servicio de despacho e idempotencia de alertas (PR #6, RF-09, A-14, A-15).
//! Implementa el patrón atómico claim -> send -> release-on-fail.

use crate::bot::Bot;
use crate::storage::{AlertsRepository, UsersRepository};
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

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

                // 3. Clasificación de errores de Telegram (A-15):
                let err_str = err.to_string();
                if err_str.contains("bot was blocked by the user")
                    || err_str.contains("user is deactivated")
                    || err_str.contains("chat not found")
                {
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

    #[test]
    fn test_fnv1a_hash_determinism() {
        let h1 = fnv1a_hash("95/100|Excelente trabajo");
        let h2 = fnv1a_hash("95/100|Excelente trabajo");
        let h3 = fnv1a_hash("90/100|Excelente trabajo");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 16);
    }
}
