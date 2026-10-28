use crate::storage::models::Module;
use sqlx::SqlitePool;

/// Repositorio de operaciones para la tabla `modules` (materiales y recursos de cursos).
pub struct ModulesRepository;

impl ModulesRepository {
    /// Obtiene un recurso/módulo por su `id`.
    pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<Module>, sqlx::Error> {
        sqlx::query_as::<_, Module>(
            "SELECT id, course_id, section_name, modname, name, url, created_at \
             FROM modules WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Obtiene todos los módulos asociados a un curso.
    pub async fn get_by_course(
        pool: &SqlitePool,
        course_id: i64,
    ) -> Result<Vec<Module>, sqlx::Error> {
        sqlx::query_as::<_, Module>(
            "SELECT id, course_id, section_name, modname, name, url, created_at \
             FROM modules WHERE course_id = ? ORDER BY id ASC",
        )
        .bind(course_id)
        .fetch_all(pool)
        .await
    }

    /// Inserta o actualiza un módulo de curso (RF-06).
    /// Retorna `Some(Module)` si el módulo es completamente nuevo para emitir alerta de nuevo material,
    /// o `None` si el módulo ya había sido registrado previamente.
    pub async fn upsert_returning_new(
        pool: &SqlitePool,
        id: i64,
        course_id: i64,
        section_name: Option<&str>,
        modname: Option<&str>,
        name: &str,
        url: Option<&str>,
    ) -> Result<Option<Module>, sqlx::Error> {
        let existing = Self::get(pool, id).await?;

        if existing.is_some() {
            sqlx::query(
                "UPDATE modules SET \
                 course_id = ?, \
                 section_name = ?, \
                 modname = ?, \
                 name = ?, \
                 url = ? \
                 WHERE id = ?",
            )
            .bind(course_id)
            .bind(section_name)
            .bind(modname)
            .bind(name)
            .bind(url)
            .bind(id)
            .execute(pool)
            .await?;

            Ok(None)
        } else {
            sqlx::query(
                "INSERT INTO modules (id, course_id, section_name, modname, name, url, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
            )
            .bind(id)
            .bind(course_id)
            .bind(section_name)
            .bind(modname)
            .bind(name)
            .bind(url)
            .execute(pool)
            .await?;

            let new_mod = Self::get(pool, id)
                .await?
                .ok_or_else(|| sqlx::Error::RowNotFound)?;

            Ok(Some(new_mod))
        }
    }
}
