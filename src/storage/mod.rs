pub mod models;
pub mod pool;
pub mod users;

pub use models::*;
pub use pool::{build_pool, create_connect_options};
pub use users::{AdmissionResult, UsersRepository};

use crate::config::Config;
use sqlx::SqlitePool;

/// Inicializa el pool SQLite, ejecuta migraciones embebidas y siembra parámetros por defecto.
pub async fn init(config: &Config) -> Result<SqlitePool, sqlx::Error> {
    let pool = build_pool(&config.database_url).await?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| sqlx::Error::Configuration(Box::new(e)))?;

    sqlx::query("INSERT OR IGNORE INTO system_settings (key, value) VALUES ('max_users', ?)")
        .bind(config.max_users_default.to_string())
        .execute(&pool)
        .await?;

    Ok(pool)
}
