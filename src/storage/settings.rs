use sqlx::SqlitePool;

/// Repositorio de operaciones para la tabla `system_settings`.
/// Gestiona parámetros globales tipados como estado de registro y límite de cupos.
pub struct SettingsRepository;

impl SettingsRepository {
    /// Obtiene el valor en crudo de un parámetro por su clave.
    pub async fn get(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM system_settings WHERE key = ?")
                .bind(key)
                .fetch_optional(pool)
                .await?;

        Ok(row.map(|(v,)| v))
    }

    /// Establece o actualiza un parámetro global en el sistema.
    pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO system_settings (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Retorna si el registro general de nuevos alumnos está abierto (por defecto `true`).
    pub async fn registration_open(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
        match Self::get(pool, "registration_open").await? {
            Some(val) => Ok(val.trim().eq_ignore_ascii_case("true")),
            None => Ok(true),
        }
    }

    /// Modifica el estado del registro general de alumnos.
    pub async fn set_registration_open(pool: &SqlitePool, open: bool) -> Result<(), sqlx::Error> {
        Self::set(
            pool,
            "registration_open",
            if open { "true" } else { "false" },
        )
        .await
    }

    /// Conmuta el estado de apertura de registro y retorna el nuevo estado.
    pub async fn toggle_registration_open(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
        let current = Self::registration_open(pool).await?;
        let next = !current;
        Self::set_registration_open(pool, next).await?;
        Ok(next)
    }

    /// Retorna la cantidad máxima de usuarios permitidos en el sistema (por defecto 300).
    pub async fn max_users(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        match Self::get(pool, "max_users").await? {
            Some(val) => Ok(val.trim().parse::<i64>().unwrap_or(300)),
            None => Ok(300),
        }
    }

    /// Modifica la cantidad máxima de usuarios permitidos en el sistema.
    pub async fn set_max_users(pool: &SqlitePool, max: i64) -> Result<(), sqlx::Error> {
        Self::set(pool, "max_users", &max.to_string()).await
    }
}
