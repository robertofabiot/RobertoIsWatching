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
            // Upsert de la tarea
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

        // 5. Actualizar timestamp de escaneo del curso
        CoursesRepository::touch_scanned(pool, mc.id).await?;
    }

    Ok(summary)
}
