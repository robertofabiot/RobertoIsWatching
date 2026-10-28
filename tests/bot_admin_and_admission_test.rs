use std::sync::Arc;
use teloxide::adaptors::throttle::Limits;
use teloxide::requests::RequesterExt;
use teloxide::types::Message;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use robertos_watching_bot::bot::commands::AdminCommand;
use robertos_watching_bot::bot::handlers::admin::handle_admin;
use robertos_watching_bot::bot::Bot;
use robertos_watching_bot::config::Config;
use robertos_watching_bot::storage::{
    self, AdmissionResult, CoursesRepository, SettingsRepository, UsersRepository,
};

fn make_msg(chat_id: i64, text: &str) -> Message {
    let msg_json = serde_json::json!({
        "message_id": 100,
        "date": 1700000000,
        "chat": {
            "id": chat_id,
            "type": "private",
            "first_name": "TestUser"
        },
        "text": text
    });
    serde_json::from_value(msg_json).unwrap()
}

fn create_test_config(database_url: String, admin_chat_id: i64) -> Config {
    Config {
        bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
        admin_chat_id,
        database_url,
        max_users_default: 300,
        bot_tz: chrono_tz::America::Managua,
        rust_log: "info".to_string(),
        scan_interval_min: 30,
    }
}

#[tokio::test]
async fn test_admin_guard_silently_ignores_non_admin_and_serves_admin() {
    let tg_server = MockServer::start().await;

    // Mock para respuestas de Telegram (sendMessage)
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "message_id": 101,
                "date": 1700000001,
                "chat": {
                    "id": 1125347763,
                    "type": "private"
                },
                "text": "admin reply"
            }
        })))
        .mount(&tg_server)
        .await;

    let tg_url = tg_server.uri().parse::<reqwest::Url>().unwrap();
    let bot: Bot = teloxide::Bot::new("123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ")
        .set_api_url(tg_url)
        .throttle(Limits::default());

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_admin_guard_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let admin_chat_id = 1125347763;
    let config = Arc::new(create_test_config(database_url, admin_chat_id));
    let pool = storage::init(&config).await.expect("Init db failed");

    // Población inicial de datos para stats
    UsersRepository::upsert_login(&pool, 1001, "24011001", 11, "User Uno", "tok1")
        .await
        .unwrap();
    UsersRepository::upsert_login(&pool, 1002, "24011002", 12, "User Dos", "tok2")
        .await
        .unwrap();
    UsersRepository::mark_inactive(&pool, 1002, "token_expired")
        .await
        .unwrap();

    CoursesRepository::upsert_course(&pool, 501, "Cálculo I", Some("MAT101"))
        .await
        .unwrap();
    CoursesRepository::link_user(&pool, 1001, 501)
        .await
        .unwrap();

    // 1. Non-admin chat intenta ejecutar /admin_stats
    let non_admin_chat_id = 999999;
    let non_admin_msg = make_msg(non_admin_chat_id, "/admin_stats");
    let res_non_admin = handle_admin(
        bot.clone(),
        non_admin_msg,
        AdminCommand::AdminStats,
        pool.clone(),
        config.clone(),
    )
    .await;
    assert!(res_non_admin.is_ok());

    // El servidor WireMock no debe haber recibido ninguna petición sendMessage
    assert_eq!(
        tg_server.received_requests().await.unwrap().len(),
        0,
        "Comandos admin invocados por usuarios no admin no deben generar respuestas en Telegram"
    );

    // 2. Admin ejecuta /admin_stats
    let admin_msg = make_msg(admin_chat_id, "/admin_stats");
    let res_stats = handle_admin(
        bot.clone(),
        admin_msg.clone(),
        AdminCommand::AdminStats,
        pool.clone(),
        config.clone(),
    )
    .await;
    assert!(res_stats.is_ok(), "res_stats error: {:?}", res_stats.err());
    assert_eq!(tg_server.received_requests().await.unwrap().len(), 1);

    // 3. Admin ejecuta /toggle_registro
    let initial_reg = SettingsRepository::registration_open(&pool).await.unwrap();
    assert!(initial_reg);

    let res_toggle = handle_admin(
        bot.clone(),
        admin_msg.clone(),
        AdminCommand::ToggleRegistro,
        pool.clone(),
        config.clone(),
    )
    .await;
    assert!(res_toggle.is_ok());
    assert!(!SettingsRepository::registration_open(&pool).await.unwrap());
    assert_eq!(tg_server.received_requests().await.unwrap().len(), 2);

    // 4. Admin ejecuta /set_max_users 500
    let res_limit = handle_admin(
        bot.clone(),
        admin_msg.clone(),
        AdminCommand::SetMaxUsers(500),
        pool.clone(),
        config.clone(),
    )
    .await;
    assert!(res_limit.is_ok());
    assert_eq!(SettingsRepository::max_users(&pool).await.unwrap(), 500);
    assert_eq!(tg_server.received_requests().await.unwrap().len(), 3);

    // 5. Admin ejecuta /set_max_users 0 (rechazado con advertencia sin cambiar valor)
    let res_zero = handle_admin(
        bot.clone(),
        admin_msg,
        AdminCommand::SetMaxUsers(0),
        pool.clone(),
        config.clone(),
    )
    .await;
    assert!(res_zero.is_ok());
    assert_eq!(SettingsRepository::max_users(&pool).await.unwrap(), 500);
    assert_eq!(tg_server.received_requests().await.unwrap().len(), 4);

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn test_admission_decision_matrix_all_permutations() {
    // Matriz de decisión de 8 casos:
    // [Registro Abierto/Cerrado] × [Cupo Libre/Lleno] × [Nuevo/Existente]
    struct TestCase {
        open: bool,
        quota_full: bool,
        is_existing: bool,
        expected: &'static str,
    }

    let cases = [
        // 1. Abierto, Cupo Libre, Nuevo => Admitted
        TestCase {
            open: true,
            quota_full: false,
            is_existing: false,
            expected: "Admitted",
        },
        // 2. Abierto, Cupo Libre, Existente => AlreadyMember
        TestCase {
            open: true,
            quota_full: false,
            is_existing: true,
            expected: "AlreadyMember",
        },
        // 3. Abierto, Cupo Lleno, Nuevo => Full
        TestCase {
            open: true,
            quota_full: true,
            is_existing: false,
            expected: "Full",
        },
        // 4. Abierto, Cupo Lleno, Existente => AlreadyMember (conserva su cupo)
        TestCase {
            open: true,
            quota_full: true,
            is_existing: true,
            expected: "AlreadyMember",
        },
        // 5. Cerrado, Cupo Libre, Nuevo => Closed
        TestCase {
            open: false,
            quota_full: false,
            is_existing: false,
            expected: "Closed",
        },
        // 6. Cerrado, Cupo Libre, Existente => AlreadyMember (alumnos existentes pueden reconectar)
        TestCase {
            open: false,
            quota_full: false,
            is_existing: true,
            expected: "AlreadyMember",
        },
        // 7. Cerrado, Cupo Lleno, Nuevo => Closed
        TestCase {
            open: false,
            quota_full: true,
            is_existing: false,
            expected: "Closed",
        },
        // 8. Cerrado, Cupo Lleno, Existente => AlreadyMember
        TestCase {
            open: false,
            quota_full: true,
            is_existing: true,
            expected: "AlreadyMember",
        },
    ];

    for (idx, tc) in cases.iter().enumerate() {
        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_matrix_{idx}_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = create_test_config(database_url, 1125347763);
        let pool = storage::init(&config).await.expect("Init db failed");

        // 1. Configurar estado de registro
        SettingsRepository::set_registration_open(&pool, tc.open)
            .await
            .unwrap();

        // 2. Configurar cupos
        if tc.quota_full {
            SettingsRepository::set_max_users(&pool, 1).await.unwrap();
            // Llenar el único cupo con un usuario de control
            UsersRepository::upsert_login(
                &pool,
                888000 + idx as i64,
                &format!("2401888{idx}"),
                888,
                "Control User",
                "tok_ctrl",
            )
            .await
            .unwrap();
        } else {
            SettingsRepository::set_max_users(&pool, 10).await.unwrap();
        }

        let target_chat_id = 777000 + idx as i64;
        let target_cif = format!("2401777{idx}");

        // 3. Si debe ser existente, registrar previamente al alumno
        if tc.is_existing {
            UsersRepository::upsert_login(
                &pool,
                target_chat_id,
                &target_cif,
                777,
                "Existing Student",
                "tok_old",
            )
            .await
            .unwrap();
        }

        // 4. Ejecutar try_admit
        let result = UsersRepository::try_admit(
            &pool,
            target_chat_id,
            &target_cif,
            777,
            "Target Student",
            "tok_new",
        )
        .await
        .unwrap();

        // 5. Validar resultado contra la matriz
        match tc.expected {
            "Admitted" => assert!(
                matches!(result, AdmissionResult::Admitted(_)),
                "Caso {idx} falló: esperado Admitted, obtenido {:?}",
                result
            ),
            "AlreadyMember" => assert!(
                matches!(result, AdmissionResult::AlreadyMember(_)),
                "Caso {idx} falló: esperado AlreadyMember, obtenido {:?}",
                result
            ),
            "Full" => assert!(
                matches!(result, AdmissionResult::Full),
                "Caso {idx} falló: esperado Full, obtenido {:?}",
                result
            ),
            "Closed" => assert!(
                matches!(result, AdmissionResult::Closed),
                "Caso {idx} falló: esperado Closed, obtenido {:?}",
                result
            ),
            other => panic!("Tipo de resultado no reconocido: {}", other),
        }

        let _ = std::fs::remove_file(&db_path);
    }
}
