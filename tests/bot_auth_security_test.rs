use robertos_watching_bot::bot::handlers::auth::handle_login;
use robertos_watching_bot::bot::rate_limit::LoginRateLimiter;
use robertos_watching_bot::bot::Bot;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::moodle::{MoodleClient, MoodleGate};
use robertos_watching_bot::storage;
use std::sync::{Arc, Mutex};
use teloxide::adaptors::throttle::Limits;
use teloxide::requests::RequesterExt;
use teloxide::types::Message;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone)]
struct BufferWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
    type Writer = BufferWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn test_login_password_never_reaches_tracing_logs_or_storage() {
    let log_buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = BufferWriter(log_buffer.clone());

    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::TRACE)
        .finish();

    let _guard = tracing::subscriber::set_default(subscriber);

    // 1. Mock de Servidor Moodle
    let moodle_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/login/token.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token": "tok_valid_secret_999"
        })))
        .mount(&moodle_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(|req: &wiremock::Request| {
            let body = std::str::from_utf8(&req.body).unwrap_or("");
            if body.contains("core_enrol_get_users_courses") {
                ResponseTemplate::new(200).set_body_json(serde_json::json!([]))
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "userid": 777,
                    "fullname": "Estudiante Prueba Seguridad"
                }))
            }
        })
        .mount(&moodle_server)
        .await;

    // 2. Mock de Servidor Telegram API
    let tg_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex("/bot.*/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 101,
                "date": 1700000001,
                "chat": {
                    "id": 1125347763,
                    "type": "private"
                },
                "text": "ok"
            }
        })))
        .mount(&tg_server)
        .await;

    let tg_url = tg_server.uri().parse::<reqwest::Url>().unwrap();
    let bot: Bot = teloxide::Bot::new("123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ")
        .set_api_url(tg_url)
        .throttle(Limits::default());

    // 3. Base de datos SQLite temporal
    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_security_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = Config {
        bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
        admin_chat_id: 1125347763,
        database_url,
        max_users_default: 300,
        bot_tz: chrono_tz::America::Managua,
        rust_log: "trace".to_string(),
        scan_interval_min: 30,
    };

    let pool = storage::init(&config)
        .await
        .expect("Fallo al inicializar BD");

    let gate = MoodleGate::new(4, 3).without_jitter();
    let moodle = robertos_watching_bot::moodle::MoodleClientBuilder::new()
        .base_url(moodle_server.uri())
        .gate(gate)
        .max_retries(0);
    let moodle_client = MoodleClient::from_builder(moodle);

    let rate_limiter = LoginRateLimiter::new();

    let sentinel_password = "ULTRA_SECRET_CENTINELA_PASSWORD_12345!@#";

    let msg_json = serde_json::json!({
        "message_id": 100,
        "date": 1700000000,
        "chat": {
            "id": 1125347763,
            "type": "private",
            "first_name": "Roberto"
        },
        "text": format!("/login 24014121 {sentinel_password}")
    });
    let msg: Message = serde_json::from_value(msg_json).unwrap();

    let args = format!("24014121 {sentinel_password}");

    // Ejecutar handler de login
    let res = handle_login(bot, msg, args, pool.clone(), moodle_client, rate_limiter).await;

    assert!(res.is_ok(), "handle_login falló: {:?}", res);

    // 4. Verificación A: Logs de Tracing en nivel TRACE
    let logs = {
        let bytes = log_buffer.lock().unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    };

    assert!(
        !logs.contains(sentinel_password),
        "¡Fuga crítica de seguridad! La contraseña centinela fue registrada en los logs de tracing: {logs}"
    );

    // 5. Verificación B: Esquema de base de datos SQLite no contiene columna de clave
    let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
        sqlx::query_as("PRAGMA table_info(users)")
            .fetch_all(&pool)
            .await
            .expect("Fallo al consultar PRAGMA table_info");

    for col in &columns {
        let col_name = col.1.to_lowercase();
        assert!(
            !col_name.contains("pass") && !col_name.contains("clave"),
            "¡Fuga en diseño! La tabla users contiene una columna para contraseñas: {col_name}"
        );
    }

    // 6. Verificación C: Valores almacenados en SQLite
    let rows: Vec<(i64, String, String, String)> =
        sqlx::query_as("SELECT chat_id, uam_cif, fullname, wstoken FROM users")
            .fetch_all(&pool)
            .await
            .expect("Fallo al consultar tabla users");

    assert_eq!(rows.len(), 1);
    let (chat_id, cif, fullname, wstoken) = &rows[0];
    assert_eq!(*chat_id, 1125347763);
    assert_eq!(cif, "24014121");
    assert_eq!(fullname, "Estudiante Prueba Seguridad");
    assert_eq!(wstoken, "tok_valid_secret_999");

    assert!(!cif.contains(sentinel_password));
    assert!(!fullname.contains(sentinel_password));
    assert!(!wstoken.contains(sentinel_password));

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_logout_purges_user_cascades_and_alerts() {
    let tg_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex("/bot.*/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 201,
                "date": 1700000002,
                "chat": {
                    "id": 555555,
                    "type": "private"
                },
                "text": "bye"
            }
        })))
        .mount(&tg_server)
        .await;

    let tg_url = tg_server.uri().parse::<reqwest::Url>().unwrap();
    let bot: Bot = teloxide::Bot::new("123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ")
        .set_api_url(tg_url)
        .throttle(Limits::default());

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_logout_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = Config {
        bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
        admin_chat_id: 1125347763,
        database_url,
        max_users_default: 300,
        bot_tz: chrono_tz::America::Managua,
        rust_log: "info".to_string(),
        scan_interval_min: 30,
    };

    let pool = storage::init(&config)
        .await
        .expect("Fallo al inicializar BD");

    // Crear usuario, vincular curso y agregar una alerta
    let _ = storage::UsersRepository::upsert_login(
        &pool,
        555555,
        "24015555",
        991,
        "Alumno Para Logout",
        "wstoken_logout",
    )
    .await
    .unwrap();

    let _ = storage::CoursesRepository::upsert_course(&pool, 777, "Física II", Some("FIS102"))
        .await
        .unwrap();
    storage::CoursesRepository::link_user(&pool, 555555, 777)
        .await
        .unwrap();

    let _ = storage::AlertsRepository::try_claim(
        &pool,
        "deadline:555555:10:24h",
        "deadline",
        Some(555555),
    )
    .await
    .unwrap();

    assert_eq!(storage::UsersRepository::count(&pool).await.unwrap(), 1);
    assert!(
        storage::AlertsRepository::is_claimed(&pool, "deadline:555555:10:24h")
            .await
            .unwrap()
    );

    let msg_json = serde_json::json!({
        "message_id": 200,
        "date": 1700000000,
        "chat": {
            "id": 555555,
            "type": "private",
            "first_name": "LogoutUser"
        },
        "text": "/logout"
    });
    let msg: Message = serde_json::from_value(msg_json).unwrap();

    // 1. Ejecutar /logout
    let res = robertos_watching_bot::bot::handlers::auth::handle_logout(
        bot.clone(),
        msg.clone(),
        pool.clone(),
        None,
    )
    .await;
    assert!(res.is_ok());

    // 2. Verificar purga en cascada
    assert_eq!(storage::UsersRepository::count(&pool).await.unwrap(), 0);
    assert!(storage::UsersRepository::get(&pool, 555555)
        .await
        .unwrap()
        .is_none());
    assert!(storage::CoursesRepository::courses_for_user(&pool, 555555)
        .await
        .unwrap()
        .is_empty());
    assert!(
        !storage::AlertsRepository::is_claimed(&pool, "deadline:555555:10:24h")
            .await
            .unwrap()
    );

    // 3. Ejecutar /logout cuando no está logueado no debe fallar
    let res_again =
        robertos_watching_bot::bot::handlers::auth::handle_logout(bot, msg, pool.clone(), None)
            .await;
    assert!(res_again.is_ok());

    let _ = std::fs::remove_file(&db_path);
}
