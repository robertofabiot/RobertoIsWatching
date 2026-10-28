use crate::storage::models::Assignment;
use sqlx::SqlitePool;

/// Detalles de campos modificados durante una actualización de tarea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentChanges {
    pub name_changed: bool,
    pub duedate_changed: bool,
    pub cutoffdate_changed: bool,
}

/// Resultado de la operación de inserción/actualización de tarea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpsertAssignmentResult {
    Inserted(Assignment),
    Updated {
        assignment: Assignment,
        changes: AssignmentChanges,
    },
    Unchanged(Assignment),
}

/// Detalle enriquecido de una tarea pendiente con información de su curso y extensiones (A-07).
#[derive(Debug, Clone, sqlx::FromRow, PartialEq, Eq)]
pub struct PendingAssignmentItem {
    pub id: i64,
    pub name: String,
    pub course_id: i64,
    pub course_name: String,
    pub course_shortname: Option<String>,
    pub duedate: Option<i64>,
    pub due_override: Option<i64>,
}

impl PendingAssignmentItem {
    pub fn effective_duedate(&self) -> Option<i64> {
        self.due_override.or(self.duedate)
    }

    pub fn is_extended(&self) -> bool {
        self.due_override.is_some()
    }
}

/// Repositorio de operaciones para la tabla `assignments`.
pub struct AssignmentsRepository;

impl AssignmentsRepository {
    /// Obtiene una tarea por su `id`.
    pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<Assignment>, sqlx::Error> {
        sqlx::query_as::<_, Assignment>(
            "SELECT id, cmid, course_id, name, duedate, cutoffdate, last_updated_at \
             FROM assignments WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Obtiene todas las tareas asociadas a un curso.
    pub async fn get_by_course(
        pool: &SqlitePool,
        course_id: i64,
    ) -> Result<Vec<Assignment>, sqlx::Error> {
        sqlx::query_as::<_, Assignment>(
            "SELECT id, cmid, course_id, name, duedate, cutoffdate, last_updated_at \
             FROM assignments WHERE course_id = ? ORDER BY duedate ASC",
        )
        .bind(course_id)
        .fetch_all(pool)
        .await
    }

    /// Inserta o actualiza una asignación detectando si fue insertada, modificada o quedó sin cambios.
    pub async fn upsert(
        pool: &SqlitePool,
        id: i64,
        cmid: Option<i64>,
        course_id: i64,
        name: &str,
        duedate: Option<i64>,
        cutoffdate: Option<i64>,
    ) -> Result<UpsertAssignmentResult, sqlx::Error> {
        let existing = Self::get(pool, id).await?;

        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO assignments (id, cmid, course_id, name, duedate, cutoffdate, last_updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
                )
                .bind(id)
                .bind(cmid)
                .bind(course_id)
                .bind(name)
                .bind(duedate)
                .bind(cutoffdate)
                .execute(pool)
                .await?;

                let inserted = Self::get(pool, id)
                    .await?
                    .ok_or_else(|| sqlx::Error::RowNotFound)?;

                Ok(UpsertAssignmentResult::Inserted(inserted))
            }
            Some(curr) => {
                let name_changed = curr.name != name;
                let duedate_changed = curr.duedate != duedate;
                let cutoffdate_changed = curr.cutoffdate != cutoffdate;
                let cmid_changed = curr.cmid != cmid;

                if name_changed || duedate_changed || cutoffdate_changed || cmid_changed {
                    sqlx::query(
                        "UPDATE assignments SET \
                         cmid = ?, \
                         course_id = ?, \
                         name = ?, \
                         duedate = ?, \
                         cutoffdate = ?, \
                         last_updated_at = CURRENT_TIMESTAMP \
                         WHERE id = ?",
                    )
                    .bind(cmid)
                    .bind(course_id)
                    .bind(name)
                    .bind(duedate)
                    .bind(cutoffdate)
                    .bind(id)
                    .execute(pool)
                    .await?;

                    let updated = Self::get(pool, id)
                        .await?
                        .ok_or_else(|| sqlx::Error::RowNotFound)?;

                    Ok(UpsertAssignmentResult::Updated {
                        assignment: updated,
                        changes: AssignmentChanges {
                            name_changed,
                            duedate_changed,
                            cutoffdate_changed,
                        },
                    })
                } else {
                    Ok(UpsertAssignmentResult::Unchanged(curr))
                }
            }
        }
    }

    /// Retorna las tareas abiertas (pendientes de entrega y no vencidas) para un estudiante.
    /// Si `reference_epoch` es provisto, filtra aquellas cuyo plazo sea superior a dicha fecha.
    pub async fn open_for_user(
        pool: &SqlitePool,
        chat_id: i64,
        reference_epoch: Option<i64>,
    ) -> Result<Vec<Assignment>, sqlx::Error> {
        let epoch = reference_epoch.unwrap_or_else(|| chrono::Utc::now().timestamp());

        sqlx::query_as::<_, Assignment>(
            "SELECT a.id, a.cmid, a.course_id, a.name, a.duedate, a.cutoffdate, a.last_updated_at \
             FROM assignments a \
             INNER JOIN user_courses uc ON a.course_id = uc.course_id \
             LEFT JOIN user_submissions us ON a.id = us.assignment_id AND us.chat_id = uc.chat_id \
             WHERE uc.chat_id = ? \
               AND (a.duedate IS NULL OR a.duedate > ?) \
               AND (us.status IS NULL OR us.status != 'submitted') \
             ORDER BY a.duedate ASC",
        )
        .bind(chat_id)
        .bind(epoch)
        .fetch_all(pool)
        .await
    }

    /// Retorna las tareas pendientes detalladas para el alumno con información de asignatura y extensiones (A-07).
    pub async fn pending_detailed_for_user(
        pool: &SqlitePool,
        chat_id: i64,
        reference_epoch: Option<i64>,
    ) -> Result<Vec<PendingAssignmentItem>, sqlx::Error> {
        let epoch = reference_epoch.unwrap_or_else(|| chrono::Utc::now().timestamp());

        sqlx::query_as::<_, PendingAssignmentItem>(
            "SELECT a.id, a.name, a.course_id, c.fullname as course_name, c.shortname as course_shortname, \
                    a.duedate, us.due_override \
             FROM assignments a \
             INNER JOIN courses c ON a.course_id = c.id \
             INNER JOIN user_courses uc ON a.course_id = uc.course_id \
             LEFT JOIN user_submissions us ON a.id = us.assignment_id AND us.chat_id = uc.chat_id \
             WHERE uc.chat_id = ? \
               AND (COALESCE(us.due_override, a.duedate) IS NULL OR COALESCE(us.due_override, a.duedate) = 0 OR COALESCE(us.due_override, a.duedate) > ?) \
               AND (us.status IS NULL OR us.status != 'submitted') \
             ORDER BY COALESCE(NULLIF(us.due_override, 0), NULLIF(a.duedate, 0)) ASC",
        )
        .bind(chat_id)
        .bind(epoch)
        .fetch_all(pool)
        .await
    }
}
