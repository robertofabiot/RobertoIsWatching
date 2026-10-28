use robertos_watching_bot::config::Config;
use robertos_watching_bot::moodle::gate::Priority;
use robertos_watching_bot::moodle::MoodleClient;
use robertos_watching_bot::services::sync::refresh_user;
use robertos_watching_bot::storage::{
    self, AssignmentsRepository, CoursesRepository, ModulesRepository, SubmissionsRepository,
    UsersRepository,
};
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
    }
}

#[tokio::test]
async fn test_refresh_user_seeding_and_idempotence() {
    let moodle_server = MockServer::start().await;

    // 1. Mock de core_enrol_get_users_courses
    let courses_json = serde_json::json!([
        {
            "id": 101,
            "shortname": "EDD101",
            "fullname": "Estructuras de Datos",
            "displayname": "Estructuras de Datos",
            "visible": 1,
            "enddate": 0,
            "completed": false,
            "hidden": false
        }
    ]);

    // 2. Mock de mod_assign_get_assignments
    let future_duedate = chrono::Utc::now().timestamp() + 86400;
    let assignments_json = serde_json::json!({
        "courses": [
            {
                "id": 101,
                "assignments": [
                    {
                        "id": 201,
                        "cmid": 5001,
                        "course": 101,
                        "name": "Laboratorio 1 - Grafos",
                        "duedate": future_duedate,
                        "allowsubmissionsfromdate": 0,
                        "cutoffdate": 0,
                        "timemodified": 0
                    }
                ]
            }
        ]
    });

    // 3. Mock de mod_assign_get_submission_status
    let submission_json = serde_json::json!({
        "lastattempt": {
            "submission": {
                "status": "submitted"
            },
            "graded": false,
            "gradingstatus": "notgraded"
        },
        "feedback": null
    });

    // 4. Mock de core_course_get_contents
    let contents_json = serde_json::json!([
        {
            "id": 1001,
            "name": "Unidad 1: Introducción a Grafos",
            "modules": [
                {
                    "id": 301,
                    "name": "Guía_Laboratorio_1.pdf",
                    "modname": "resource",
                    "url": "https://uamvirtual.uam.edu.ni/mod/resource/view.php?id=301"
                }
            ]
        }
    ]);

    // Montar respuestas en WireMock usando matcher por body de form
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(move |req: &wiremock::Request| {
            let body_str = std::str::from_utf8(&req.body).unwrap_or("");
            if body_str.contains("core_enrol_get_users_courses") {
                ResponseTemplate::new(200).set_body_json(&courses_json)
            } else if body_str.contains("mod_assign_get_assignments") {
                ResponseTemplate::new(200).set_body_json(&assignments_json)
            } else if body_str.contains("mod_assign_get_submission_status") {
                ResponseTemplate::new(200).set_body_json(&submission_json)
            } else if body_str.contains("core_course_get_contents") {
                ResponseTemplate::new(200).set_body_json(&contents_json)
            } else {
                ResponseTemplate::new(404)
            }
        })
        .mount(&moodle_server)
        .await;

    let moodle = MoodleClient::new(moodle_server.uri());

    let rand_id: u64 = rand::random();
    let db_path = std::env::temp_dir().join(format!("test_sync_refresh_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = create_test_config(database_url);
    let pool = storage::init(&config).await.expect("Init db failed");

    let chat_id = 12345678;
    UsersRepository::upsert_login(
        &pool,
        chat_id,
        "24019999",
        888,
        "Estudiante Sincronizado",
        "wstoken_valid_mock",
    )
    .await
    .unwrap();

    // --- PRIMERA EJECUCIÓN (Siembra Silenciosa / Línea Base) ---
    let summary1 = refresh_user(&pool, &moodle, chat_id, Priority::Interactive)
        .await
        .expect("refresh_user debe ser exitoso");

    assert_eq!(summary1.courses_count, 1);
    assert_eq!(summary1.assignments_count, 1);
    assert_eq!(summary1.new_assignments, 1);
    assert_eq!(summary1.updated_assignments, 0);
    assert_eq!(summary1.new_grades, 0);
    assert!(summary1.has_changes());

    // Verificar persistencia en base de datos
    let courses = CoursesRepository::courses_for_user(&pool, chat_id)
        .await
        .unwrap();
    assert_eq!(courses.len(), 1);
    assert_eq!(courses[0].id, 101);
    assert!(courses[0].last_scanned.is_some());

    let assign = AssignmentsRepository::get(&pool, 201)
        .await
        .unwrap()
        .expect("Tarea 201 debe existir");
    assert_eq!(assign.name, "Laboratorio 1 - Grafos");

    let sub = SubmissionsRepository::get(&pool, chat_id, 201)
        .await
        .unwrap()
        .expect("Entrega debe existir");
    assert_eq!(sub.status.as_deref(), Some("submitted"));
    assert!(!sub.graded);

    let module = ModulesRepository::get(&pool, 301)
        .await
        .unwrap()
        .expect("Módulo 301 debe existir");
    assert_eq!(module.name, "Guía_Laboratorio_1.pdf");

    // Verificar A-03: Cero alertas emitidas (siembra silenciosa)
    let alert_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sent_alerts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        alert_count.0, 0,
        "La siembra silenciosa no debe generar alertas"
    );

    // --- SEGUNDA EJECUCIÓN (Idempotencia) ---
    let summary2 = refresh_user(&pool, &moodle, chat_id, Priority::Interactive)
        .await
        .expect("Segunda ejecución de refresh_user debe ser exitosa");

    assert_eq!(summary2.courses_count, 1);
    assert_eq!(summary2.assignments_count, 1);
    assert_eq!(summary2.new_assignments, 0);
    assert_eq!(summary2.updated_assignments, 0);
    assert_eq!(summary2.new_grades, 0);
    assert!(
        !summary2.has_changes(),
        "Una segunda ejecución sin cambios en Moodle no debe reportar novedades"
    );

    // Los datos siguen íntegros sin duplicaciones
    let courses_count = CoursesRepository::courses_for_user(&pool, chat_id)
        .await
        .unwrap()
        .len();
    assert_eq!(courses_count, 1);

    let _ = std::fs::remove_file(&db_path);
}
