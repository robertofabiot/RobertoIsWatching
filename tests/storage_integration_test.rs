use robertos_watching_bot::config::Config;
use robertos_watching_bot::storage::{
    AdmissionResult, AlertsRepository, AssignmentsRepository, CoursesRepository,
    SettingsRepository, SubmissionData, SubmissionsRepository, UsersRepository,
};
use sqlx::SqlitePool;
use std::sync::Arc;

async fn setup_test_db(name: &str) -> (SqlitePool, std::path::PathBuf) {
    let rand_id: u64 = rand::random();
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("roberto_{name}_{rand_id}.db"));
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

    let pool = robertos_watching_bot::storage::init(&config)
        .await
        .expect("Fallo al inicializar BD de prueba");

    (pool, db_path)
}

fn cleanup_test_db(pool: SqlitePool, db_path: std::path::PathBuf) {
    drop(pool);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
}

#[tokio::test]
async fn test_user_delete_cascades() {
    let (pool, db_path) = setup_test_db("cascade").await;

    // 1. Crear usuario
    let user = UsersRepository::upsert_login(
        &pool,
        1001,
        "22010011",
        5501,
        "Estudiante Prueba",
        "tok_123456",
    )
    .await
    .expect("Fallo al crear usuario");
    assert_eq!(user.chat_id, 1001);

    // 2. Crear curso y vincularlo
    let course = CoursesRepository::upsert_course(&pool, 501, "Cálculo I", Some("MAT101"))
        .await
        .expect("Fallo al crear curso");
    CoursesRepository::link_user(&pool, 1001, 501)
        .await
        .expect("Fallo al vincular curso");

    // 3. Crear asignación y entrega
    let assign_res = AssignmentsRepository::upsert(
        &pool,
        2001,
        Some(7001),
        course.id,
        "Tarea 1 - Derivadas",
        Some(1700000000),
        None,
    )
    .await
    .expect("Fallo al crear tarea");
    assert!(matches!(
        assign_res,
        robertos_watching_bot::storage::UpsertAssignmentResult::Inserted(_)
    ));

    SubmissionsRepository::upsert(
        &pool,
        SubmissionData {
            chat_id: 1001,
            assignment_id: 2001,
            status: Some("submitted"),
            graded: false,
            grade: None,
            feedback: None,
            due_override: None,
        },
    )
    .await
    .expect("Fallo al crear submission");

    // Verificar que existen los datos vinculados
    let user_courses = CoursesRepository::courses_for_user(&pool, 1001)
        .await
        .unwrap();
    assert_eq!(user_courses.len(), 1);

    let sub = SubmissionsRepository::get(&pool, 1001, 2001).await.unwrap();
    assert!(sub.is_some());

    // 4. Eliminar el usuario (logout)
    let deleted = UsersRepository::delete(&pool, 1001).await.unwrap();
    assert!(deleted);

    // 5. Verificar que ON DELETE CASCADE limpió user_courses y user_submissions
    let user_courses_after = CoursesRepository::courses_for_user(&pool, 1001)
        .await
        .unwrap();
    assert_eq!(
        user_courses_after.len(),
        0,
        "user_courses debió eliminarse en cascada"
    );

    let sub_after = SubmissionsRepository::get(&pool, 1001, 2001).await.unwrap();
    assert!(
        sub_after.is_none(),
        "user_submissions debió eliminarse en cascada"
    );

    // El curso y la tarea deben seguir existiendo
    assert!(CoursesRepository::get(&pool, 501).await.unwrap().is_some());
    assert!(AssignmentsRepository::get(&pool, 2001)
        .await
        .unwrap()
        .is_some());

    cleanup_test_db(pool, db_path);
}

#[tokio::test]
async fn test_idempotent_claims_and_release() {
    let (pool, db_path) = setup_test_db("claims").await;

    let alert_key = "deadline:1001:2001:24h";

    // 1. Primer reclamo: debe ser exitoso (true)
    let first_claim = AlertsRepository::try_claim(&pool, alert_key, "deadline", Some(1001))
        .await
        .expect("Error al reclamar alerta");
    assert!(first_claim, "El primer reclamo debe tener éxito");

    // 2. Segundo reclamo idéntico: debe fallar (false) por idempotencia (A-14)
    let second_claim = AlertsRepository::try_claim(&pool, alert_key, "deadline", Some(1001))
        .await
        .expect("Error al intentar segundo reclamo");
    assert!(
        !second_claim,
        "El segundo reclamo duplicado debe ser rechazado"
    );

    assert!(AlertsRepository::is_claimed(&pool, alert_key)
        .await
        .unwrap());

    // 3. Simular fallo de red hacia Telegram -> release(key)
    let released = AlertsRepository::release(&pool, alert_key).await.unwrap();
    assert!(released);
    assert!(!AlertsRepository::is_claimed(&pool, alert_key)
        .await
        .unwrap());

    // 4. Reintentar tras liberación -> ahora debe volver a tener éxito
    let third_claim = AlertsRepository::try_claim(&pool, alert_key, "deadline", Some(1001))
        .await
        .unwrap();
    assert!(third_claim, "Debe permitir reintento tras liberación");

    // 5. Purga de chat
    let purged = AlertsRepository::purge_chat(&pool, 1001).await.unwrap();
    assert_eq!(purged, 1);
    assert!(!AlertsRepository::is_claimed(&pool, alert_key)
        .await
        .unwrap());

    cleanup_test_db(pool, db_path);
}

#[tokio::test]
async fn test_concurrent_admission_with_quota() {
    let (pool, db_path) = setup_test_db("concurrency").await;

    // Fijamos max_users = 5
    SettingsRepository::set_max_users(&pool, 5)
        .await
        .expect("Fallo al fijar max_users");

    let pool = Arc::new(pool);
    let mut handles = Vec::new();

    // Lanzamos 20 solicitudes de admisión simultáneas
    for i in 0..20 {
        let pool_clone = Arc::clone(&pool);
        let chat_id = 2000 + i;
        let cif = format!("CIF{chat_id}");
        let fullname = format!("Alumno {i}");
        let token = format!("token_{chat_id}");

        let handle = tokio::spawn(async move {
            UsersRepository::try_admit(&pool_clone, chat_id, &cif, 10000 + i, &fullname, &token)
                .await
        });
        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await);
    }

    let mut admitted_users = Vec::new();
    let mut full_count = 0;

    for res in results {
        let admission = res
            .expect("Task tokio panicked")
            .expect("Error en try_admit");
        match admission {
            AdmissionResult::Admitted(user) => admitted_users.push(user),
            AdmissionResult::Full => full_count += 1,
            other => panic!("Resultado inesperado: {:?}", other),
        }
    }

    // Exactamente 5 admitidos y 15 rechazados por cupo lleno
    assert_eq!(
        admitted_users.len(),
        5,
        "Deben haberse admitido exactamente 5 alumnos"
    );
    assert_eq!(
        full_count, 15,
        "Deben haberse rechazado exactamente 15 por Full"
    );

    let total_in_db = UsersRepository::count(&pool).await.unwrap();
    assert_eq!(total_in_db, 5, "Total en BD debe coincidir con max_users");

    // Re-login de uno de los admitidos no debe consumir cupo extra ni ser rechazado
    let admitted_sample = &admitted_users[0];
    let relogin_res = UsersRepository::try_admit(
        &pool,
        admitted_sample.chat_id,
        &admitted_sample.uam_cif,
        admitted_sample.uam_user_id,
        "Alumno Renombrado",
        "new_token_sample",
    )
    .await
    .unwrap();
    assert!(
        matches!(relogin_res, AdmissionResult::AlreadyMember(_)),
        "Re-login de miembro existente debe devolver AlreadyMember"
    );

    let total_after_relogin = UsersRepository::count(&pool).await.unwrap();
    assert_eq!(total_after_relogin, 5);

    // Cerramos el registro general
    SettingsRepository::set_registration_open(&pool, false)
        .await
        .unwrap();

    // Alumno nuevo intentando registrarse con registro cerrado -> Closed
    let closed_res = UsersRepository::try_admit(
        &pool,
        9999,
        "CIF9999",
        99999,
        "Nuevo Con Registro Cerrado",
        "token_closed",
    )
    .await
    .unwrap();
    assert_eq!(closed_res, AdmissionResult::Closed);

    // Alumno ya miembro re-logueándose con registro cerrado -> Permitido (AlreadyMember)
    let member_with_closed_reg = UsersRepository::try_admit(
        &pool,
        admitted_sample.chat_id,
        &admitted_sample.uam_cif,
        admitted_sample.uam_user_id,
        &admitted_sample.fullname,
        "token_refresh",
    )
    .await
    .unwrap();
    assert!(matches!(
        member_with_closed_reg,
        AdmissionResult::AlreadyMember(_)
    ));

    let pool = Arc::try_unwrap(pool).expect("Arc cleanup");
    cleanup_test_db(pool, db_path);
}

#[tokio::test]
async fn test_active_tokens_for_course_returns_donor_tokens() {
    let (pool, db_path) = setup_test_db("donors").await;

    // Crear curso
    CoursesRepository::upsert_course(&pool, 301, "Estructuras de Datos", Some("INF201"))
        .await
        .unwrap();

    // 1. Estudiante activo con wstoken válido
    UsersRepository::upsert_login(
        &pool,
        5001,
        "2301001",
        1001,
        "Estudiante Uno",
        "wstoken_alpha",
    )
    .await
    .unwrap();
    CoursesRepository::link_user(&pool, 5001, 301)
        .await
        .unwrap();

    // 2. Estudiante activo con wstoken válido
    UsersRepository::upsert_login(
        &pool,
        5002,
        "2301002",
        1002,
        "Estudiante Dos",
        "wstoken_beta",
    )
    .await
    .unwrap();
    CoursesRepository::link_user(&pool, 5002, 301)
        .await
        .unwrap();

    // 3. Estudiante inactivo (desactivado por token inválido)
    UsersRepository::upsert_login(
        &pool,
        5003,
        "2301003",
        1003,
        "Estudiante Tres",
        "wstoken_gamma",
    )
    .await
    .unwrap();
    CoursesRepository::link_user(&pool, 5003, 301)
        .await
        .unwrap();
    UsersRepository::mark_inactive(&pool, 5003, "invalid_token")
        .await
        .unwrap();

    // 4. Estudiante en otro curso distinto
    UsersRepository::upsert_login(
        &pool,
        5004,
        "2301004",
        1004,
        "Estudiante Cuatro",
        "wstoken_delta",
    )
    .await
    .unwrap();

    let donors = UsersRepository::active_tokens_for_course(&pool, 301)
        .await
        .unwrap();

    assert_eq!(
        donors.len(),
        2,
        "Solo deben retornar los estudiantes activos en el curso 301"
    );
    assert!(donors
        .iter()
        .any(|d| d.chat_id == 5001 && d.token == "wstoken_alpha"));
    assert!(donors
        .iter()
        .any(|d| d.chat_id == 5002 && d.token == "wstoken_beta"));

    cleanup_test_db(pool, db_path);
}

#[tokio::test]
async fn test_cif_transfer_when_quota_is_full() {
    let (pool, db_path) = setup_test_db("transfer").await;

    // Cupo máximo fijado en 2
    SettingsRepository::set_max_users(&pool, 2).await.unwrap();

    // Admitir dos usuarios llenando el cupo al 100% (2/2)
    let adm1 = UsersRepository::try_admit(&pool, 6001, "CIF_A", 10, "User A", "tok_a")
        .await
        .unwrap();
    assert!(matches!(adm1, AdmissionResult::Admitted(_)));

    let adm2 = UsersRepository::try_admit(&pool, 6002, "CIF_B", 20, "User B", "tok_b")
        .await
        .unwrap();
    assert!(matches!(adm2, AdmissionResult::Admitted(_)));

    assert_eq!(UsersRepository::count(&pool).await.unwrap(), 2);

    // Un tercer usuario con CIF diferente es rechazado por cupo lleno
    let adm3 = UsersRepository::try_admit(&pool, 6003, "CIF_C", 30, "User C", "tok_c")
        .await
        .unwrap();
    assert_eq!(adm3, AdmissionResult::Full);

    // Pero si el usuario A migra a un nuevo chat de Telegram con el mismo CIF ("CIF_A"):
    // Debe permitirse porque se elimina el registro previo antes del chequeo de cupo (A-12).
    let migration = UsersRepository::try_admit(
        &pool,
        6099, // Nuevo chat_id
        "CIF_A",
        10,
        "User A Migrado",
        "tok_a_nuevo",
    )
    .await
    .unwrap();

    assert!(
        matches!(migration, AdmissionResult::Admitted(_)),
        "El traspaso de CIF debe admitirse exitosamente aun con cupo al 100%"
    );

    // El chat anterior (6001) ya no debe existir
    assert!(UsersRepository::get(&pool, 6001).await.unwrap().is_none());

    // El nuevo chat (6099) debe existir
    let new_user = UsersRepository::get(&pool, 6099)
        .await
        .unwrap()
        .expect("Usuario migrado debe existir");
    assert_eq!(new_user.uam_cif, "CIF_A");
    assert_eq!(new_user.fullname, "User A Migrado");

    // Total de usuarios debe mantenerse exactamente en 2
    assert_eq!(UsersRepository::count(&pool).await.unwrap(), 2);

    cleanup_test_db(pool, db_path);
}
