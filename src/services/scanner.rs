//! Motor de escaneo compartido en segundo plano (PR #6, Plan Maestro §4).
//! Ejecuta un worker loop en Tokio con periodicidad configurable y parada limpia.

use crate::bot::Bot;
use crate::config::Config;
use crate::moodle::gate::Priority;
use crate::moodle::MoodleClient;
use crate::services::session;
use crate::storage::{CoursesRepository, UsersRepository};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

pub const DEFAULT_INITIAL_DELAY_SECS: u64 = 5;

/// Resumen de estadísticas de ejecución de un ciclo de escaneo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanCycleSummary {
    pub courses_scanned: usize,
    pub courses_failed: usize,
    pub new_assignments: usize,
    pub new_materials: usize,
    pub submissions_checked: usize,
    pub alerts_dispatched: usize,
}

/// Worker asíncrono que ejecuta periódicamente el escaneo compartido (RF-02, RF-08).
pub struct ScannerWorker {
    pub(crate) pool: SqlitePool,
    pub(crate) moodle: MoodleClient,
    pub(crate) bot: Bot,
    #[allow(dead_code)]
    pub(crate) config: Arc<Config>,
    pub(crate) interval: Duration,
    pub(crate) initial_delay: Duration,
}

impl ScannerWorker {
    pub fn new(pool: SqlitePool, moodle: MoodleClient, bot: Bot, config: Arc<Config>) -> Self {
        let interval = config.scan_interval();
        Self {
            pool,
            moodle,
            bot,
            config,
            interval,
            initial_delay: Duration::from_secs(DEFAULT_INITIAL_DELAY_SECS),
        }
    }

    /// Permite personalizar el intervalo y el retardo inicial (esencial para tests).
    pub fn with_delays(mut self, interval: Duration, initial_delay: Duration) -> Self {
        self.interval = interval;
        self.initial_delay = initial_delay;
        self
    }

    /// Ejecuta un único ciclo de escaneo compartido con failover de donantes (RF-02, RF-08).
    pub async fn run_scan_cycle(&self) -> Result<ScanCycleSummary, anyhow::Error> {
        tracing::info!("Iniciando ciclo de escaneo compartido de Moodle...");
        let mut summary = ScanCycleSummary::default();

        let courses = CoursesRepository::unique_active_courses(&self.pool).await?;
        for course in courses {
            let donors = UsersRepository::active_tokens_for_course(&self.pool, course.id).await?;
            if donors.is_empty() {
                tracing::warn!(
                    course_id = course.id,
                    "No hay donantes activos para el curso"
                );
                summary.courses_failed += 1;
                continue;
            }

            // 1. Obtener tareas con rotación y failover de donantes con prioridad Background
            let assignments_outcome = match self
                .moodle
                .fetch_course_with_failover(&donors, course.id, Priority::Background)
                .await
            {
                Ok(outcome) => outcome,
                Err(err) => {
                    tracing::warn!(
                        course_id = course.id,
                        error = %err,
                        "Todos los donantes fallaron al consultar tareas del curso"
                    );
                    summary.courses_failed += 1;
                    continue;
                }
            };

            // Notificar y desactivar donantes cuyos tokens hayan sido revocados (A-22)
            for invalid_chat_id in &assignments_outcome.invalid {
                if let Err(e) = session::expire_user(&self.pool, &self.bot, *invalid_chat_id).await
                {
                    tracing::error!(
                        chat_id = invalid_chat_id,
                        error = %e,
                        "Error al expirar sesión de usuario con token revocado"
                    );
                }
            }

            // Filtrar donantes inválidos para la consulta de contenidos
            let valid_donors: Vec<_> = donors
                .into_iter()
                .filter(|d| !assignments_outcome.invalid.contains(&d.chat_id))
                .collect();

            // 2. Obtener contenidos/módulos con failover
            if !valid_donors.is_empty() {
                if let Ok(contents_outcome) = self
                    .moodle
                    .fetch_course_contents_with_failover(
                        &valid_donors,
                        course.id,
                        Priority::Background,
                    )
                    .await
                {
                    for invalid_chat_id in &contents_outcome.invalid {
                        if let Err(e) =
                            session::expire_user(&self.pool, &self.bot, *invalid_chat_id).await
                        {
                            tracing::error!(
                                chat_id = invalid_chat_id,
                                error = %e,
                                "Error al expirar sesión de usuario con token revocado en contenidos"
                            );
                        }
                    }
                }
            }

            summary.courses_scanned += 1;
        }

        tracing::info!(
            courses = summary.courses_scanned,
            failed = summary.courses_failed,
            alerts = summary.alerts_dispatched,
            "Ciclo de escaneo compartido finalizado."
        );
        Ok(summary)
    }

    /// Inicia el bucle periódico de escaneo cancelable mediante `shutdown_rx`.
    pub async fn run_loop(self, mut shutdown_rx: watch::Receiver<bool>) {
        tracing::info!(
            interval_mins = self.interval.as_secs() / 60,
            initial_delay_secs = self.initial_delay.as_secs(),
            "ScannerWorker loop iniciado."
        );

        // 1. Retardo corto inicial antes del primer escaneo
        tokio::select! {
            _ = tokio::time::sleep(self.initial_delay) => {}
            _ = shutdown_rx.changed() => {
                tracing::info!("ScannerWorker apagado durante retardo inicial.");
                return;
            }
        }

        // Primer ciclo tras el retardo inicial
        if let Err(err) = self.run_scan_cycle().await {
            tracing::error!(error = %err, "Error en ciclo inicial de escaneo.");
        }

        // Bucle continuo cada `interval`
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    tracing::info!("ScannerWorker recibió señal de apagado. Deteniendo worker...");
                    break;
                }
                _ = tokio::time::sleep(self.interval) => {
                    if let Err(err) = self.run_scan_cycle().await {
                        tracing::error!(error = %err, "Error durante ciclo periódico de escaneo.");
                    }
                }
            }
        }

        tracing::info!("ScannerWorker detenido limpiamente.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scanner_worker_shutdown() {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 1125347763,
            database_url: "sqlite::memory:".to_string(),
            max_users_default: 300,
            bot_tz: chrono_tz::America::Managua,
            rust_log: "info".to_string(),
            scan_interval_min: 30,
        });

        let pool = crate::storage::init(&config).await.unwrap();
        let bot = crate::bot::create_bot(&config.bot_token);
        let moodle = MoodleClient::new("https://mock.uam.ni");

        let worker = ScannerWorker::new(pool, moodle, bot, config)
            .with_delays(Duration::from_millis(50), Duration::from_millis(10));

        let handle = tokio::spawn(async move {
            worker.run_loop(shutdown_rx).await;
        });

        // Esperar a que pase el retardo inicial y corra el primer ciclo
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Enviar señal de apagado
        shutdown_tx.send(true).unwrap();

        // El worker debe terminar prontamente
        let res = tokio::time::timeout(Duration::from_millis(500), handle).await;
        assert!(res.is_ok(), "Worker no respondió a shutdown a tiempo");
    }

    #[tokio::test]
    async fn test_scanner_donor_failover_and_expiry() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // Responder para token inválido
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wstoken=bad_token_111",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "exception": "moodle_exception",
                "errorcode": "invalidtoken",
                "message": "Token inválido"
            })))
            .mount(&server)
            .await;

        // Responder para token válido (devuelve tareas)
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wstoken=good_token_222",
            ))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=mod_assign_get_assignments",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "courses": [{
                    "id": 999,
                    "assignments": []
                }]
            })))
            .mount(&server)
            .await;

        // Responder para core_course_get_contents con buen token
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wstoken=good_token_222",
            ))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=core_course_get_contents",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_scanner_failover_{rand_id}.db"));
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

        let pool = crate::storage::init(&config).await.unwrap();
        let bot = crate::bot::create_bot(&config.bot_token);
        let moodle = MoodleClient::new(server.uri());

        // Insertar curso 999
        CoursesRepository::upsert_course(&pool, 999, "Curso de Prueba", Some("PRUEBA"))
            .await
            .unwrap();

        // Insertar User 1 con bad_token
        UsersRepository::upsert_login(&pool, 111, "24010111", 11, "User Bad", "bad_token_111")
            .await
            .unwrap();
        CoursesRepository::link_user(&pool, 111, 999).await.unwrap();

        // Insertar User 2 con good_token
        UsersRepository::upsert_login(&pool, 222, "24010222", 22, "User Good", "good_token_222")
            .await
            .unwrap();
        CoursesRepository::link_user(&pool, 222, 999).await.unwrap();

        let worker = ScannerWorker::new(pool.clone(), moodle, bot, config);

        let summary = worker.run_scan_cycle().await.unwrap();
        assert_eq!(summary.courses_scanned, 1);
        assert_eq!(summary.courses_failed, 0);

        // Verificar que el usuario 111 fue desactivado por token expirado
        let u1 = UsersRepository::get(&pool, 111).await.unwrap().unwrap();
        assert!(!u1.is_active);
        assert_eq!(u1.inactive_reason.as_deref(), Some("token_expired"));

        // Verificar que el usuario 222 sigue activo
        let u2 = UsersRepository::get(&pool, 222).await.unwrap().unwrap();
        assert!(u2.is_active);
    }
}
