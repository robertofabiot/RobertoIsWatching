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
        admin_chat_id: 1125347763,
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
