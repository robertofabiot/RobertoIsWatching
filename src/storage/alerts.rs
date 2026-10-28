use sqlx::SqlitePool;

/// Repositorio de operaciones para la tabla `sent_alerts`.
/// Implementa el patrón claim -> send -> release-si-falla (A-14) para garantizar idempotencia.
pub struct AlertsRepository;

impl AlertsRepository {
    /// Intenta reclamar de forma atómica una clave de alerta antes de enviarla.
    /// Retorna `true` si la alerta fue reclamada (no había sido enviada previamente).
    /// Retorna `false` si la alerta ya existía en la base de datos (evita duplicados).
    pub async fn try_claim(
        pool: &SqlitePool,
        alert_key: &str,
        alert_type: &str,
        chat_id: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "INSERT OR IGNORE INTO sent_alerts (alert_key, alert_type, chat_id, created_at) \
             VALUES (?, ?, ?, CURRENT_TIMESTAMP)",
        )
        .bind(alert_key)
        .bind(alert_type)
        .bind(chat_id)
        .execute(pool)
        .await?;

        Ok(res.rows_affected() > 0)
    }

    /// Libera una alerta previamente reclamada si el envío de mensaje a Telegram falló.
    pub async fn release(pool: &SqlitePool, alert_key: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM sent_alerts WHERE alert_key = ?")
            .bind(alert_key)
            .execute(pool)
            .await?;

        Ok(res.rows_affected() > 0)
    }

    /// Comprueba si una alerta ya ha sido reclamada/enviada.
    pub async fn is_claimed(pool: &SqlitePool, alert_key: &str) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT 1 FROM sent_alerts WHERE alert_key = ?")
            .bind(alert_key)
            .fetch_optional(pool)
            .await?;

        Ok(row.is_some())
    }

    /// Purga alertas con antigüedad mayor a `days` días (política de retención de 60 días según A-14).
    pub async fn purge_older_than(pool: &SqlitePool, days: i64) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "DELETE FROM sent_alerts WHERE created_at < datetime('now', '-' || ? || ' days')",
        )
        .bind(days)
        .execute(pool)
        .await?;

        Ok(res.rows_affected())
    }

    /// Purga todas las alertas asociadas a un chat_id específico (usado al hacer /logout).
    pub async fn purge_chat(pool: &SqlitePool, chat_id: i64) -> Result<u64, sqlx::Error> {
        let res = sqlx::query("DELETE FROM sent_alerts WHERE chat_id = ?")
            .bind(chat_id)
            .execute(pool)
            .await?;

        Ok(res.rows_affected())
    }
}
