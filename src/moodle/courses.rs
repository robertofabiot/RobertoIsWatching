use crate::moodle::client::MoodleClient;
use crate::moodle::envelope::decode_envelope;
use crate::moodle::error::{MoodleError, Result};
use crate::moodle::gate::Priority;
use crate::storage::DonorToken;
use chrono::Utc;
use serde::Deserialize;

/// Resultado de una consulta compartida con rotación de tokens donantes (A-07, A-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailoverOutcome<T> {
    /// Datos obtenidos de Moodle (ej. tareas o módulos del curso).
    pub data: T,
    /// Token del donante exitoso que respondió a la consulta.
    pub donor: DonorToken,
    /// Lista de `chat_id` de los donantes cuyos tokens fallaron con `is_token_error()`.
    /// El llamador debe marcar a estos usuarios como inactivos en base de datos.
    pub invalid: Vec<i64>,
}

/// Tarea retornado por `mod_assign_get_assignments`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MoodleAssignment {
    pub id: i64,
    pub cmid: Option<i64>,
    pub course: i64,
    pub name: String,
    pub duedate: i64,
    pub allowsubmissionsfromdate: Option<i64>,
    pub cutoffdate: Option<i64>,
    pub timemodified: Option<i64>,
}

/// Módulo o recurso de estudio extraído de `core_course_get_contents` (RF-06).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModuleInfo {
    pub id: i64,
    pub course_id: i64,
    pub section_name: Option<String>,
    pub name: String,
    pub mod_name: String,
    pub url: Option<String>,
}

#[derive(Deserialize)]
struct CourseSection {
    #[allow(dead_code)]
    id: i64,
    name: String,
    modules: Vec<CourseModuleRaw>,
}

#[derive(Deserialize)]
struct CourseModuleRaw {
    id: i64,
    name: String,
    modname: String,
    url: Option<String>,
}

#[derive(Deserialize)]
struct AssignmentsContainer {
    courses: Vec<CourseAssignmentsEntry>,
}

#[derive(Deserialize)]
struct CourseAssignmentsEntry {
    id: i64,
    assignments: Vec<MoodleAssignment>,
}

/// Curso matriculado retornado por Moodle `core_enrol_get_users_courses`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MoodleCourse {
    pub id: i64,
    pub shortname: Option<String>,
    pub fullname: String,
    pub displayname: Option<String>,
    pub visible: Option<i64>,
    pub enddate: Option<i64>,
    pub completed: Option<bool>,
    pub hidden: Option<bool>,
}

impl MoodleCourse {
    /// Determina si un curso se encuentra activo (no oculto, no terminado, visible).
    pub fn is_active(&self, now_timestamp: i64) -> bool {
        // visible == 0 indica curso ocultado por el docente o administración
        if self.visible == Some(0) {
            return false;
        }
        // hidden == true indica que el estudiante ocultó la tarjeta en su dashboard
        if self.hidden == Some(true) {
            return false;
        }
        // completed == true indica curso completado según criterios de finalización
        if self.completed == Some(true) {
            return false;
        }
        // enddate > 0 y menor que el momento actual indica semestre/periodo cerrado
        if let Some(end) = self.enddate {
            if end > 0 && end < now_timestamp {
                return false;
            }
        }
        true
    }
}

impl MoodleClient {
    /// Obtiene la lista de cursos del estudiante y filtra los cursos terminados u ocultos.
    pub async fn user_courses(
        &self,
        wstoken: &str,
        userid: i64,
        priority: Priority,
    ) -> Result<Vec<MoodleCourse>> {
        let rest_url = self.rest_url();
        let userid_str = userid.to_string();
        let params = [
            ("wstoken", wstoken),
            ("wsfunction", "core_enrol_get_users_courses"),
            ("moodlewsrestformat", "json"),
            ("userid", &userid_str),
        ];

        let _permit = self.gate().acquire(priority).await?;

        let response = self
            .http()
            .post(&rest_url)
            .form(&params)
            .send()
            .await
            .map_err(MoodleError::from)?;

        if response.status().as_u16() == 503 {
            return Err(MoodleError::Maintenance);
        }

        let body_text = response.text().await.map_err(MoodleError::from)?;
        let courses: Vec<MoodleCourse> = decode_envelope(&body_text)?;

        let now = Utc::now().timestamp();
        let active_courses: Vec<MoodleCourse> =
            courses.into_iter().filter(|c| c.is_active(now)).collect();

        Ok(active_courses)
    }

    /// Consulta las tareas de un curso utilizando un wstoken específico (`mod_assign_get_assignments`).
    pub async fn fetch_course_assignments(
        &self,
        wstoken: &str,
        course_id: i64,
        priority: Priority,
    ) -> Result<Vec<MoodleAssignment>> {
        let rest_url = self.rest_url();
        let course_id_str = course_id.to_string();
        let params = [
            ("wstoken", wstoken),
            ("wsfunction", "mod_assign_get_assignments"),
            ("moodlewsrestformat", "json"),
            ("courseids[0]", &course_id_str),
        ];

        let _permit = self.gate().acquire(priority).await?;

        let response = self
            .http()
            .post(&rest_url)
            .form(&params)
            .send()
            .await
            .map_err(MoodleError::from)?;

        if response.status().as_u16() == 503 {
            return Err(MoodleError::Maintenance);
        }

        let body_text = response.text().await.map_err(MoodleError::from)?;
        let container: AssignmentsContainer = decode_envelope(&body_text)?;

        let assignments = container
            .courses
            .into_iter()
            .find(|c| c.id == course_id)
            .map(|c| c.assignments)
            .unwrap_or_default();

        Ok(assignments)
    }

    /// Consulta las tareas de un curso iterando la lista de donantes con rotación y failover (A-07, A-14).
    /// Ante tokens inválidos acumula el `chat_id` en `invalid` y continúa con el siguiente compañero.
    pub async fn fetch_course_with_failover(
        &self,
        donors: &[DonorToken],
        course_id: i64,
        priority: Priority,
    ) -> Result<FailoverOutcome<Vec<MoodleAssignment>>> {
        if donors.is_empty() {
            return Err(MoodleError::Protocol(
                "No hay donantes disponibles para consultar el curso".into(),
            ));
        }

        let mut invalid = Vec::new();
        let mut last_err = None;

        for donor in donors {
            match self
                .fetch_course_assignments(&donor.token, course_id, priority)
                .await
            {
                Ok(assignments) => {
                    return Ok(FailoverOutcome {
                        data: assignments,
                        donor: donor.clone(),
                        invalid,
                    });
                }
                Err(err) => {
                    if err.is_token_error() {
                        tracing::warn!(
                            chat_id = donor.chat_id,
                            course_id = course_id,
                            "Token donante inválido o revocado. Rotando al siguiente donante..."
                        );
                        invalid.push(donor.chat_id);
                        last_err = Some(err);
                        continue;
                    } else {
                        last_err = Some(err);
                    }
                }
            }
        }

        Err(last_err.unwrap_or(MoodleError::InvalidToken))
    }

    /// Consulta los contenidos y recursos de un curso utilizando un wstoken específico (`core_course_get_contents`).
    pub async fn fetch_course_contents(
        &self,
        wstoken: &str,
        course_id: i64,
        priority: Priority,
    ) -> Result<Vec<ModuleInfo>> {
        let rest_url = self.rest_url();
        let course_id_str = course_id.to_string();
        let params = [
            ("wstoken", wstoken),
            ("wsfunction", "core_course_get_contents"),
            ("moodlewsrestformat", "json"),
            ("courseid", &course_id_str),
        ];

        let _permit = self.gate().acquire(priority).await?;

        let response = self
            .http()
            .post(&rest_url)
            .form(&params)
            .send()
            .await
            .map_err(MoodleError::from)?;

        if response.status().as_u16() == 503 {
            return Err(MoodleError::Maintenance);
        }

        let body_text = response.text().await.map_err(MoodleError::from)?;
        let sections: Vec<CourseSection> = decode_envelope(&body_text)?;

        let mut modules = Vec::new();
        for section in sections {
            let section_name = if section.name.trim().is_empty() {
                None
            } else {
                Some(section.name)
            };

            for m in section.modules {
                modules.push(ModuleInfo {
                    id: m.id,
                    course_id,
                    section_name: section_name.clone(),
                    name: m.name,
                    mod_name: m.modname,
                    url: m.url,
                });
            }
        }

        Ok(modules)
    }

    /// Consulta los contenidos de un curso con rotación y failover de donantes (RF-06).
    pub async fn fetch_course_contents_with_failover(
        &self,
        donors: &[DonorToken],
        course_id: i64,
        priority: Priority,
    ) -> Result<FailoverOutcome<Vec<ModuleInfo>>> {
        if donors.is_empty() {
            return Err(MoodleError::Protocol(
                "No hay donantes disponibles para consultar los contenidos del curso".into(),
            ));
        }

        let mut invalid = Vec::new();
        let mut last_err = None;

        for donor in donors {
            match self
                .fetch_course_contents(&donor.token, course_id, priority)
                .await
            {
                Ok(modules) => {
                    return Ok(FailoverOutcome {
                        data: modules,
                        donor: donor.clone(),
                        invalid,
                    });
                }
                Err(err) => {
                    if err.is_token_error() {
                        tracing::warn!(
                            chat_id = donor.chat_id,
                            course_id = course_id,
                            "Token donante inválido al obtener contenidos. Rotando al siguiente donante..."
                        );
                        invalid.push(donor.chat_id);
                        last_err = Some(err);
                        continue;
                    } else {
                        last_err = Some(err);
                    }
                }
            }
        }

        Err(last_err.unwrap_or(MoodleError::InvalidToken))
    }
}
