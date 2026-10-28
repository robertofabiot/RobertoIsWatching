//! Motor de escaneo compartido en segundo plano (PR #6, Plan Maestro §4).
//! Ejecuta un worker loop en Tokio con periodicidad configurable y parada limpia.

use crate::bot::{copy, Bot};
use crate::config::Config;
use crate::moodle::gate::Priority;
use crate::moodle::MoodleClient;
use crate::services::alerts::AlertService;
use crate::services::session;
use crate::storage::assignments::UpsertAssignmentResult;
use crate::storage::submissions::SubmissionData;
use crate::storage::{
    AssignmentsRepository, CoursesRepository, ModulesRepository, SubmissionsRepository,
    UsersRepository,
};
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
            let contents_outcome_opt = if !valid_donors.is_empty() {
                match self
                    .moodle
                    .fetch_course_contents_with_failover(
                        &valid_donors,
                        course.id,
                        Priority::Background,
                    )
                    .await
                {
                    Ok(outcome) => {
                        for invalid_chat_id in &outcome.invalid {
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
                        Some(outcome)
                    }
                    Err(err) => {
                        tracing::warn!(
                            course_id = course.id,
                            error = %err,
                            "Error al obtener contenidos del curso con failover"
                        );
                        None
                    }
                }
            } else {
                None
            };

            // 3. Siembra silenciosa (Línea base A-03):
            // Si `last_scanned IS NULL`, el curso nunca había sido escaneado previamente.
            // Se persisten tareas y módulos en SQLite sin emitir alertas para evitar la tormenta
            // de notificaciones sobre ítems preexistentes.
            let is_baseline = course.last_scanned.is_none();
            if is_baseline {
                tracing::info!(
                    course_id = course.id,
                    "Primer escaneo del curso detectado: ejecutando siembra silenciosa (A-03)"
                );
            }

            // Upsert de tareas
            let mut newly_inserted_assignments = Vec::new();
            for assign in assignments_outcome.data {
                let res = AssignmentsRepository::upsert(
                    &self.pool,
                    assign.id,
                    assign.cmid,
                    course.id,
                    &assign.name,
                    Some(assign.duedate),
                    assign.cutoffdate,
                )
                .await?;

                match res {
                    UpsertAssignmentResult::Inserted(inserted_assign) => {
                        summary.new_assignments += 1;
                        if !is_baseline {
                            newly_inserted_assignments.push(inserted_assign);
                        }
                    }
                    UpsertAssignmentResult::Updated { .. } => {}
                    UpsertAssignmentResult::Unchanged(_) => {}
                }
            }

            // Si el curso ya tenía línea base, notificar a los alumnos inscritos (RF-03)
            // tras confirmar visibilidad individualmente con su token (A-07)
            if !is_baseline && !newly_inserted_assignments.is_empty() {
                let enrolled_students =
                    UsersRepository::active_users_for_course(&self.pool, course.id).await?;

                for assign in &newly_inserted_assignments {
                    for student in &enrolled_students {
                        // Confirmar visibilidad con el token del propio alumno (A-07)
                        match self
                            .moodle
                            .fetch_user_submission(
                                &student.wstoken,
                                assign.id,
                                student.uam_user_id,
                                Priority::Background,
                            )
                            .await
                        {
                            Ok(sub) => {
                                // Guardar registro inicial de entrega
                                let _ = SubmissionsRepository::upsert(
                                    &self.pool,
                                    SubmissionData {
                                        chat_id: student.chat_id,
                                        assignment_id: assign.id,
                                        status: sub.status.as_deref(),
                                        graded: sub.graded,
                                        grade: sub.grade.as_deref(),
                                        feedback: sub.feedback_text.as_deref(),
                                        due_override: sub.due_override,
                                    },
                                )
                                .await;

                                let effective_due =
                                    sub.due_override.or(assign.duedate).unwrap_or(0);
                                let due_str = if effective_due == 0 {
                                    "Sin fecha límite".to_string()
                                } else {
                                    copy::format_epoch_local(effective_due, self.config.bot_tz)
                                };

                                let msg = copy::alert_new_assignment(
                                    &assign.name,
                                    &course.fullname,
                                    &due_str,
                                );
                                let alert_key =
                                    format!("new_assign:{}:{}", student.chat_id, assign.id);

                                if AlertService::dispatch(
                                    &self.pool,
                                    &self.bot,
                                    &alert_key,
                                    "new_assignment",
                                    student.chat_id,
                                    &msg,
                                )
                                .await
                                .unwrap_or(false)
                                {
                                    summary.alerts_dispatched += 1;
                                }
                            }
                            Err(err) if err.is_token_error() => {
                                let _ =
                                    session::expire_user(&self.pool, &self.bot, student.chat_id)
                                        .await;
                            }
                            Err(err) => {
                                tracing::debug!(
                                    assign_id = assign.id,
                                    chat_id = student.chat_id,
                                    error = %err,
                                    "Tarea no visible para este estudiante (grupo/sección distinta o sin acceso)"
                                );
                            }
                        }
                    }
                }
            }

            // Upsert de módulos
            if let Some(contents_outcome) = contents_outcome_opt {
                for m in contents_outcome.data {
                    if let Some(_inserted) = ModulesRepository::upsert_returning_new(
                        &self.pool,
                        m.id,
                        course.id,
                        m.section_name.as_deref(),
                        Some(&m.mod_name),
                        &m.name,
                        m.url.as_deref(),
                    )
                    .await?
                    {
                        summary.new_materials += 1;
                    }
                }
            }

            // Actualizar marca de último escaneo del curso
            CoursesRepository::touch_scanned(&self.pool, course.id).await?;

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

    #[tokio::test]
    async fn test_scanner_silent_baseline_seeding() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // Responder tareas
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=mod_assign_get_assignments",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "courses": [{
                    "id": 500,
                    "assignments": [
                        {
                            "id": 5001,
                            "cmid": 6001,
                            "course": 500,
                            "name": "Tarea Base 1",
                            "duedate": 1800000000
                        },
                        {
                            "id": 5002,
                            "cmid": 6002,
                            "course": 500,
                            "name": "Tarea Base 2",
                            "duedate": 1800000000
                        }
                    ]
                }]
            })))
            .mount(&server)
            .await;

        // Responder contenidos
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=core_course_get_contents",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 7001,
                    "name": "Tema 1",
                    "modules": [
                        {
                            "id": 8001,
                            "name": "Syllabus PDF",
                            "modname": "resource",
                            "url": "https://moodle/syllabus.pdf"
                        }
                    ]
                }
            ])))
            .mount(&server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_scanner_baseline_{rand_id}.db"));
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

        // Insertar curso 500 (nace con last_scanned = NULL)
        CoursesRepository::upsert_course(&pool, 500, "Curso Línea Base", Some("BASE"))
            .await
            .unwrap();
        let c_init = CoursesRepository::get(&pool, 500).await.unwrap().unwrap();
        assert!(c_init.last_scanned.is_none());

        // Insertar usuario matriculado
        UsersRepository::upsert_login(&pool, 555, "24010555", 55, "Alumno Base", "token_555")
            .await
            .unwrap();
        CoursesRepository::link_user(&pool, 555, 500).await.unwrap();

        let worker = ScannerWorker::new(pool.clone(), moodle, bot, config);

        // Ciclo 1: Siembra silenciosa
        let summary1 = worker.run_scan_cycle().await.unwrap();
        assert_eq!(summary1.courses_scanned, 1);
        assert_eq!(summary1.new_assignments, 2);
        assert_eq!(summary1.new_materials, 1);
        assert_eq!(summary1.alerts_dispatched, 0);

        // Verificar que touch_scanned actualizó last_scanned
        let c_after = CoursesRepository::get(&pool, 500).await.unwrap().unwrap();
        assert!(c_after.last_scanned.is_some());

        // Ciclo 2: Segundo ciclo con los mismos datos no debe registrar nada nuevo ni generar alertas
        let summary2 = worker.run_scan_cycle().await.unwrap();
        assert_eq!(summary2.courses_scanned, 1);
        assert_eq!(summary2.new_assignments, 0);
        assert_eq!(summary2.new_materials, 0);
        assert_eq!(summary2.alerts_dispatched, 0);
    }

    #[tokio::test]
    async fn test_scanner_new_assignment_alert_with_visibility_check() {
        use crate::bot::create_bot_with_url;
        use crate::storage::AlertsRepository;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let moodle_server = MockServer::start().await;
        let telegram_server = MockServer::start().await;

        // Mock Telegram SendMessage
        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 99,
                    "date": 1700000000,
                    "chat": { "id": 777, "type": "private" },
                    "text": "alerta"
                }
            })))
            .mount(&telegram_server)
            .await;

        // Mock Moodle mod_assign_get_assignments devolviendo tarea nueva 6001
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=mod_assign_get_assignments",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "courses": [{
                    "id": 600,
                    "assignments": [{
                        "id": 6001,
                        "cmid": 7001,
                        "course": 600,
                        "name": "Tarea Diferencial",
                        "duedate": 1800000000
                    }]
                }]
            })))
            .mount(&moodle_server)
            .await;

        // Mock Moodle fetch_user_submission confirmando visibilidad y devolviendo due_override (A-07)
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=mod_assign_get_submission_status",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "lastattempt": {
                    "submission": {
                        "id": 8881,
                        "status": "new"
                    },
                    "extensionduedate": 1800050000
                }
            })))
            .mount(&moodle_server)
            .await;

        // Mock core_course_get_contents vacío
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=core_course_get_contents",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&moodle_server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_scanner_new_assign_{rand_id}.db"));
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
        let bot = create_bot_with_url(
            &config.bot_token,
            reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
        );
        let moodle = MoodleClient::new(moodle_server.uri());

        // Insertar curso 600 y marcarlo previamente como escaneado para establecer línea base
        CoursesRepository::upsert_course(&pool, 600, "Cálculo II", Some("CALC2"))
            .await
            .unwrap();
        CoursesRepository::touch_scanned(&pool, 600).await.unwrap();

        // Insertar usuario 777
        UsersRepository::upsert_login(&pool, 777, "24010777", 77, "Estudiante 7", "token_777")
            .await
            .unwrap();
        CoursesRepository::link_user(&pool, 777, 600).await.unwrap();

        let worker = ScannerWorker::new(pool.clone(), moodle, bot, config);

        // Ciclo 1: detecta tarea nueva, confirma visibilidad con token del alumno y despacha alerta
        let summary1 = worker.run_scan_cycle().await.unwrap();
        assert_eq!(summary1.courses_scanned, 1);
        assert_eq!(summary1.new_assignments, 1);
        assert_eq!(summary1.alerts_dispatched, 1);

        // Comprobar que la alerta quedó reclamada en sent_alerts
        let claimed = AlertsRepository::is_claimed(&pool, "new_assign:777:6001")
            .await
            .unwrap();
        assert!(claimed);

        // Comprobar que due_override fue persistido en user_submissions (A-07)
        let sub = SubmissionsRepository::get(&pool, 777, 6001)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sub.due_override, Some(1800050000));

        // Ciclo 2: Segundo escaneo no duplica alerta (RF-09 idempotencia)
        let summary2 = worker.run_scan_cycle().await.unwrap();
        assert_eq!(summary2.new_assignments, 0);
        assert_eq!(summary2.alerts_dispatched, 0);
    }
}
