pub mod alerts;
pub mod assignments;
pub mod courses;
pub mod models;
pub mod modules;
pub mod pool;
pub mod settings;
pub mod submissions;
pub mod users;

pub use alerts::AlertsRepository;
pub use assignments::{
    AssignmentChanges, AssignmentsRepository, PendingAssignmentItem, UpsertAssignmentResult,
};
pub use courses::CoursesRepository;
pub use models::*;
pub use modules::ModulesRepository;
pub use pool::{build_pool, create_connect_options};
pub use settings::SettingsRepository;
pub use submissions::{
    GradedSubmissionItem, SubmissionChanges, SubmissionData, SubmissionsRepository,
    UpsertSubmissionResult,
};
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
