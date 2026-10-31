//! Tests de integración para alertas de plazos (24h y 2h) y temporizadores min-heap con reloj pausado de Tokio (PR #7, A-16).

use chrono::Utc;
use robertos_watching_bot::bot::create_bot_with_url;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::scheduler::deadlines::DeadlineScheduler;
use robertos_watching_bot::storage::submissions::SubmissionData;
use robertos_watching_bot::storage::{
    AlertsRepository, AssignmentsRepository, CoursesRepository, SubmissionsRepository,
    UsersRepository,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn setup_test_env(
    test_name: &str,
) -> (
    MockServer,
    sqlx::SqlitePool,
    Arc<Config>,
    robertos_watching_bot::bot::Bot,
) {
    let telegram_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 777,
                "date": 1700000000,
                "chat": { "id": 1001, "type": "private" },
                "text": "alerta"
            }
        })))
        .mount(&telegram_server)
        .await;

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("{test_name}_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = Arc::new(Config {
        bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
        admin_chat_id: 123456789,
        database_url,
        max_users_default: 300,
        bot_tz: chrono_tz::America::Managua,
        rust_log: "info".to_string(),
        scan_interval_min: 30,
    });

    let pool = robertos_watching_bot::storage::init(&config).await.unwrap();
    let bot = create_bot_with_url(
        &config.bot_token,
        reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
    );

    // Sembrar curso y usuario base
    CoursesRepository::upsert_course(&pool, 100, "Arquitectura de Software", Some("ARQ"))
        .await
        .unwrap();
    UsersRepository::upsert_login(
        &pool,
        1001,
        "24011001",
        101,
        "Roberto Estudiante",
        "tok_1001",
    )
    .await
    .unwrap();
    CoursesRepository::link_user(&pool, 1001, 100)
        .await
        .unwrap();

    (telegram_server, pool, config, bot)
}

#[tokio::test]
async fn test_deadline_timer_fires_at_exact_due_minus_2h() {
    let (telegram_server, pool, config, bot) = setup_test_env("test_timer_exact_2h").await;

    let now_ts = Utc::now().timestamp();
    let due_ts = now_ts + 3 * 3600; // Vence en 3 horas (el aviso de 2h debe disparar a la hora 1 = 3600 s)

    AssignmentsRepository::upsert(
        &pool,
        1001,
        Some(10010),
        100,
        "Ensayo de Patrones",
        Some(due_ts),
        None,
    )
    .await
    .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (_scheduler, mut worker) = DeadlineScheduler::new(pool.clone(), bot, config);

    // Reconstruir min-heap con tiempo normal (boot lookahead agendará 1001)
    let summary = worker.boot_lookahead().await.unwrap();
    assert_eq!(summary.jobs_scheduled, 1);

    // Pausar reloj virtual de Tokio para probar el temporizador exacto
    tokio::time::pause();

    let worker_handle = tokio::spawn(async move {
        worker.run_loop(shutdown_rx).await;
    });

    // Ceder ejecución para que el worker loop entre a sleep_until
    tokio::task::yield_now().await;

    // 1. Avanzar el reloj virtual a 59 minutos (3540 s). Faltan 60 s para due - 2h.
    tokio::time::advance(Duration::from_secs(3540)).await;
    tokio::task::yield_now().await;

    // Reanudar momentáneamente para verificar BD sin interferencia del reloj pausado
    tokio::time::resume();
    let claimed_early = AlertsRepository::is_claimed(&pool, "deadline:1001:1001:2h")
        .await
        .unwrap();
    assert!(
        !claimed_early,
        "La alerta de 2h NO debe disparar antes de due - 2h"
    );
    assert_eq!(telegram_server.received_requests().await.unwrap().len(), 0);

    // 2. Pausar nuevamente y avanzar 120 segundos (cruzando los 3600 s = exactos due - 2h)
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(120)).await;
    tokio::task::yield_now().await;

    // Reanudar para que el worker ejecute la verificación en SQLite y envíe a Telegram
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let claimed_exact = AlertsRepository::is_claimed(&pool, "deadline:1001:1001:2h")
        .await
        .unwrap();
    assert!(
        claimed_exact,
        "La alerta de 2h DEBE disparar al cumplirse due - 2h"
    );

    let requests = telegram_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("ROBERTO'S WATCHING YOU PANIC (Quedan 2h)"));
    assert!(body.contains("Ensayo de Patrones"));

    shutdown_tx.send(true).unwrap();
    let _ = worker_handle.await;
}

#[tokio::test]
async fn test_deadline_timer_does_not_fire_if_submitted_before_2h() {
    let (telegram_server, pool, config, bot) = setup_test_env("test_timer_submitted").await;

    let now_ts = Utc::now().timestamp();
    let due_ts = now_ts + 3 * 3600;

    AssignmentsRepository::upsert(
        &pool,
        1002,
        Some(10020),
        100,
        "Laboratorio de Concurrencia",
        Some(due_ts),
        None,
    )
    .await
    .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (scheduler, mut worker) = DeadlineScheduler::new(pool.clone(), bot, config);

    // Boot lookahead antes de pausar el reloj
    worker.boot_lookahead().await.unwrap();

    tokio::time::pause();

    let worker_handle = tokio::spawn(async move {
        worker.run_loop(shutdown_rx).await;
    });

    tokio::task::yield_now().await;

    // A los 30 minutos (1800 s), el alumno entrega la tarea
    tokio::time::advance(Duration::from_secs(1800)).await;
    tokio::task::yield_now().await;

    // Reanudar para registrar la entrega en SQLite y cancelar en el scheduler
    tokio::time::resume();
    SubmissionsRepository::upsert(
        &pool,
        SubmissionData {
            chat_id: 1001,
            assignment_id: 1002,
            status: Some("submitted"),
            graded: false,
            grade: None,
            feedback: None,
            due_override: None,
        },
    )
    .await
    .unwrap();

    // Cancelar en el scheduler
    scheduler.cancel_job(1001, 1002).await.unwrap();

    // Pausar y avanzar reloj virtual pasando el umbral de due - 2h (3600 s)
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3600)).await;
    tokio::task::yield_now().await;

    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let claimed = AlertsRepository::is_claimed(&pool, "deadline:1001:1002:2h")
        .await
        .unwrap();
    assert!(
        !claimed,
        "No debe dispararse alerta si la tarea ya fue entregada"
    );
    assert_eq!(telegram_server.received_requests().await.unwrap().len(), 0);

    shutdown_tx.send(true).unwrap();
    let _ = worker_handle.await;
}

#[tokio::test]
async fn test_deadline_timer_reschedules_when_deadline_moves() {
    let (telegram_server, pool, config, bot) = setup_test_env("test_timer_reschedule").await;

    let now_ts = Utc::now().timestamp();
    let initial_due = now_ts + 3 * 3600; // Inicialmente vence en 3h (disparo a la 1h = 3600 s)

    AssignmentsRepository::upsert(
        &pool,
        1003,
        Some(10030),
        100,
        "Proyecto Final",
        Some(initial_due),
        None,
    )
    .await
    .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (scheduler, mut worker) = DeadlineScheduler::new(pool.clone(), bot, config);

    // Boot lookahead antes de pausar reloj
    worker.boot_lookahead().await.unwrap();

    tokio::time::pause();

    let worker_handle = tokio::spawn(async move {
        worker.run_loop(shutdown_rx).await;
    });

    tokio::task::yield_now().await;

    // A los 30 min (1800 s), el docente extiende el plazo 2 horas más (now + 5h)
    tokio::time::advance(Duration::from_secs(1800)).await;
    tokio::task::yield_now().await;

    // Reanudar reloj para actualizar BD y reprogramar
    tokio::time::resume();
    let extended_due = now_ts + 5 * 3600; // Nueva fecha: vence en 5h (nuevo disparo a las 3h = 10800 s)
    AssignmentsRepository::upsert(
        &pool,
        1003,
        Some(10030),
        100,
        "Proyecto Final",
        Some(extended_due),
        None,
    )
    .await
    .unwrap();

    scheduler
        .reschedule(1001, 1003, extended_due)
        .await
        .unwrap();

    // Pausar y avanzar reloj virtual a 1 hora y 10 minutos (4200 s). Pasó la hora inicial (3600 s) pero NO la nueva (10800 s).
    tokio::time::pause();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2400)).await;
    tokio::task::yield_now().await;

    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let claimed_early = AlertsRepository::is_claimed(&pool, "deadline:1001:1003:2h")
        .await
        .unwrap();
    assert!(
        !claimed_early,
        "La alerta vieja a 1h no debió disparar tras la reprogramación"
    );
    assert_eq!(telegram_server.received_requests().await.unwrap().len(), 0);

    // Avanzar al nuevo vencimiento de 2h (3 horas desde inicio = 10800 s total)
    // Ya avanzamos 1800 + 2400 = 4200 s. Faltan 6600 + 120 = 6720 s
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(6720)).await;
    tokio::task::yield_now().await;

    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let claimed_extended = AlertsRepository::is_claimed(&pool, "deadline:1001:1003:2h")
        .await
        .unwrap();
    assert!(
        claimed_extended,
        "La alerta reprogramada DEBE disparar a due_nuevo - 2h"
    );
    assert_eq!(telegram_server.received_requests().await.unwrap().len(), 1);

    shutdown_tx.send(true).unwrap();
    let _ = worker_handle.await;
}

#[tokio::test]
async fn test_simulated_restart_boot_lookahead_no_duplicate_or_omission() {
    let (telegram_server, pool, config, bot) = setup_test_env("test_timer_restart").await;

    let now_ts = Utc::now().timestamp();

    // Tarea A: Vence en 6 horas (fuera de ventana 2h)
    AssignmentsRepository::upsert(
        &pool,
        2001,
        Some(20010),
        100,
        "Tarea Futura 6h",
        Some(now_ts + 6 * 3600),
        None,
    )
    .await
    .unwrap();

    // Tarea B: Vence en 1 hora (dentro de ventana 2h, aún no enviada)
    AssignmentsRepository::upsert(
        &pool,
        2002,
        Some(20020),
        100,
        "Tarea Inminente 1h",
        Some(now_ts + 3600),
        None,
    )
    .await
    .unwrap();

    // Tarea C: Vence en 1 hora pero YA FUE ENVIADA previamente
    AssignmentsRepository::upsert(
        &pool,
        2003,
        Some(20030),
        100,
        "Tarea Enviada 1h",
        Some(now_ts + 3600),
        None,
    )
    .await
    .unwrap();
    AlertsRepository::try_claim(&pool, "deadline:1001:2003:2h", "deadline_2h", Some(1001))
        .await
        .unwrap();

    // Tarea D: Vencida hace 2 horas (-2h)
    AssignmentsRepository::upsert(
        &pool,
        2004,
        Some(20040),
        100,
        "Tarea Vencida -2h",
        Some(now_ts - 7200),
        None,
    )
    .await
    .unwrap();

    // Primer arranque: el worker reconstruye el min-heap desde SQLite
    let (_scheduler_1, mut worker_1) =
        DeadlineScheduler::new(pool.clone(), bot.clone(), config.clone());
    let summary_1 = worker_1.boot_lookahead().await.unwrap();

    assert_eq!(
        summary_1.jobs_scheduled, 1,
        "Debe agendar solo 2001 en heap"
    );
    assert_eq!(
        summary_1.alerts_dispatched, 1,
        "Debe disparar 2002 (inminente no enviada)"
    );
    assert_eq!(
        summary_1.already_claimed_skipped, 1,
        "Debe omitir 2003 (ya enviada)"
    );
    assert_eq!(
        summary_1.expired_discarded, 1,
        "Debe descartar 2004 (vencida)"
    );

    // Telegram recibió exactamente 1 mensaje (por la tarea 2002)
    let requests = telegram_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("Tarea Inminente 1h"));

    // Segundo arranque simulado inmediatamente (reinicio del proceso):
    let (_scheduler_2, mut worker_2) = DeadlineScheduler::new(pool.clone(), bot, config);
    let summary_2 = worker_2.boot_lookahead().await.unwrap();

    // 2002 ya está reclamada en sent_alerts tras el primer arranque
    assert_eq!(
        summary_2.jobs_scheduled, 1,
        "Debe seguir agendando 2001 en heap"
    );
    assert_eq!(
        summary_2.alerts_dispatched, 0,
        "No debe reenviar ninguna alerta (cero duplicados)"
    );
    assert_eq!(
        summary_2.already_claimed_skipped, 2,
        "2002 y 2003 están reclamadas"
    );
    assert_eq!(summary_2.expired_discarded, 1, "2004 sigue vencida");

    // Telegram no recibió mensajes adicionales
    assert_eq!(telegram_server.received_requests().await.unwrap().len(), 1);
}
