use crate::storage::models::UserSubmission;
use sqlx::SqlitePool;

/// Datos para inserción o actualización de la entrega de un alumno.
#[derive(Debug, Clone, Default)]
pub struct SubmissionData<'a> {
    pub chat_id: i64,
    pub assignment_id: i64,
    pub status: Option<&'a str>,
    pub graded: bool,
    pub grade: Option<&'a str>,
    pub feedback: Option<&'a str>,
    pub due_override: Option<i64>,
}

/// Cambios detectados en el estado de entrega o calificación de una tarea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionChanges {
    pub status_changed: bool,
    pub grade_changed: bool,
    pub feedback_changed: bool,
    pub due_override_changed: bool,
}

/// Resultado de la operación de inserción o actualización de la entrega del alumno.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpsertSubmissionResult {
    Inserted(UserSubmission),
    Updated {
        submission: UserSubmission,
        changes: SubmissionChanges,
    },
    Unchanged(UserSubmission),
}

/// Detalle enriquecido de una entrega calificada con información de curso y feedback (RF-05, RF-08).
#[derive(Debug, Clone, sqlx::FromRow, PartialEq, Eq)]
pub struct GradedSubmissionItem {
    pub assignment_id: i64,
    pub assignment_name: String,
    pub course_id: i64,
    pub course_name: String,
    pub grade: Option<String>,
    pub feedback: Option<String>,
    pub updated_at: chrono::NaiveDateTime,
}

/// Repositorio de operaciones para la tabla `user_submissions`.
pub struct SubmissionsRepository;

impl SubmissionsRepository {
    /// Obtiene el estado de entrega de una tarea para un estudiante específico.
    pub async fn get(
        pool: &SqlitePool,
        chat_id: i64,
        assignment_id: i64,
    ) -> Result<Option<UserSubmission>, sqlx::Error> {
        sqlx::query_as::<_, UserSubmission>(
            "SELECT chat_id, assignment_id, status, graded, grade, feedback, due_override, updated_at \
             FROM user_submissions WHERE chat_id = ? AND assignment_id = ?",
        )
        .bind(chat_id)
        .bind(assignment_id)
        .fetch_optional(pool)
        .await
    }

    /// Obtiene todas las entregas registradas para un estudiante.
    pub async fn get_for_user(
        pool: &SqlitePool,
        chat_id: i64,
    ) -> Result<Vec<UserSubmission>, sqlx::Error> {
        sqlx::query_as::<_, UserSubmission>(
            "SELECT chat_id, assignment_id, status, graded, grade, feedback, due_override, updated_at \
             FROM user_submissions WHERE chat_id = ? ORDER BY assignment_id ASC",
        )
        .bind(chat_id)
        .fetch_all(pool)
        .await
    }

    /// Obtiene las entregas ya calificadas del estudiante con nombre de tarea y curso (RF-08).
    pub async fn graded_for_user_detailed(
        pool: &SqlitePool,
        chat_id: i64,
    ) -> Result<Vec<GradedSubmissionItem>, sqlx::Error> {
        sqlx::query_as::<_, GradedSubmissionItem>(
            "SELECT us.assignment_id, a.name as assignment_name, c.id as course_id, c.fullname as course_name, \
                    us.grade, us.feedback, us.updated_at \
             FROM user_submissions us \
             INNER JOIN assignments a ON us.assignment_id = a.id \
             INNER JOIN courses c ON a.course_id = c.id \
             INNER JOIN user_courses uc ON c.id = uc.course_id AND us.chat_id = uc.chat_id \
             WHERE us.chat_id = ? AND (us.graded = 1 OR us.grade IS NOT NULL) \
             ORDER BY us.updated_at DESC",
        )
        .bind(chat_id)
        .fetch_all(pool)
        .await
    }

    /// Inserta o actualiza el estado de entrega de un alumno, detectando cambios en estado, nota y feedback.
    pub async fn upsert(
        pool: &SqlitePool,
        data: SubmissionData<'_>,
    ) -> Result<UpsertSubmissionResult, sqlx::Error> {
        let existing = Self::get(pool, data.chat_id, data.assignment_id).await?;

        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO user_submissions (chat_id, assignment_id, status, graded, grade, feedback, due_override, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
                )
                .bind(data.chat_id)
                .bind(data.assignment_id)
                .bind(data.status)
                .bind(data.graded)
                .bind(data.grade)
                .bind(data.feedback)
                .bind(data.due_override)
                .execute(pool)
                .await?;

                let inserted = Self::get(pool, data.chat_id, data.assignment_id)
                    .await?
                    .ok_or_else(|| sqlx::Error::RowNotFound)?;

                Ok(UpsertSubmissionResult::Inserted(inserted))
            }
            Some(curr) => {
                let status_changed = curr.status.as_deref() != data.status;
                let grade_changed =
                    curr.grade.as_deref() != data.grade || curr.graded != data.graded;
                let feedback_changed = curr.feedback.as_deref() != data.feedback;
                let due_override_changed = curr.due_override != data.due_override;

                if status_changed || grade_changed || feedback_changed || due_override_changed {
                    sqlx::query(
                        "UPDATE user_submissions SET \
                         status = ?, \
                         graded = ?, \
                         grade = ?, \
                         feedback = ?, \
                         due_override = ?, \
                         updated_at = CURRENT_TIMESTAMP \
                         WHERE chat_id = ? AND assignment_id = ?",
                    )
                    .bind(data.status)
                    .bind(data.graded)
                    .bind(data.grade)
                    .bind(data.feedback)
                    .bind(data.due_override)
                    .bind(data.chat_id)
                    .bind(data.assignment_id)
                    .execute(pool)
                    .await?;

                    let updated = Self::get(pool, data.chat_id, data.assignment_id)
                        .await?
                        .ok_or_else(|| sqlx::Error::RowNotFound)?;

                    Ok(UpsertSubmissionResult::Updated {
                        submission: updated,
                        changes: SubmissionChanges {
                            status_changed,
                            grade_changed,
                            feedback_changed,
                            due_override_changed,
                        },
                    })
                } else {
                    Ok(UpsertSubmissionResult::Unchanged(curr))
                }
            }
        }
    }
}
