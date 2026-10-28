use crate::storage::models::User;
use sqlx::SqlitePool;

/// Repositorio de operaciones para la tabla `users`.
pub struct UsersRepository;

impl UsersRepository {
    /// Obtiene un estudiante por su `chat_id`.
    pub async fn get(pool: &SqlitePool, chat_id: i64) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>(
            "SELECT chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason, summary_enabled, summary_times, summary_days, created_at \
             FROM users WHERE chat_id = ?"
        )
        .bind(chat_id)
        .fetch_optional(pool)
        .await
    }

    /// Obtiene un estudiante por su `uam_cif`.
    pub async fn get_by_cif(pool: &SqlitePool, cif: &str) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>(
            "SELECT chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason, summary_enabled, summary_times, summary_days, created_at \
             FROM users WHERE uam_cif = ?"
        )
        .bind(cif)
        .fetch_optional(pool)
        .await
    }

    /// Cuenta el total de estudiantes registrados en el sistema (ocupación de cupo según §0.3).
    pub async fn count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(pool)
            .await?;
        Ok(row.0)
    }

    /// Cuenta los estudiantes con estado activo.
    pub async fn count_active(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE is_active = 1")
            .fetch_one(pool)
            .await?;
        Ok(row.0)
    }

    /// Actualiza o registra la sesión de login de un usuario (A-12).
    /// Si el CIF ya existía vinculado a otro chat_id, reemplaza el vínculo anterior.
    /// Si el chat_id ya existía, reactiva la cuenta y renueva el token.
    pub async fn upsert_login(
        pool: &SqlitePool,
        chat_id: i64,
        cif: &str,
        uam_user_id: i64,
        fullname: &str,
        wstoken: &str,
    ) -> Result<User, sqlx::Error> {
        let mut tx = pool.begin().await?;

        // Si el mismo CIF estaba registrado en otro chat, se reemplaza (A-12)
        sqlx::query("DELETE FROM users WHERE uam_cif = ? AND chat_id != ?")
            .bind(cif)
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "INSERT INTO users (chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason) \
             VALUES (?, ?, ?, ?, ?, 1, NULL) \
             ON CONFLICT(chat_id) DO UPDATE SET \
                 uam_cif = excluded.uam_cif, \
                 uam_user_id = excluded.uam_user_id, \
                 fullname = excluded.fullname, \
                 wstoken = excluded.wstoken, \
                 is_active = 1, \
                 inactive_reason = NULL"
        )
        .bind(chat_id)
        .bind(cif)
        .bind(uam_user_id)
        .bind(fullname)
        .bind(wstoken)
        .execute(&mut *tx)
        .await?;

        let user = sqlx::query_as::<_, User>(
            "SELECT chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason, summary_enabled, summary_times, summary_days, created_at \
             FROM users WHERE chat_id = ?"
        )
        .bind(chat_id)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(user)
    }

    /// Marca un usuario como inactivo, especificando la razón y limpiando el token por seguridad (§0.3/A-15).
    pub async fn mark_inactive(
        pool: &SqlitePool,
        chat_id: i64,
        reason: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE users SET is_active = 0, inactive_reason = ?, wstoken = '' WHERE chat_id = ?",
        )
        .bind(reason)
        .bind(chat_id)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Elimina a un usuario del sistema (/logout).
    /// Por foreign key con ON DELETE CASCADE, esto purga user_courses y user_submissions.
    pub async fn delete(pool: &SqlitePool, chat_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM users WHERE chat_id = ?")
            .bind(chat_id)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Obtiene la lista de tokens válidos de estudiantes activos matriculados en un curso.
    /// Utilizado para rotación y failover de tokens donantes en el escaneo compartido.
    pub async fn active_tokens_for_course(
        pool: &SqlitePool,
        course_id: i64,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT u.wstoken FROM users u \
             INNER JOIN user_courses uc ON u.chat_id = uc.chat_id \
             WHERE uc.course_id = ? AND u.is_active = 1 AND u.wstoken != ''",
        )
        .bind(course_id)
        .fetch_all(pool)
        .await?;

        Ok(rows.into_iter().map(|(token,)| token).collect())
    }
}
