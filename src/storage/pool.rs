use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::str::FromStr;
use std::time::Duration;

/// Construye las opciones de conexión SQLite asegurando modo WAL,
/// foreign_keys activos, synchronous NORMAL y timeout de 5 segundos (A-02).
pub fn create_connect_options(database_url: &str) -> Result<SqliteConnectOptions, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .create_if_missing(true);
    Ok(options)
}

/// Inicializa el pool de conexiones SQLite con un límite de 5 conexiones concurrentes.
pub async fn build_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = create_connect_options(database_url)?;
    SqlitePoolOptions::new()
        .max_connections(5)
        .idle_timeout(None)
        .max_lifetime(None)
        .test_before_acquire(false)
        .acquire_timeout(Duration::from_secs(3600 * 24))
        .connect_with(options)
        .await
}
