//! Pruebas integrales de selección de horarios, idempotencia, política de reinicio
//! y apagado ordenado para el worker de resúmenes diarios (PR #8, A-18, ORIGINAL_REQUEST R6).

use chrono::{Datelike, TimeZone, Utc};
use chrono_tz::Tz;
use robertos_watching_bot::bot::create_bot_with_url;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::scheduler::summary::{duration_until_next_minute, SummaryWorker};
use robertos_watching_bot::storage::{
    AlertsRepository, AssignmentsRepository, CoursesRepository, UsersRepository,
};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct TestEnv {
    server: MockServer,
    pool: SqlitePool,
    config: Arc<Config>,
    worker: Option<SummaryWorker>,
    db_path: std::path::PathBuf,
}

impl TestEnv {
    fn worker(&self) -> &SummaryWorker {
        self.worker.as_ref().unwrap()
    }

    fn take_worker(&mut self) -> SummaryWorker {
        self.worker.take().unwrap()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.db_path);
        let _ = std::fs::remove_file(format!("{}-wal", self.db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.db_path.display()));
    }
}

async fn setup_test_env(test_name: &str) -> TestEnv {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 888,
                "date": 1700000000,
                "chat": { "id": 12345, "type": "private" },
                "text": "resumen"
            }
        })))
        .mount(&server)
        .await;

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_sum_{test_name}_{rand_id}.db"));
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

    let pool = robertos_watching_bot::storage::init(&config).await.unwrap();
    let bot = create_bot_with_url(
        &config.bot_token,
        reqwest::Url::parse(&format!("{}/", server.uri())).unwrap(),
    );

    let worker = SummaryWorker::new(pool.clone(), bot, config.clone());

    TestEnv {
        server,
        pool,
        config,
        worker: Some(worker),
        db_path,
    }
}

/// Test 1: Conversión de zona horaria `BOT_TZ=America/Managua` sin horario de verano (DST)
/// y cálculo de duración al límite de minuto exacto (:00.000).
#[test]
fn test_timezone_managua_no_dst_and_duration_calculation() {
    let tz: Tz = "America/Managua".parse().unwrap();

    // Invierno: Enero 15 (UTC-6)
    let winter_utc = Utc.with_ymd_and_hms(2026, 1, 15, 13, 30, 0).unwrap();
    let winter_local = winter_utc.with_timezone(&tz);
    assert_eq!(winter_local.format("%H:%M").to_string(), "07:30");
    assert_eq!(winter_local.format("%z").to_string(), "-0600");

    // Verano: Julio 15 (UTC-6 permanente, sin cambio por DST)
    let summer_utc = Utc.with_ymd_and_hms(2026, 7, 15, 13, 30, 0).unwrap();
    let summer_local = summer_utc.with_timezone(&tz);
    assert_eq!(summer_local.format("%H:%M").to_string(), "07:30");
    assert_eq!(summer_local.format("%z").to_string(), "-0600");

    // Medianoche y cambio de fecha (06:00 UTC = 00:00 Managua)
    let midnight_utc = Utc.with_ymd_and_hms(2026, 10, 31, 6, 0, 0).unwrap();
    let midnight_local = midnight_utc.with_timezone(&tz);
    assert_eq!(
        midnight_local.format("%Y-%m-%d %H:%M").to_string(),
        "2026-10-31 00:00"
    );

    // 02:00 UTC del día siguiente = 20:00 Managua del día anterior
    let evening_utc = Utc.with_ymd_and_hms(2026, 11, 1, 2, 0, 0).unwrap();
    let evening_local = evening_utc.with_timezone(&tz);
    assert_eq!(
        evening_local.format("%Y-%m-%d %H:%M").to_string(),
        "2026-10-31 20:00"
    );

    // Cálculo de duración al límite de minuto con margen de 20ms
    let at_15s = Utc.with_ymd_and_hms(2026, 10, 30, 10, 0, 15).unwrap();
    let dur = duration_until_next_minute(at_15s);
    assert_eq!(dur.as_millis(), 45020); // (60 - 15) * 1000 + 20

    let at_59s_900ms = Utc
        .with_ymd_and_hms(2026, 10, 30, 10, 0, 59)
        .unwrap()
        .checked_add_signed(chrono::Duration::milliseconds(900))
        .unwrap();
    let dur_near = duration_until_next_minute(at_59s_900ms);
    assert_eq!(dur_near.as_millis(), 120); // 100 + 20
}

/// Test 2: Selección de usuarios con resúmenes pendientes según hora HH:MM y día de la semana,
/// soportando domingo codificado como 7 o como 0, y tolerando JSON corrupto sin abortar.
#[tokio::test]
async fn test_schedule_selection_comprehensive() {
    let env = setup_test_env("sched_sel").await;

    // Alumno 1: Lun-Vie (1..5) a las 07:30 y 20:00
    let u1 =
        UsersRepository::upsert_login(&env.pool, 2001, "24012001", 1, "Estudiante LunVie", "tok_1")
            .await
            .unwrap();
    UsersRepository::update_summary_config(
        &env.pool,
        u1.chat_id,
        true,
        r#"["07:30", "20:00"]"#,
        r#"[1, 2, 3, 4, 5]"#,
    )
    .await
    .unwrap();

    // Alumno 2: Domingo codificado como 7 a las 07:30
    let u2 =
        UsersRepository::upsert_login(&env.pool, 2002, "24012002", 2, "Estudiante Dom7", "tok_2")
            .await
            .unwrap();
    UsersRepository::update_summary_config(&env.pool, u2.chat_id, true, r#"["07:30"]"#, r#"[7]"#)
        .await
        .unwrap();

    // Alumno 3: Domingo codificado como 0 a las 07:30
    let u3 =
        UsersRepository::upsert_login(&env.pool, 2003, "24012003", 3, "Estudiante Dom0", "tok_3")
            .await
            .unwrap();
    UsersRepository::update_summary_config(&env.pool, u3.chat_id, true, r#"["07:30"]"#, r#"[0]"#)
        .await
        .unwrap();

    // Alumno 4: Inactivo a las 07:30 en Lunes
    let u4 = UsersRepository::upsert_login(
        &env.pool,
        2004,
        "24012004",
        4,
        "Estudiante Inactivo",
        "tok_4",
    )
    .await
    .unwrap();
    UsersRepository::update_summary_config(&env.pool, u4.chat_id, true, r#"["07:30"]"#, r#"[1]"#)
        .await
        .unwrap();
    UsersRepository::mark_inactive(&env.pool, u4.chat_id, "user_blocked")
        .await
        .unwrap();

    // Alumno 5: Con resúmenes deshabilitados (summary_enabled = 0)
    let u5 =
        UsersRepository::upsert_login(&env.pool, 2005, "24012005", 5, "Estudiante Off", "tok_5")
            .await
            .unwrap();
    UsersRepository::update_summary_config(&env.pool, u5.chat_id, false, r#"["07:30"]"#, r#"[1]"#)
        .await
        .unwrap();

    // Alumno 6: Registro con JSON malformado
    let u6 = UsersRepository::upsert_login(
        &env.pool,
        2006,
        "24012006",
        6,
        "Estudiante Malformado",
        "tok_6",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE users SET summary_times = '{not: json}', summary_days = 'invalid' WHERE chat_id = ?")
        .bind(u6.chat_id)
        .execute(&env.pool)
        .await
        .unwrap();

    // Lunes (1) a las 07:30 -> debe retornar solo u1
    let due_mon = UsersRepository::users_due_for_summary(&env.pool, "07:30", 1)
        .await
        .unwrap();
    assert_eq!(due_mon.len(), 1);
    assert_eq!(due_mon[0].chat_id, 2001);

    // Lunes (1) a las 20:00 -> debe retornar solo u1
    let due_mon_eve = UsersRepository::users_due_for_summary(&env.pool, "20:00", 1)
        .await
        .unwrap();
    assert_eq!(due_mon_eve.len(), 1);
    assert_eq!(due_mon_eve[0].chat_id, 2001);

    // Lunes (1) a las 12:00 -> vacío
    let due_mon_none = UsersRepository::users_due_for_summary(&env.pool, "12:00", 1)
        .await
        .unwrap();
    assert!(due_mon_none.is_empty());

    // Domingo consultado con ISO 7 -> debe retornar u2 ([7]) y u3 ([0])
    let due_sun_7 = UsersRepository::users_due_for_summary(&env.pool, "07:30", 7)
        .await
        .unwrap();
    assert_eq!(due_sun_7.len(), 2);
    let ids7: Vec<i64> = due_sun_7.iter().map(|u| u.chat_id).collect();
    assert!(ids7.contains(&2002));
    assert!(ids7.contains(&2003));

    // Domingo consultado con 0 -> debe retornar u2 ([7]) y u3 ([0])
    let due_sun_0 = UsersRepository::users_due_for_summary(&env.pool, "07:30", 0)
        .await
        .unwrap();
    assert_eq!(due_sun_0.len(), 2);
    let ids0: Vec<i64> = due_sun_0.iter().map(|u| u.chat_id).collect();
    assert!(ids0.contains(&2002));
    assert!(ids0.contains(&2003));
}

/// Test 3: Deduplicación mediante clave de idempotencia `summary:{chat_id}:{YYYY-MM-DD}:{HH:MM}`.
/// Dos ejecuciones dentro del mismo minuto para el mismo usuario resultan en un único envío HTTP.
#[tokio::test]
async fn test_summary_idempotency_key_deduplication() {
    let env = setup_test_env("idem_key").await;

    let user = UsersRepository::upsert_login(
        &env.pool,
        3001,
        "24013001",
        1,
        "Alumno Deduplicación",
        "tok_dedup",
    )
    .await
    .unwrap();

    UsersRepository::update_summary_config(
        &env.pool,
        user.chat_id,
        true,
        r#"["08:00"]"#,
        r#"[1]"#, // Lunes
    )
    .await
    .unwrap();

    let dt = env
        .config
        .bot_tz
        .with_ymd_and_hms(2026, 11, 2, 8, 0, 0)
        .unwrap(); // Lunes 2026-11-02 08:00

    // Primer disparo para el minuto
    env.worker().evaluate_minute(&dt).await;

    let key = "summary:3001:2026-11-02:08:00";
    assert!(
        AlertsRepository::is_claimed(&env.pool, key).await.unwrap(),
        "La clave de idempotencia debió ser reclamada en sent_alerts"
    );

    // Segundo disparo para el mismo minuto
    env.worker().evaluate_minute(&dt).await;

    // Verificar que solo se ejecutó 1 llamada POST a Telegram
    let requests = env.server.received_requests().await.unwrap();
    let send_message_calls: Vec<_> = requests
        .into_iter()
        .filter(|req| req.url.path().to_lowercase().contains("sendmessage"))
        .collect();
    assert_eq!(
        send_message_calls.len(),
        1,
        "Telegram no debió recibir más de 1 mensaje por la clave de idempotencia"
    );
}

/// Test 4: Política de reinicio y lookback de 10 minutos (`[now - 10 min .. now]`).
/// Tras reiniciar el bot:
/// - Resumen debido hace 5 minutos (no enviado previamente) -> se despacha.
/// - Resumen debido hace 5 minutos (ya reclamado antes de la caída) -> no se duplica.
/// - Resumen debido hace 25 minutos (caída prolongada) -> se descarta para evitar aluviones.
#[tokio::test]
async fn test_restart_lookback_10min_window() {
    let env = setup_test_env("lookback_10m").await;

    let now_local = Utc::now().with_timezone(&env.config.bot_tz);
    let weekday_iso = now_local.weekday().number_from_monday();

    // 1. Alumno A: programado para hace 5 minutos (dentro de los 10 min). No enviado.
    let dt_5m_ago = now_local - chrono::Duration::minutes(5);
    let time_5m_ago = dt_5m_ago.format("%H:%M").to_string();
    let u_a = UsersRepository::upsert_login(
        &env.pool,
        4001,
        "24014001",
        1,
        "Alumno 5Min Missed",
        "tok_a",
    )
    .await
    .unwrap();
    UsersRepository::update_summary_config(
        &env.pool,
        u_a.chat_id,
        true,
        &format!(r#"["{time_5m_ago}"]"#),
        &format!(r#"[{weekday_iso}]"#),
    )
    .await
    .unwrap();

    // 2. Alumno B: programado para hace 5 minutos, pero ya fue enviado antes del reinicio
    let u_b = UsersRepository::upsert_login(
        &env.pool,
        4002,
        "24014002",
        2,
        "Alumno 5Min YaEnviado",
        "tok_b",
    )
    .await
    .unwrap();
    UsersRepository::update_summary_config(
        &env.pool,
        u_b.chat_id,
        true,
        &format!(r#"["{time_5m_ago}"]"#),
        &format!(r#"[{weekday_iso}]"#),
    )
    .await
    .unwrap();
    let key_b = format!(
        "summary:{}:{}:{}",
        u_b.chat_id,
        dt_5m_ago.format("%Y-%m-%d"),
        time_5m_ago
    );
    AlertsRepository::try_claim(&env.pool, &key_b, "summary", Some(u_b.chat_id))
        .await
        .unwrap();

    // 3. Alumno C: programado para hace 25 minutos (fuera de la ventana de 10 min)
    let dt_25m_ago = now_local - chrono::Duration::minutes(25);
    let time_25m_ago = dt_25m_ago.format("%H:%M").to_string();
    let u_c = UsersRepository::upsert_login(
        &env.pool,
        4003,
        "24014003",
        3,
        "Alumno 25Min Descartado",
        "tok_c",
    )
    .await
    .unwrap();
    UsersRepository::update_summary_config(
        &env.pool,
        u_c.chat_id,
        true,
        &format!(r#"["{time_25m_ago}"]"#),
        &format!(r#"[{weekday_iso}]"#),
    )
    .await
    .unwrap();

    // Ejecutar el boot lookahead
    env.worker().boot_lookback().await.unwrap();

    // Verificaciones:
    // Alumno A debió haber sido despachado y reclamado
    let key_a = format!(
        "summary:{}:{}:{}",
        u_a.chat_id,
        dt_5m_ago.format("%Y-%m-%d"),
        time_5m_ago
    );
    assert!(
        AlertsRepository::is_claimed(&env.pool, &key_a)
            .await
            .unwrap(),
        "El resumen de hace 5 minutos (no enviado previamente) debió reclamarse"
    );

    // Alumno C (hace 25 min) NO debió haber sido reclamado ni enviado
    let key_c = format!(
        "summary:{}:{}:{}",
        u_c.chat_id,
        dt_25m_ago.format("%Y-%m-%d"),
        time_25m_ago
    );
    assert!(
        !AlertsRepository::is_claimed(&env.pool, &key_c)
            .await
            .unwrap(),
        "El resumen de hace 25 minutos debió ser descartado para evitar aluviones"
    );
}

/// Test 5: Apagado ordenado del bucle `SummaryWorker::run_loop` mediante `watch::Receiver<bool>`.
#[tokio::test]
async fn test_summary_worker_graceful_shutdown() {
    let mut env = setup_test_env("shutdown_sum").await;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker = env.take_worker();
    let handle = tokio::spawn(worker.run_loop(shutdown_rx));

    // Permitir que el worker inicie el bucle
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Enviar señal de shutdown
    shutdown_tx.send(true).expect("Fallo al enviar shutdown");

    // Verificar que el worker se detiene en < 1 segundo
    let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
    assert!(
        result.is_ok(),
        "SummaryWorker debió terminar inmediatamente tras recibir señal de shutdown"
    );
}

/// Test 6: Reutilización del render compartido (`render_daily_summary`):
/// - Sin entregas pendientes: mensaje único con copy calmado `PENDIENTES_ALL_CLEAR`.
/// - Con entregas pendientes: formato detallado con fechas en hora local Managua y badges.
#[tokio::test]
async fn test_summary_delivery_renders_tasks_and_empty_fallback() {
    let env = setup_test_env("render_digest").await;

    // 1. Alumno sin entregas: fallback calmado
    let u_empty =
        UsersRepository::upsert_login(&env.pool, 5001, "24015001", 1, "Alumno Al Día", "tok_empty")
            .await
            .unwrap();

    env.worker()
        .dispatch_summary_to_user(u_empty.chat_id)
        .await
        .unwrap();

    let requests = env.server.received_requests().await.unwrap();
    let last_req = requests.last().expect("Debió enviarse mensaje a Telegram");
    let body_str = String::from_utf8_lossy(&last_req.body);
    assert!(
        body_str.contains("no%20debes%20nada")
            || body_str.contains("no debes nada")
            || body_str.contains("Milagro"),
        "El mensaje vacío debió contener el copy calmado PENDIENTES_ALL_CLEAR"
    );

    // 2. Alumno con entregas: resumen con curso y plazo
    let u_tasks = UsersRepository::upsert_login(
        &env.pool,
        5002,
        "24015002",
        2,
        "Alumno Con Tareas",
        "tok_tasks",
    )
    .await
    .unwrap();

    CoursesRepository::upsert_course(&env.pool, 301, "Sistemas Operativos", Some("SO101"))
        .await
        .unwrap();
    CoursesRepository::link_user(&env.pool, u_tasks.chat_id, 301)
        .await
        .unwrap();

    let now_epoch = Utc::now().timestamp();
    let due_in_2d = now_epoch + 172800; // vence en 2 días
    AssignmentsRepository::upsert(
        &env.pool,
        9001,
        Some(90010),
        301,
        "Laboratorio de Memoria Virtual",
        Some(due_in_2d),
        None,
    )
    .await
    .unwrap();

    env.worker()
        .dispatch_summary_to_user(u_tasks.chat_id)
        .await
        .unwrap();

    let requests_after = env.server.received_requests().await.unwrap();
    let task_req = requests_after
        .last()
        .expect("Debió enviarse mensaje con tareas");
    let task_body = String::from_utf8_lossy(&task_req.body);
    assert!(
        task_body.contains("Laboratorio") || task_body.contains("Memoria"),
        "El mensaje debió detallar la tarea asignada"
    );
    assert!(
        task_body.contains("Sistemas") || task_body.contains("Operativos"),
        "El mensaje debió detallar el nombre del curso"
    );
}
