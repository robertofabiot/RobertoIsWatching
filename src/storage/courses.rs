use crate::storage::models::Course;
use chrono::NaiveDateTime;
use sqlx::SqlitePool;

/// Repositorio de operaciones para las tablas `courses` y `user_courses`.
pub struct CoursesRepository;

impl CoursesRepository {
    /// Inserta o actualiza los datos de un curso (preservando last_scanned si ya existía).
    pub async fn upsert_course(
        pool: &SqlitePool,
        id: i64,
        fullname: &str,
        shortname: Option<&str>,
    ) -> Result<Course, sqlx::Error> {
        sqlx::query(
            "INSERT INTO courses (id, fullname, shortname, last_scanned) \
             VALUES (?, ?, ?, NULL) \
             ON CONFLICT(id) DO UPDATE SET \
                 fullname = excluded.fullname, \
                 shortname = COALESCE(excluded.shortname, courses.shortname)",
        )
        .bind(id)
        .bind(fullname)
        .bind(shortname)
        .execute(pool)
        .await?;

        sqlx::query_as::<_, Course>(
            "SELECT id, fullname, shortname, last_scanned FROM courses WHERE id = ?",
        )
        .bind(id)
        .fetch_one(pool)
        .await
    }

    /// Obtiene un curso por su `id`.
    pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<Course>, sqlx::Error> {
        sqlx::query_as::<_, Course>(
            "SELECT id, fullname, shortname, last_scanned FROM courses WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Vincula un alumno a un curso matriculado (user_courses).
    pub async fn link_user(
        pool: &SqlitePool,
        chat_id: i64,
        course_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT OR IGNORE INTO user_courses (chat_id, course_id) VALUES (?, ?)")
            .bind(chat_id)
            .bind(course_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Desvincula un alumno de un curso.
    pub async fn unlink_user(
        pool: &SqlitePool,
        chat_id: i64,
        course_id: i64,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM user_courses WHERE chat_id = ? AND course_id = ?")
            .bind(chat_id)
            .bind(course_id)
            .execute(pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Obtiene los cursos asociados a un estudiante ordenados por nombre.
    pub async fn courses_for_user(
        pool: &SqlitePool,
        chat_id: i64,
    ) -> Result<Vec<Course>, sqlx::Error> {
        sqlx::query_as::<_, Course>(
            "SELECT c.id, c.fullname, c.shortname, c.last_scanned \
             FROM courses c \
             INNER JOIN user_courses uc ON c.id = uc.course_id \
             WHERE uc.chat_id = ? \
             ORDER BY c.fullname ASC",
        )
        .bind(chat_id)
        .fetch_all(pool)
        .await
    }

    /// Retorna la lista única de cursos que tienen al menos un alumno activo registrado.
    /// Esta es la lista que procesa el Shared Course Scanner para no escanear cursos vacíos.
    pub async fn unique_active_courses(pool: &SqlitePool) -> Result<Vec<Course>, sqlx::Error> {
        sqlx::query_as::<_, Course>(
            "SELECT DISTINCT c.id, c.fullname, c.shortname, c.last_scanned \
             FROM courses c \
             INNER JOIN user_courses uc ON c.id = uc.course_id \
             INNER JOIN users u ON uc.chat_id = u.chat_id \
             WHERE u.is_active = 1 \
             ORDER BY c.id ASC",
        )
        .fetch_all(pool)
        .await
    }

    /// Actualiza la marca de tiempo de último escaneo del curso a la hora actual.
    pub async fn touch_scanned(pool: &SqlitePool, course_id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE courses SET last_scanned = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(course_id)
            .execute(pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Retorna el timestamp del escaneo más reciente entre todos los cursos del estudiante.
    /// Alimenta la comprobación de cooldown de 10 min para el comando `/sync` interactivo.
    pub async fn latest_scan_for_user(
        pool: &SqlitePool,
        chat_id: i64,
    ) -> Result<Option<NaiveDateTime>, sqlx::Error> {
        let row: (Option<NaiveDateTime>,) = sqlx::query_as(
            "SELECT MAX(c.last_scanned) \
             FROM courses c \
             INNER JOIN user_courses uc ON c.id = uc.course_id \
             WHERE uc.chat_id = ?",
        )
        .bind(chat_id)
        .fetch_one(pool)
        .await?;

        Ok(row.0)
    }
}
