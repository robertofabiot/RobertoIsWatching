//! Pruebas de cableado e integración del ensamblado de la aplicación (PR-A, Prompt A4).
//! Verifican que el scanner y el deadline worker realmente se ejecuten desde el ensamblado
//! y que respondan a la señal de apagado ordenado.

use robertos_watching_bot::app;
use robertos_watching_bot::bot::create_bot_with_url;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::moodle::MoodleClient;
use robertos_watching_bot::scheduler::deadlines::DeadlineJob;
use robertos_watching_bot::storage::{
    AlertsRepository, AssignmentsRepository, CoursesRepository, UsersRepository,
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

/// Test 1: Verifica que `spawn_workers` realmente arranca el ScannerWorker
/// y este ejecuta al menos una consulta HTTP contra Moodle.
#[tokio::test]
async fn spawn_workers_runs_a_scan_cycle() {
    let moodle_server = MockServer::start().await;
    let telegram_server = MockServer::start().await;

    // Mock de Moodle para mod_assign_get_assignments
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=mod_assign_get_assignments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "courses": [{
                "id": 101,
                "assignments": []
            }]
        })))
        .mount(&moodle_server)
        .await;

    // Mock de Moodle para core_course_get_contents
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=core_course_get_contents",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&moodle_server)
        .await;

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_app_wiring_scanner_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let config = Arc::new(create_test_config(database_url));

    let pool = robertos_watching_bot::storage::init(&config).await.unwrap();
    let bot = create_bot_with_url(
        &config.bot_token,
        reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
    );
    let moodle = MoodleClient::new(moodle_server.uri());

    // Sembrar un curso y un usuario donante
    CoursesRepository::upsert_course(&pool, 101, "Curso Test", Some("CT"))
        .await
        .unwrap();
    UsersRepository::upsert_login(
        &pool,
        1001,
        "24011001",
        101,
        "Alumno Donante",
        "token_donante",
    )
    .await
    .unwrap();
    CoursesRepository::link_user(&pool, 1001, 101)
        .await
        .unwrap();

    let mut app = app::build(pool, bot, moodle, config);

    // Ajustar retardos cortos para ejecución casi inmediata
    app.scanner = app
        .scanner
        .with_delays(Duration::from_millis(50), Duration::from_millis(1));

    let running = app::spawn_workers(app.scanner, app.deadline_worker, app.summary_worker);

    // Esperar a que el scanner realice al menos una petición a Moodle
    let mut requests_received = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Some(requests) = moodle_server.received_requests().await {
            if !requests.is_empty() {
                requests_received = true;
                break;
            }
        }
    }

    assert!(
        requests_received,
        "Moodle debió recibir peticiones de escaneo desde el ScannerWorker ensamblado"
    );

    // Apagado ordenado
    let clean = running.shutdown_and_wait(Duration::from_secs(2)).await;
    assert!(clean, "El apagado de workers debió completarse limpiamente");
}

/// Test 2: Verifica que el `DeadlineWorker` consume y procesa comandos
/// enviados a través del `DeadlineScheduler` ensamblado.
#[tokio::test]
async fn deadline_worker_consumes_commands() {
    let telegram_server = MockServer::start().await;

    // Mock de Telegram SendMessage
    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 999,
                "date": 1700000000,
                "chat": { "id": 1002, "type": "private" },
                "text": "alerta deadline 2h"
            }
        })))
        .mount(&telegram_server)
        .await;

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_app_wiring_deadlines_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let config = Arc::new(create_test_config(database_url));

    let pool = robertos_watching_bot::storage::init(&config).await.unwrap();
    let bot = create_bot_with_url(
        &config.bot_token,
        reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
    );
    let moodle = MoodleClient::new("http://127.0.0.1:8080");

    // Sembrar alumno activo y tarea que vence en 1 hora (dentro de ventana de 2h para disparo inmediato)
    CoursesRepository::upsert_course(&pool, 201, "Curso Deadlines", Some("CD"))
        .await
        .unwrap();
    UsersRepository::upsert_login(&pool, 1002, "24011002", 102, "Alumno Test", "tok_test")
        .await
        .unwrap();

    let now_epoch = chrono::Utc::now().timestamp();
    let due_in_1h = now_epoch + 3600; // vence en 1 hora (< 2h)
    AssignmentsRepository::upsert(
        &pool,
        5001,
        Some(50010),
        201,
        "Tarea Urgente",
        Some(due_in_1h),
        None,
    )
    .await
    .unwrap();

    let app = app::build(pool.clone(), bot, moodle, config);
    let scheduler = app.scheduler.clone();

    let running = app::spawn_workers(app.scanner, app.deadline_worker, app.summary_worker);

    // Encolar comando vía DeadlineScheduler
    let alert_key = "deadline:1002:5001:2h";
    scheduler
        .schedule(DeadlineJob {
            chat_id: 1002,
            assignment_id: 5001,
            effective_due: due_in_1h,
            version: 0,
        })
        .await
        .expect("Fallo al enviar comando al DeadlineScheduler");

    // Esperar a que el DeadlineWorker consuma el comando y registre el claim en SQLite
    let mut consumed_and_claimed = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(claimed) = AlertsRepository::is_claimed(&pool, alert_key).await {
            if claimed {
                consumed_and_claimed = true;
                break;
            }
        }
    }

    assert!(
        consumed_and_claimed,
        "DeadlineWorker debió consumir el comando del scheduler y registrar la alerta en BD"
    );

    let clean = running.shutdown_and_wait(Duration::from_secs(2)).await;
    assert!(clean, "El apagado de workers debió completarse limpiamente");
}

/// Test 3: Verifica que el envío de `shutdown_tx.send(true)` detiene todos los workers en < 2 s.
#[tokio::test]
async fn shutdown_stops_all_workers() {
    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_app_wiring_shutdown_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let config = Arc::new(create_test_config(database_url));

    let pool = robertos_watching_bot::storage::init(&config).await.unwrap();
    let bot = robertos_watching_bot::bot::create_bot(&config.bot_token);
    let moodle = MoodleClient::new("http://127.0.0.1:8080");

    let app = app::build(pool, bot, moodle, config);
    let running = app::spawn_workers(app.scanner, app.deadline_worker, app.summary_worker);

    // Enviar señal de apagado
    running
        .shutdown_tx
        .send(true)
        .expect("Fallo al enviar shutdown");

    // Afirmar que todos los JoinHandle terminan en < 2 segundos
    let scanner_res = tokio::time::timeout(Duration::from_secs(2), running.scanner_handle).await;
    assert!(
        scanner_res.is_ok(),
        "scanner_handle debió terminar en < 2 s"
    );

    let deadline_res = tokio::time::timeout(Duration::from_secs(2), running.deadline_handle).await;
    assert!(
        deadline_res.is_ok(),
        "deadline_handle debió terminar en < 2 s"
    );

    let summary_res = tokio::time::timeout(Duration::from_secs(2), running.summary_handle).await;
    assert!(
        summary_res.is_ok(),
        "summary_handle debió terminar en < 2 s"
    );
}
