use std::sync::Arc;
use teloxide::adaptors::throttle::Limits;
use teloxide::requests::RequesterExt;
use teloxide::types::{CallbackQuery, Message};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use robertos_watching_bot::bot::handlers::assignments::handle_pendientes;
use robertos_watching_bot::bot::handlers::config::{handle_config, handle_config_callback};
use robertos_watching_bot::bot::handlers::courses::handle_cursos;
use robertos_watching_bot::bot::handlers::grades::handle_notas;
use robertos_watching_bot::bot::handlers::summary::handle_resumen;
use robertos_watching_bot::bot::handlers::sync::handle_sync;
use robertos_watching_bot::bot::Bot;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::moodle::MoodleClient;
use robertos_watching_bot::storage::{
    self, AssignmentsRepository, CoursesRepository, SubmissionData, SubmissionsRepository,
    UsersRepository,
};

fn make_msg(chat_id: i64, text: &str) -> Message {
    let msg_json = serde_json::json!({
        "message_id": 100,
        "date": 1700000000,
        "chat": {
            "id": chat_id,
            "type": "private",
            "first_name": "TestStudent"
        },
        "text": text
    });
    serde_json::from_value(msg_json).unwrap()
}

fn make_callback(chat_id: i64, data: &str, msg_id: i32) -> CallbackQuery {
    let cb_json = serde_json::json!({
        "id": "cb_query_12345",
        "from": {
            "id": chat_id,
            "is_bot": false,
            "first_name": "TestStudent"
        },
        "message": {
            "message_id": msg_id,
            "date": 1700000000,
            "chat": {
                "id": chat_id,
                "type": "private"
            },
            "text": "Config View"
        },
        "chat_instance": "instance_999",
        "data": data
    });
    serde_json::from_value(cb_json).unwrap()
}

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
async fn test_sync_cooldown_boundary_9m59s_vs_10m00s() {
    let tg_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 101,
                "date": 1700000001,
                "chat": { "id": 77701, "type": "private" },
                "text": "reply"
            }
        })))
        .mount(&tg_server)
        .await;

    let moodle_server = MockServer::start().await;
    // Mock general para llamadas a Moodle si el cooldown se vence
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&moodle_server)
        .await;

    let tg_url = tg_server.uri().parse::<reqwest::Url>().unwrap();
    let bot: Bot = teloxide::Bot::new("123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ")
        .set_api_url(tg_url)
        .throttle(Limits::default());
    let moodle = MoodleClient::new(moodle_server.uri());

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_cooldown_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = create_test_config(database_url);
    let pool = storage::init(&config).await.expect("Init db failed");

    let chat_id = 77701;
    UsersRepository::upsert_login(&pool, chat_id, "24017701", 100, "Cooldown Student", "tok")
        .await
        .unwrap();
    CoursesRepository::upsert_course(&pool, 111, "Física I", Some("FIS101"))
        .await
        .unwrap();
    CoursesRepository::link_user(&pool, chat_id, 111)
        .await
        .unwrap();

    // 1. Caso Límite A: Escaneo hace 9 minutos y 59 segundos (599 segundos) => Cooldown Activo
    // Actualizar courses.last_scanned manualmente a now - 599s
    sqlx::query("UPDATE courses SET last_scanned = datetime('now', '-599 seconds') WHERE id = 111")
        .execute(&pool)
        .await
        .unwrap();

    let msg = make_msg(chat_id, "/sync");
    let res_cooldown = handle_sync(bot.clone(), msg.clone(), pool.clone(), moodle.clone()).await;
    assert!(res_cooldown.is_ok());

    // Debe haber CERO llamadas hacia el servidor de Moodle porque el cooldown respondió desde SQLite
    let moodle_reqs = moodle_server.received_requests().await.unwrap();
    assert_eq!(
        moodle_reqs.len(),
        0,
        "A los 9m59s de cooldown no debe realizarse ninguna petición HTTP a Moodle"
    );

    // 2. Caso Límite B: Escaneo hace 10 minutos y 1 segundo (601 segundos) => Cooldown Vencido
    sqlx::query("UPDATE courses SET last_scanned = datetime('now', '-601 seconds') WHERE id = 111")
        .execute(&pool)
        .await
        .unwrap();

    let res_expired = handle_sync(bot.clone(), msg, pool.clone(), moodle.clone()).await;
    assert!(res_expired.is_ok());

    // Al vencer el cooldown, debe contactar a Moodle para refrescar
    let moodle_reqs_after = moodle_server.received_requests().await.unwrap();
    assert!(
        !moodle_reqs_after.is_empty(),
        "A los 10m01s debe superarse el cooldown y realizar peticiones a Moodle"
    );

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_config_callbacks_and_validation() {
    let tg_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 105,
                "date": 1700000001,
                "chat": { "id": 88801, "type": "private" },
                "text": "config reply"
            }
        })))
        .mount(&tg_server)
        .await;

    let tg_url = tg_server.uri().parse::<reqwest::Url>().unwrap();
    let bot: Bot = teloxide::Bot::new("123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ")
        .set_api_url(tg_url)
        .throttle(Limits::default());

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_cfg_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = create_test_config(database_url);
    let pool = storage::init(&config).await.expect("Init db failed");

    let chat_id = 88801;
    let user =
        UsersRepository::upsert_login(&pool, chat_id, "24018801", 101, "Config Student", "tok")
            .await
            .unwrap();

    // Valores iniciales por defecto
    assert!(user.summary_enabled);
    assert_eq!(user.summary_times, "[\"07:30\", \"20:00\"]");
    assert_eq!(user.summary_days, "[1, 2, 3, 4, 5]");

    // 1. Callback cfg:toggle_pause (debe cambiar a false)
    let cb_toggle = make_callback(chat_id, "cfg:toggle_pause", 105);
    let res_toggle = handle_config_callback(bot.clone(), cb_toggle, pool.clone()).await;
    assert!(res_toggle.is_ok());

    let updated1 = UsersRepository::get(&pool, chat_id).await.unwrap().unwrap();
    assert!(!updated1.summary_enabled);

    // 2. Callback cfg:repeat (debe rotar a 3x ["07:30", "13:30", "20:00"])
    let cb_repeat = make_callback(chat_id, "cfg:repeat", 105);
    let res_repeat = handle_config_callback(bot.clone(), cb_repeat, pool.clone()).await;
    assert!(res_repeat.is_ok());

    let updated2 = UsersRepository::get(&pool, chat_id).await.unwrap().unwrap();
    let times2: Vec<String> = serde_json::from_str(&updated2.summary_times).unwrap();
    assert_eq!(times2, vec!["07:30", "13:30", "20:00"]);

    // 3. Callback cfg:day:6 (activar sábado en summary_days)
    let cb_day = make_callback(chat_id, "cfg:day:6", 105);
    let res_day = handle_config_callback(bot.clone(), cb_day, pool.clone()).await;
    assert!(res_day.is_ok());

    let updated3 = UsersRepository::get(&pool, chat_id).await.unwrap().unwrap();
    let days3: Vec<i64> = serde_json::from_str(&updated3.summary_days).unwrap();
    assert!(days3.contains(&6));

    // 4. Callback cfg:hour:12:00 (agregar horario 12:00)
    let cb_hour = make_callback(chat_id, "cfg:hour:12:00", 105);
    let res_hour = handle_config_callback(bot.clone(), cb_hour, pool.clone()).await;
    assert!(res_hour.is_ok());

    let updated4 = UsersRepository::get(&pool, chat_id).await.unwrap().unwrap();
    let times4: Vec<String> = serde_json::from_str(&updated4.summary_times).unwrap();
    assert!(times4.contains(&"12:00".to_string()));

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_student_commands_surface_cursos_pendientes_notas_resumen() {
    let tg_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 200,
                "date": 1700000001,
                "chat": { "id": 99901, "type": "private" },
                "text": "command reply"
            }
        })))
        .mount(&tg_server)
        .await;

    let tg_url = tg_server.uri().parse::<reqwest::Url>().unwrap();
    let bot: Bot = teloxide::Bot::new("123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ")
        .set_api_url(tg_url)
        .throttle(Limits::default());

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_cmds_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = Arc::new(create_test_config(database_url));
    let pool = storage::init(&config).await.expect("Init db failed");

    let chat_id = 99901;
    UsersRepository::upsert_login(&pool, chat_id, "24019901", 102, "Student AllCmds", "tok")
        .await
        .unwrap();

    CoursesRepository::upsert_course(&pool, 301, "Estructuras de Datos", Some("EDD"))
        .await
        .unwrap();
    CoursesRepository::link_user(&pool, chat_id, 301)
        .await
        .unwrap();

    let future_time = chrono::Utc::now().timestamp() + 7200;
    AssignmentsRepository::upsert(
        &pool,
        401,
        Some(1001),
        301,
        "Proyecto Árboles B+",
        Some(future_time),
        None,
    )
    .await
    .unwrap();

    SubmissionsRepository::upsert(
        &pool,
        SubmissionData {
            chat_id,
            assignment_id: 401,
            status: Some("submitted"),
            graded: true,
            grade: Some("95 / 100"),
            feedback: Some("Gran implementación de balanceo"),
            due_override: None,
        },
    )
    .await
    .unwrap();

    // 1. /cursos
    let msg_cursos = make_msg(chat_id, "/cursos");
    let res_cursos = handle_cursos(bot.clone(), msg_cursos, pool.clone()).await;
    assert!(res_cursos.is_ok());

    // 2. /pendientes (como ya fue entregada, debe responder al día)
    let msg_pendientes = make_msg(chat_id, "/pendientes");
    let res_pendientes =
        handle_pendientes(bot.clone(), msg_pendientes, pool.clone(), config.clone()).await;
    assert!(res_pendientes.is_ok());

    // 3. /notas
    let msg_notas = make_msg(chat_id, "/notas");
    let res_notas = handle_notas(bot.clone(), msg_notas, pool.clone()).await;
    assert!(res_notas.is_ok());

    // 4. /resumen
    let msg_resumen = make_msg(chat_id, "/resumen");
    let res_resumen = handle_resumen(bot.clone(), msg_resumen, pool.clone(), config.clone()).await;
    assert!(res_resumen.is_ok());

    // 5. /config
    let msg_config = make_msg(chat_id, "/config");
    let res_config = handle_config(bot.clone(), msg_config, pool.clone()).await;
    assert!(res_config.is_ok());

    let _ = std::fs::remove_file(&db_path);
}
