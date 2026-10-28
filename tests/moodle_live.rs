use robertos_watching_bot::moodle::{MoodleClient, Priority};
use std::env;

#[tokio::test]
#[ignore = "Requiere conectividad con UAM Virtual y UAM_TEST_WSTOKEN configurado"]
async fn test_moodle_live_smoke() {
    dotenvy::dotenv().ok();

    let base_url = env::var("UAM_BASE_URL")
        .unwrap_or_else(|_| "https://uamvirtual.uam.edu.ni/grado".to_string());
    let token = match env::var("UAM_TEST_WSTOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            eprintln!("Omitiendo prueba en vivo: UAM_TEST_WSTOKEN no configurado en entorno");
            return;
        }
    };

    let client = MoodleClient::new(&base_url);

    // 1. Obtener site_info
    let site_info = client
        .get_site_info(&token, Priority::Interactive)
        .await
        .expect("Fallo al consultar site_info en Moodle");

    assert!(!site_info.fullname.is_empty());
    assert!(site_info.userid > 0);
    println!("Sitio Moodle OK: Usuario conectado id={}", site_info.userid);

    // 2. Obtener cursos del usuario
    let courses = client
        .user_courses(&token, site_info.userid, Priority::Interactive)
        .await
        .expect("Fallo al consultar cursos de usuario en Moodle");

    println!("Cursos activos encontrados: {}", courses.len());
    for course in &courses {
        println!(" - [id: {}] {}", course.id, course.fullname);
    }

    // 3. Si tiene al menos un curso, consultar tareas y contenidos
    if let Some(course) = courses.first() {
        let assignments = client
            .fetch_course_assignments(&token, course.id, Priority::Interactive)
            .await
            .expect("Fallo al consultar tareas del curso");

        println!(
            "Tareas encontradas en curso {}: {}",
            course.id,
            assignments.len()
        );

        let contents = client
            .fetch_course_contents(&token, course.id, Priority::Interactive)
            .await
            .expect("Fallo al consultar contenidos del curso");

        println!(
            "Módulos/Recursos encontrados en curso {}: {}",
            course.id,
            contents.len()
        );

        // 4. Si tiene tareas, consultar entrega de la primera
        if let Some(assign) = assignments.first() {
            let submission = client
                .fetch_user_submission(&token, assign.id, site_info.userid, Priority::Interactive)
                .await
                .expect("Fallo al consultar entrega de tarea");

            println!(
                "Estado de entrega tarea {}: status={:?}, graded={}",
                assign.id, submission.status, submission.graded
            );
        }
    }
}
