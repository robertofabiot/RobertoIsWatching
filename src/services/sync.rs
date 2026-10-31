use crate::moodle::gate::Priority;
use crate::moodle::MoodleClient;
use crate::storage::assignments::UpsertAssignmentResult;
use crate::storage::submissions::{SubmissionData, UpsertSubmissionResult};
use crate::storage::{
    AssignmentsRepository, CoursesRepository, ModulesRepository, SubmissionsRepository,
    UsersRepository,
};
use sqlx::SqlitePool;

/// Resumen del resultado de una ejecución del pipeline de sincronización silencioso.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshSummary {
    pub courses_count: usize,
    pub assignments_count: usize,
    pub new_assignments: usize,
    pub updated_assignments: usize,
    pub new_grades: usize,
}

impl RefreshSummary {
    /// Determina si hubo novedades (nuevas tareas, cambios en plazos o nuevas calificaciones).
    pub fn has_changes(&self) -> bool {
        self.new_assignments > 0 || self.updated_assignments > 0 || self.new_grades > 0
    }
}

/// Pipeline de refresco silencioso por estudiante (A-03, A-04).
///
/// Flujo:
/// 1. Consulta cursos activos de Moodle con el token del alumno.
/// 2. Upsert de cursos y vinculación `user_courses`.
/// 3. Por cada curso, consulta tareas (`mod_assign_get_assignments`) y módulos (`core_course_get_contents`).
/// 4. Para tareas abiertas o no calificadas, consulta entrega del alumno (`mod_assign_get_submission_status`).
/// 5. Upsert de `assignments`, `user_submissions` y `modules`.
/// 6. Actualiza `courses.last_scanned = CURRENT_TIMESTAMP`.
///
/// **No emite alertas ni mensajes en Telegram**: Siembra la base de datos en silencio.
pub async fn refresh_user(
    pool: &SqlitePool,
    moodle: &MoodleClient,
    chat_id: i64,
    priority: Priority,
) -> Result<RefreshSummary, anyhow::Error> {
    let user = match UsersRepository::get(pool, chat_id).await? {
        Some(u) if u.is_active && !u.wstoken.is_empty() => u,
        Some(_) => anyhow::bail!("Usuario inactivo o sin token válido"),
        None => anyhow::bail!("Usuario no encontrado en la base de datos"),
    };

    let mut summary = RefreshSummary::default();

    // 1. Obtener cursos activos del estudiante en Moodle
    let moodle_courses = moodle
        .user_courses(&user.wstoken, user.uam_user_id, priority)
        .await?;
    summary.courses_count = moodle_courses.len();

    let now_epoch = chrono::Utc::now().timestamp();

    for mc in &moodle_courses {
        // 2. Persistir curso y vínculo con el alumno
        CoursesRepository::upsert_course(pool, mc.id, &mc.fullname, mc.shortname.as_deref())
            .await?;
        CoursesRepository::link_user(pool, chat_id, mc.id).await?;

        // Comprobar si el curso ya tiene línea base registrada por el scanner (R-02)
        let course_record = CoursesRepository::get(pool, mc.id).await?;
        let has_baseline = course_record
            .as_ref()
            .and_then(|c| c.last_scanned)
            .is_some();

        // 3. Tareas del curso
        let assignments = match moodle
            .fetch_course_assignments(&user.wstoken, mc.id, priority)
            .await
        {
            Ok(assigns) => assigns,
            Err(err) if err.is_token_error() => return Err(err.into()),
            Err(err) => {
                tracing::warn!(
                    course_id = mc.id,
                    error = %err,
                    "Error al obtener tareas del curso en Moodle, continuando con siguiente..."
                );
                continue;
            }
        };

        summary.assignments_count += assignments.len();

        for assign in assignments {
            // R-02: Si el curso ya tiene línea base, no consumir la novedad que le corresponde
            // alertar al scanner para el resto de alumnos. No insertar en `assignments` si no existía.
            let existing_assign = AssignmentsRepository::get(pool, assign.id).await?;
            if has_baseline {
                match existing_assign {
                    None => {
                        summary.new_assignments += 1;
                        continue;
                    }
                    Some(ref curr) => {
                        if curr.duedate != Some(assign.duedate) || curr.name != assign.name {
                            summary.updated_assignments += 1;
                        }
                    }
                }
            } else {
                // Upsert de la tarea solo si no hay línea base previa
                let assign_res = AssignmentsRepository::upsert(
                    pool,
                    assign.id,
                    assign.cmid,
                    mc.id,
                    &assign.name,
                    Some(assign.duedate),
                    assign.cutoffdate,
                )
                .await?;

                match assign_res {
                    UpsertAssignmentResult::Inserted(_) => {
                        summary.new_assignments += 1;
                    }
                    UpsertAssignmentResult::Updated { .. } => {
                        summary.updated_assignments += 1;
                    }
                    UpsertAssignmentResult::Unchanged(_) => {}
                }
            }

            // Verificar si necesitamos consultar el estado de entrega del alumno
            let existing_sub = SubmissionsRepository::get(pool, chat_id, assign.id).await?;
            let is_open = assign.duedate == 0
                || assign.duedate > now_epoch
                || assign.cutoffdate.map(|c| c > now_epoch).unwrap_or(false);
            let needs_grade = existing_sub.as_ref().map(|s| !s.graded).unwrap_or(true);

            if existing_sub.is_none() || is_open || needs_grade {
                match moodle
                    .fetch_user_submission(&user.wstoken, assign.id, user.uam_user_id, priority)
                    .await
                {
                    Ok(sub_status) => {
                        let sub_res = SubmissionsRepository::upsert(
                            pool,
                            SubmissionData {
                                chat_id,
                                assignment_id: assign.id,
                                status: sub_status.status.as_deref(),
                                graded: sub_status.graded,
                                grade: sub_status.grade.as_deref(),
                                feedback: sub_status.feedback_text.as_deref(),
                                due_override: sub_status.due_override,
                            },
                        )
                        .await?;

                        match sub_res {
                            UpsertSubmissionResult::Inserted(s) if s.graded => {
                                summary.new_grades += 1;
                            }
                            UpsertSubmissionResult::Updated { changes, .. }
                                if changes.grade_changed =>
                            {
                                summary.new_grades += 1;
                            }
                            _ => {}
                        }
                    }
                    Err(err) if err.is_token_error() => return Err(err.into()),
                    Err(err) => {
                        tracing::warn!(
                            assign_id = assign.id,
                            error = %err,
                            "Error al consultar entrega de alumno para tarea, continuando..."
                        );
                    }
                }
            }
        }

        // 4. Recursos y módulos del curso (core_course_get_contents)
        // R-02: Si el curso ya tiene línea base, no consumir módulos nuevos en refresh_user
        if !has_baseline {
            match moodle
                .fetch_course_contents(&user.wstoken, mc.id, priority)
                .await
            {
                Ok(modules) => {
                    for m in modules {
                        let _ = ModulesRepository::upsert_returning_new(
                            pool,
                            m.id,
                            mc.id,
                            m.section_name.as_deref(),
                            Some(&m.mod_name),
                            &m.name,
                            m.url.as_deref(),
                        )
                        .await;
                    }
                }
                Err(err) if err.is_token_error() => return Err(err.into()),
                Err(err) => {
                    tracing::warn!(
                        course_id = mc.id,
                        error = %err,
                        "Error al obtener contenidos del curso en Moodle, continuando..."
                    );
                }
            }
        }

        // 5. R-02: NO llamar a CoursesRepository::touch_scanned desde refresh_user;
        // el escáner periódico es el único que marca y actualiza last_scanned.
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::create_bot_with_url;
    use crate::config::Config;
    use crate::services::scanner::ScannerWorker;
    use crate::storage::AlertsRepository;
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
    async fn test_r02_sync_does_not_silence_scanner_alerts_for_other_students() {
        let moodle_server = MockServer::start().await;
        let telegram_server = MockServer::start().await;

        // 1. Mock de Telegram SendMessage
        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 42,
                    "date": 1700000000,
                    "chat": { "id": 2002, "type": "private" },
                    "text": "alerta"
                }
            })))
            .mount(&telegram_server)
            .await;

        // 2. Mock de Moodle: core_enrol_get_users_courses para alumno A (1001)
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=core_enrol_get_users_courses",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": 301,
                    "fullname": "Sistemas Operativos",
                    "shortname": "SO"
                }
            ])))
            .mount(&moodle_server)
            .await;

        // 3. Mock de Moodle: mod_assign_get_assignments con nueva tarea 501
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
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
                            "name": "Laboratorio 2 - Sockets",
                            "duedate": 1800000000
                        }
                    ]
                }]
            })))
            .mount(&moodle_server)
            .await;

        // 4. Mock de Moodle: core_course_get_contents (vacío de nuevos módulos)
        Mock::given(method("POST"))
            .and(path("/webservice/rest/server.php"))
            .and(wiremock::matchers::body_string_contains(
                "wsfunction=core_course_get_contents",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&moodle_server)
            .await;

        // 5. Mock de Moodle: fetch_user_submission para comprobar visibilidad de alumnos
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
        let db_path = std::env::temp_dir().join(format!("test_sync_r02_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let config = Arc::new(create_test_config(database_url));

        let pool = crate::storage::init(&config).await.unwrap();
        let bot = create_bot_with_url(
            &config.bot_token,
            reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
        );
        let moodle = MoodleClient::new(moodle_server.uri());

        // Registrar curso 301 y simular que YA cuenta con línea base previa
        CoursesRepository::upsert_course(&pool, 301, "Sistemas Operativos", Some("SO"))
            .await
            .unwrap();
        CoursesRepository::touch_scanned(&pool, 301).await.unwrap();

        let initial_course = CoursesRepository::get(&pool, 301).await.unwrap().unwrap();
        assert!(
            initial_course.last_scanned.is_some(),
            "El curso debe tener línea base previa"
        );
        let initial_last_scanned = initial_course.last_scanned;

        // Alumno A (1001) y Alumno B (2002) inscritos en el mismo curso 301
        UsersRepository::upsert_login(&pool, 1001, "24011001", 1001, "Alumno A", "token_student_a")
            .await
            .unwrap();
        CoursesRepository::link_user(&pool, 1001, 301)
            .await
            .unwrap();

        UsersRepository::upsert_login(&pool, 2002, "24012002", 2002, "Alumno B", "token_student_b")
            .await
            .unwrap();
        CoursesRepository::link_user(&pool, 2002, 301)
            .await
            .unwrap();

        // PASO 1: Alumno A ejecuta /sync (refresh_user) cuando hay una tarea nueva (501)
        let summary_a = refresh_user(&pool, &moodle, 1001, Priority::Interactive)
            .await
            .unwrap();
        assert_eq!(
            summary_a.new_assignments, 1,
            "El summary de /sync debe reportar 1 nueva tarea encontrada"
        );

        // R-02 Verificación 1: refresh_user NO debe insertar la tarea en SQLite assignments
        // para no matar la novedad del escáner para los demás alumnos
        let assign_in_db = AssignmentsRepository::get(&pool, 501).await.unwrap();
        assert!(
            assign_in_db.is_none(),
            "La tarea 501 no debe haber sido insertada por refresh_user si el curso ya tenía línea base"
        );

        // R-02 Verificación 2: refresh_user NO debe actualizar touch_scanned
        let course_after_sync = CoursesRepository::get(&pool, 301).await.unwrap().unwrap();
        assert_eq!(
            course_after_sync.last_scanned, initial_last_scanned,
            "refresh_user no debe llamar a touch_scanned ni alterar la marca de escaneo"
        );

        // PASO 2: Corre el scanner periódico en segundo plano
        let worker = ScannerWorker::new(pool.clone(), moodle.clone(), bot.clone(), config.clone())
            .with_delays(Duration::from_millis(50), Duration::from_millis(10));
        let scan_summary = worker.run_scan_cycle().await.unwrap();

        // R-02 Verificación 3: El scanner detecta la tarea nueva
        assert_eq!(
            scan_summary.new_assignments, 1,
            "El scanner debe detectar la tarea 501 como nueva inserción"
        );

        // R-02 Verificación 4: El Alumno B recibe su alerta (se reclama y envía en sent_alerts)
        let alert_b_key = format!("new_assign:{}:{}", 2002, 501);
        let alert_b_claimed = AlertsRepository::is_claimed(&pool, &alert_b_key)
            .await
            .unwrap();
        assert!(
            alert_b_claimed,
            "El Alumno B debe recibir su alerta de nueva tarea despachada por el scanner (R-02)"
        );

        // El Alumno A también recibe su alerta oficial con la fecha formateada
        let alert_a_key = format!("new_assign:{}:{}", 1001, 501);
        let alert_a_claimed = AlertsRepository::is_claimed(&pool, &alert_a_key)
            .await
            .unwrap();
        assert!(
            alert_a_claimed,
            "El Alumno A también debe recibir su notificación formal del scanner"
        );

        let _ = std::fs::remove_file(&db_path);
    }
}
