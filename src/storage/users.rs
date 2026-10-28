use crate::storage::models::User;
use sqlx::{Executor, SqlitePool};

/// Resultado del intento atómico de admisión al sistema (A-12).
#[derive(Debug, PartialEq, Eq)]
pub enum AdmissionResult {
    /// Admitido como nuevo alumno dentro del cupo disponible.
    Admitted(User),
    /// Ya era miembro (re-login de chat existente o reactivación sin consumir nuevo cupo).
    AlreadyMember(User),
    /// Capacidad máxima de alumnos alcanzada.
    Full,
    /// El registro general de nuevos alumnos se encuentra temporalmente cerrado.
    Closed,
}

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

    /// Intenta admitir a un usuario de forma estrictamente atómica usando BEGIN IMMEDIATE (A-12).
    /// Previene carreras entre múltiples logins concurrentes y asegura que el re-login de un
    /// chat_id ya existente no consuma cupo ni sea rechazado.
    pub async fn try_admit(
        pool: &SqlitePool,
        chat_id: i64,
        cif: &str,
        uam_user_id: i64,
        fullname: &str,
        wstoken: &str,
    ) -> Result<AdmissionResult, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        conn.execute("BEGIN IMMEDIATE").await?;

        // 1. Verificar si el usuario ya es miembro existente (por chat_id)
        let existing_user = sqlx::query_as::<_, User>(
            "SELECT chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason, summary_enabled, summary_times, summary_days, created_at \
             FROM users WHERE chat_id = ?"
        )
        .bind(chat_id)
        .fetch_optional(&mut *conn)
        .await;

        let existing_user = match existing_user {
            Ok(u) => u,
            Err(e) => {
                let _ = conn.execute("ROLLBACK").await;
                return Err(e);
            }
        };

        if let Some(_user) = existing_user {
            // Re-login: si el CIF estaba en otro chat, liberar el vínculo previo (A-12)
            if let Err(e) = sqlx::query("DELETE FROM users WHERE uam_cif = ? AND chat_id != ?")
                .bind(cif)
                .bind(chat_id)
                .execute(&mut *conn)
                .await
            {
                let _ = conn.execute("ROLLBACK").await;
                return Err(e);
            }

            // Actualizar datos de sesión y reactivar cuenta
            let update_res = sqlx::query(
                "UPDATE users SET \
                 uam_cif = ?, \
                 uam_user_id = ?, \
                 fullname = ?, \
                 wstoken = ?, \
                 is_active = 1, \
                 inactive_reason = NULL \
                 WHERE chat_id = ?",
            )
            .bind(cif)
            .bind(uam_user_id)
            .bind(fullname)
            .bind(wstoken)
            .bind(chat_id)
            .execute(&mut *conn)
            .await;

            if let Err(e) = update_res {
                let _ = conn.execute("ROLLBACK").await;
                return Err(e);
            }

            let updated_user = sqlx::query_as::<_, User>(
                "SELECT chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason, summary_enabled, summary_times, summary_days, created_at \
                 FROM users WHERE chat_id = ?"
            )
            .bind(chat_id)
            .fetch_one(&mut *conn)
            .await;

            let updated_user = match updated_user {
                Ok(u) => u,
                Err(e) => {
                    let _ = conn.execute("ROLLBACK").await;
                    return Err(e);
                }
            };

            conn.execute("COMMIT").await?;
            return Ok(AdmissionResult::AlreadyMember(updated_user));
        }

        // 2. Si no es miembro, verificar si el registro está abierto
        let reg_setting = sqlx::query_as::<_, (String,)>(
            "SELECT value FROM system_settings WHERE key = 'registration_open'",
        )
        .fetch_optional(&mut *conn)
        .await;

        let reg_open = match reg_setting {
            Ok(Some((val,))) => val.trim().eq_ignore_ascii_case("true"),
            Ok(None) => true,
            Err(e) => {
                let _ = conn.execute("ROLLBACK").await;
                return Err(e);
            }
        };

        if !reg_open {
            conn.execute("ROLLBACK").await?;
            return Ok(AdmissionResult::Closed);
        }

        // 3. Verificar límite de cupos
        let max_setting = sqlx::query_as::<_, (String,)>(
            "SELECT value FROM system_settings WHERE key = 'max_users'",
        )
        .fetch_optional(&mut *conn)
        .await;

        let max_users: i64 = match max_setting {
            Ok(Some((val,))) => val.trim().parse::<i64>().unwrap_or(300),
            Ok(None) => 300,
            Err(e) => {
                let _ = conn.execute("ROLLBACK").await;
                return Err(e);
            }
        };

        let count_res: Result<(i64,), sqlx::Error> = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *conn)
            .await;

        let current_count = match count_res {
            Ok((count,)) => count,
            Err(e) => {
                let _ = conn.execute("ROLLBACK").await;
                return Err(e);
            }
        };

        if current_count >= max_users {
            conn.execute("ROLLBACK").await?;
            return Ok(AdmissionResult::Full);
        }

        // 4. Si el mismo CIF existía en otro chat, liberar ese registro (A-12)
        if let Err(e) = sqlx::query("DELETE FROM users WHERE uam_cif = ? AND chat_id != ?")
            .bind(cif)
            .bind(chat_id)
            .execute(&mut *conn)
            .await
        {
            let _ = conn.execute("ROLLBACK").await;
            return Err(e);
        }

        // 5. Insertar nuevo usuario
        if let Err(e) = sqlx::query(
            "INSERT INTO users (chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason) \
             VALUES (?, ?, ?, ?, ?, 1, NULL)",
        )
        .bind(chat_id)
        .bind(cif)
        .bind(uam_user_id)
        .bind(fullname)
        .bind(wstoken)
        .execute(&mut *conn)
        .await
        {
            let _ = conn.execute("ROLLBACK").await;
            return Err(e);
        }

        let new_user = sqlx::query_as::<_, User>(
            "SELECT chat_id, uam_cif, uam_user_id, fullname, wstoken, is_active, inactive_reason, summary_enabled, summary_times, summary_days, created_at \
             FROM users WHERE chat_id = ?"
        )
        .bind(chat_id)
        .fetch_one(&mut *conn)
        .await;

        let new_user = match new_user {
            Ok(u) => u,
            Err(e) => {
                let _ = conn.execute("ROLLBACK").await;
                return Err(e);
            }
        };

        conn.execute("COMMIT").await?;
        Ok(AdmissionResult::Admitted(new_user))
    }
}
