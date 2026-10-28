use crate::moodle::client::MoodleClient;
use crate::moodle::envelope::decode_envelope;
use crate::moodle::error::{MoodleError, Result};
use crate::moodle::gate::Priority;
use chrono::Utc;
use serde::Deserialize;

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
}
