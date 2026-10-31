//! Pruebas de integración del Shared Scanner, Línea Base, Failover, Detección de Diffs y Niveles (PR #6).

use robertos_watching_bot::bot::create_bot_with_url;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::moodle::MoodleClient;
use robertos_watching_bot::services::scanner::ScannerWorker;
use robertos_watching_bot::storage::{
    AssignmentsRepository, CoursesRepository, SubmissionsRepository, UsersRepository,
};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn create_test_config(database_url: String) -> Config {
    Config {
        bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
        admin_chat_id: 123456789,
        database_url,
        max_users_default: 300,
        bot_tz: chrono_tz::America::Managua,
        rust_log: "info".to_string(),
        scan_interval_min: 30,
    }
}

#[tokio::test]
async fn test_scanner_e2e_failover_baseline_diff_and_tiers() {
    let moodle_server = MockServer::start().await;
    let telegram_server = MockServer::start().await;

    // 1. Mock de Telegram SendMessage
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 1,
                "date": 1700000000,
                "chat": { "id": 2002, "type": "private" },
                "text": "alerta"
            }
        })))
        .mount(&telegram_server)
        .await;

    // 2. Mock de Moodle: token 1 falla con invalidtoken
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wstoken=bad_token_1001",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "exception": "moodle_exception",
            "errorcode": "invalidtoken",
            "message": "Token inválido o expirado"
        })))
        .mount(&moodle_server)
        .await;

    // 3. Mock de Moodle: mod_assign_get_assignments con token 2
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wstoken=good_token_2002",
        ))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=mod_assign_get_assignments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "courses": [{
                "id": 301,
                "assignments": [
                    {
                        "id": 501,
                        "cmid": 601,
                        "course": 301,
                        "name": "Proyecto Fase 1",
                        "duedate": 1800000000
                    }
                ]
            }]
        })))
        .mount(&moodle_server)
        .await;

    // 4. Mock de Moodle: core_course_get_contents con token 2
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wstoken=good_token_2002",
        ))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=core_course_get_contents",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": 701,
                "name": "Unidad 1",
                "modules": [
                    {
                        "id": 801,
                        "name": "Introducción.pdf",
                        "modname": "resource",
                        "url": "https://moodle/intro.pdf"
                    }
                ]
            }
        ])))
        .mount(&moodle_server)
        .await;

    // 5. Mock de Moodle: fetch_user_submission para alumno 2002
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=mod_assign_get_submission_status",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "lastattempt": {
                "submission": {
                    "id": 901,
                    "status": "new"
                }
            }
        })))
        .mount(&moodle_server)
        .await;

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_scanner_e2e_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let config = Arc::new(create_test_config(database_url));

    let pool = robertos_watching_bot::storage::init(&config).await.unwrap();
    let bot = create_bot_with_url(
        &config.bot_token,
        reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
    );
    let moodle = MoodleClient::new(moodle_server.uri());

    // Crear curso 301 (last_scanned = NULL)
    CoursesRepository::upsert_course(&pool, 301, "Sistemas Operativos", Some("SO"))
        .await
        .unwrap();

    // Crear alumno 1001 (con token malo)
    UsersRepository::upsert_login(
        &pool,
        1001,
        "24011001",
        1001,
        "Alumno Fallo",
        "bad_token_1001",
    )
    .await
    .unwrap();
    CoursesRepository::link_user(&pool, 1001, 301)
        .await
        .unwrap();

    // Crear alumno 2002 (con token bueno)
    UsersRepository::upsert_login(
        &pool,
        2002,
        "24012002",
        2002,
        "Alumno Exito",
        "good_token_2002",
    )
    .await
    .unwrap();
    CoursesRepository::link_user(&pool, 2002, 301)
        .await
        .unwrap();

    let worker = ScannerWorker::new(pool.clone(), moodle, bot, config)
        .with_delays(Duration::from_millis(50), Duration::from_millis(10));

    // ==========================================
    // FASE 1: Primer ciclo - Siembra Silenciosa (A-03) y Failover de Donantes (A-22)
    // ==========================================
    let summary1 = worker.run_scan_cycle_with_count(1).await.unwrap();

    // El donante 1001 debió fallar y ser expirado automáticamente
    let u1001 = UsersRepository::get(&pool, 1001).await.unwrap().unwrap();
    assert!(!u1001.is_active);
    assert_eq!(u1001.inactive_reason.as_deref(), Some("token_expired"));

    // El donante 2002 continuó y el curso fue escaneado
    assert_eq!(summary1.courses_scanned, 1);
    assert_eq!(summary1.courses_failed, 0);
    assert_eq!(summary1.new_assignments, 1);
    assert_eq!(summary1.new_materials, 1);
    // CERO alertas: la primera pasada es siembra silenciosa (A-03)
    assert_eq!(summary1.alerts_dispatched, 0);
    assert_eq!(summary1.tokens_invalidated, 1);
    assert!(summary1.moodle_requests_background >= 1);
    assert!(summary1.duration > Duration::ZERO);

    // last_scanned del curso ahora está establecido
    let c301 = CoursesRepository::get(&pool, 301).await.unwrap().unwrap();
    assert!(c301.last_scanned.is_some());

    // ==========================================
    // FASE 2: Segundo ciclo con los mismos datos -> 0 Novedades, 0 Alertas (Idempotencia)
    // ==========================================
    let summary2 = worker.run_scan_cycle_with_count(2).await.unwrap();
    assert_eq!(summary2.courses_scanned, 1);
    assert_eq!(summary2.new_assignments, 0);
    assert_eq!(summary2.new_materials, 0);
    assert_eq!(summary2.alerts_dispatched, 0);
    assert_eq!(summary2.alerts_new_assignment, 0);
    assert_eq!(summary2.alerts_new_material, 0);
    assert_eq!(summary2.alerts_grade, 0);
    assert_eq!(summary2.alerts_deadline_24h, 0);
    assert_eq!(summary2.tokens_invalidated, 0);
    assert_eq!(summary2.polled_hot, 0);
    assert_eq!(summary2.users_polled_hot, 0);
    assert_eq!(summary2.polled_warm, 0);
    assert_eq!(summary2.users_polled_warm, 0);
    assert_eq!(summary2.polled_cold, 0);
    assert_eq!(summary2.users_polled_cold, 0);
    assert!(summary2.moodle_requests_background >= 1);
    assert!(summary2.duration > Duration::ZERO);

    // ==========================================
    // FASE 3: Detección de Novedades (RF-03) tras tener línea base previa
    // ==========================================
    // Tarea 501 ya existe en SQLite; agregamos una segunda tarea 502 directamente en SQLite
    // simulando una nueva tarea detectada en un escaneo con diff
    let due_tomorrow = chrono::Utc::now().timestamp() + 86400;
    AssignmentsRepository::upsert(
        &pool,
        502,
        Some(602),
        301,
        "Examen Parcial",
        Some(due_tomorrow),
        None,
    )
    .await
    .unwrap();

    // Verificamos que la tarea 502 es Hot (< 72h) y al sondear se inspecciona
    let sub502 = SubmissionsRepository::get(&pool, 2002, 502).await.unwrap();
    assert!(sub502.is_none());

    // Al correr ciclo 3, se sondea la tarea Hot 502
    let summary3 = worker.run_scan_cycle_with_count(3).await.unwrap();
    assert!(summary3.submissions_checked >= 1);
    assert_eq!(summary3.polled_hot, 1);
    assert_eq!(summary3.users_polled_hot, 1);
    assert_eq!(summary3.polled_warm, 0);
    assert_eq!(summary3.users_polled_warm, 0);
    assert_eq!(summary3.polled_cold, 0);
    assert_eq!(summary3.users_polled_cold, 0);
    assert_eq!(summary3.alerts_dispatched, 1);
    assert_eq!(summary3.alerts_new_assignment, 0);
    assert_eq!(summary3.alerts_new_material, 0);
    assert_eq!(summary3.alerts_grade, 0);
    assert_eq!(summary3.alerts_deadline_24h, 1);
    assert_eq!(summary3.tokens_invalidated, 0);
    assert!(summary3.moodle_requests_background >= 1);
    assert!(summary3.duration > Duration::ZERO);

    // La entrega de la tarea 502 fue persistida
    let sub502_after = SubmissionsRepository::get(&pool, 2002, 502).await.unwrap();
    assert!(sub502_after.is_some());
}

#[tokio::test]
async fn test_sync_does_not_silence_scanner_alerts_for_other_students() {
    let moodle_server = MockServer::start().await;
    let telegram_server = MockServer::start().await;

    // 1. Mock de Telegram SendMessage
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 99,
                "date": 1700000000,
                "chat": { "id": 3002, "type": "private" },
                "text": "alerta"
            }
        })))
        .mount(&telegram_server)
        .await;

    // 2. Mocks iniciales de Moodle
    let courses_json = serde_json::json!([
        {
            "id": 701,
            "shortname": "PROG701",
            "fullname": "Programación Concurrente",
            "displayname": "Programación Concurrente",
            "visible": 1,
            "enddate": 0,
            "completed": false,
            "hidden": false
        }
    ]);

    // Inicialmente no hay tareas ni módulos
    let empty_assignments = serde_json::json!({
        "courses": [{
            "id": 701,
            "assignments": []
        }]
    });

    let new_assignment_json = serde_json::json!({
        "courses": [{
            "id": 701,
            "assignments": [
                {
                    "id": 801,
                    "cmid": 8001,
                    "course": 701,
                    "name": "Proyecto Final Rust",
                    "duedate": chrono::Utc::now().timestamp() + 86400 * 5,
                    "allowsubmissionsfromdate": 0,
                    "cutoffdate": 0,
                    "timemodified": 0
                }
            ]
        }]
    });

    let submission_json = serde_json::json!({
        "lastattempt": {
            "submission": { "status": "new" },
            "graded": false,
            "gradingstatus": "notgraded"
        },
        "feedback": null
    });

    let empty_contents = serde_json::json!([]);

    let has_new_assign = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let has_new_assign_c = has_new_assign.clone();

    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(move |req: &wiremock::Request| {
            let body_str = std::str::from_utf8(&req.body).unwrap_or("");
            if body_str.contains("core_enrol_get_users_courses") {
                ResponseTemplate::new(200).set_body_json(&courses_json)
            } else if body_str.contains("mod_assign_get_assignments") {
                if has_new_assign_c.load(std::sync::atomic::Ordering::SeqCst) {
                    ResponseTemplate::new(200).set_body_json(&new_assignment_json)
                } else {
                    ResponseTemplate::new(200).set_body_json(&empty_assignments)
                }
            } else if body_str.contains("mod_assign_get_submission_status") {
                ResponseTemplate::new(200).set_body_json(&submission_json)
            } else if body_str.contains("core_course_get_contents") {
                ResponseTemplate::new(200).set_body_json(&empty_contents)
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({}))
            }
        })
        .mount(&moodle_server)
        .await;

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_r02_shared_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = Arc::new(create_test_config(database_url));
    let pool = robertos_watching_bot::storage::init(&config).await.unwrap();
    let bot = create_bot_with_url(
        &config.bot_token,
        reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
    );
    let moodle = MoodleClient::from_builder(
        robertos_watching_bot::moodle::MoodleClientBuilder::new()
            .base_url(moodle_server.uri())
            .gate(robertos_watching_bot::moodle::MoodleGate::new(4, 3).without_jitter())
            .max_retries(0),
    );

    // 3. Crear Curso 701 y alumnos A (3001) y B (3002) inscritos en el curso
    CoursesRepository::upsert_course(&pool, 701, "Programación Concurrente", Some("PROG701"))
        .await
        .unwrap();

    UsersRepository::upsert_login(&pool, 3001, "22013001", 301, "Alumno A", "tok_a")
        .await
        .unwrap();
    CoursesRepository::link_user(&pool, 3001, 701)
        .await
        .unwrap();

    UsersRepository::upsert_login(&pool, 3002, "22013002", 302, "Alumno B", "tok_b")
        .await
        .unwrap();
    CoursesRepository::link_user(&pool, 3002, 701)
        .await
        .unwrap();

    let worker = ScannerWorker::new(pool.clone(), moodle.clone(), bot, config.clone());

    // 4. Ciclo inicial del scanner: establece la línea base del curso 701
    let baseline_summary = worker.run_scan_cycle().await.unwrap();
    assert_eq!(baseline_summary.courses_scanned, 1);
    assert_eq!(baseline_summary.alerts_dispatched, 0);

    let course_record = CoursesRepository::get(&pool, 701).await.unwrap().unwrap();
    assert!(
        course_record.last_scanned.is_some(),
        "El ciclo del scanner debe establecer la línea base (last_scanned NOT NULL)"
    );

    // 5. El docente publica una nueva tarea en Moodle
    has_new_assign.store(true, std::sync::atomic::Ordering::SeqCst);

    // 6. Alumno A ejecuta /sync (refresh_user) antes de que corra el siguiente escaneo periódico
    let _sync_summary = robertos_watching_bot::services::sync::refresh_user(
        &pool,
        &moodle,
        3001,
        robertos_watching_bot::moodle::gate::Priority::Interactive,
    )
    .await
    .expect("Fallo al ejecutar /sync para Alumno A");

    // Con el fix R-02, refresh_user NO debe consumir silenciosamente la nueva tarea en la BD
    // para evitar silenciar la alerta de los demás compañeros del curso
    let assign_in_db = AssignmentsRepository::get(&pool, 801).await.unwrap();
    assert!(
        assign_in_db.is_none(),
        "R-02: /sync no debe haber insertado la nueva tarea en la tabla compartida"
    );

    // 7. Corre el siguiente ciclo del scanner periódico
    let scan_summary = worker.run_scan_cycle().await.unwrap();
    assert_eq!(scan_summary.courses_scanned, 1);
    assert_eq!(
        scan_summary.new_assignments, 1,
        "El scanner debe detectar la nueva tarea como Inserted"
    );
    assert!(
        scan_summary.alerts_dispatched >= 1,
        "El scanner debe haber despachado alertas de nueva tarea"
    );

    // 8. Verificar que el Alumno B recibió su alerta en Telegram (R-02)
    let alert_b_claimed =
        robertos_watching_bot::storage::AlertsRepository::is_claimed(&pool, "new_assign:3002:801")
            .await
            .unwrap();
    assert!(
        alert_b_claimed,
        "R-02: El Alumno B debió recibir la alerta de la nueva tarea a pesar de que Alumno A hizo /sync antes"
    );

    let _ = std::fs::remove_file(&db_path);
}
