use crate::moodle::client::MoodleClient;
use crate::moodle::error::Result;
use crate::moodle::gate::Priority;
use serde::Deserialize;

/// Estado de entrega, calificación y extensión de plazo de una tarea para un alumno (A-07, RF-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionStatus {
    /// Estado de la entrega (ej. "submitted", "draft", "new").
    pub status: Option<String>,
    /// Si la entrega ya ha sido calificada por el docente.
    pub graded: bool,
    /// Calificación textual (ej. "30 / 30" o "30.00").
    pub grade: Option<String>,
    /// Comentario docente reducido a texto plano (sin HTML) (A-13).
    pub feedback_text: Option<String>,
    /// Fecha de extensión individual concedida al alumno en segundos epoch (A-07).
    pub due_override: Option<i64>,
}

#[derive(Deserialize)]
struct SubmissionStatusResponse {
    lastattempt: Option<LastAttemptRaw>,
    feedback: Option<FeedbackRaw>,
}

#[derive(Deserialize)]
struct LastAttemptRaw {
    submission: Option<SubmissionRaw>,
    teamsubmission: Option<SubmissionRaw>,
    graded: Option<bool>,
    gradingstatus: Option<String>,
    extensionduedate: Option<i64>,
}

#[derive(Deserialize)]
struct SubmissionRaw {
    status: Option<String>,
}

#[derive(Deserialize)]
struct FeedbackRaw {
    grade: Option<GradeRaw>,
    gradefordisplay: Option<String>,
    plugins: Option<Vec<FeedbackPluginRaw>>,
}

#[derive(Deserialize)]
struct GradeRaw {
    grade: Option<String>,
}

#[derive(Deserialize)]
struct FeedbackPluginRaw {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    plugin_type: Option<String>,
    editorfields: Option<Vec<EditorFieldRaw>>,
}

#[derive(Deserialize)]
struct EditorFieldRaw {
    text: Option<String>,
}

/// Convierte contenido HTML de retroalimentación o entregas de Moodle a texto plano limpio (A-13).
pub fn strip_html_to_plain_text(html: &str) -> String {
    let text = html
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n\n")
        .replace("</div>", "\n")
        .replace("</li>", "\n");

    // Eliminar etiquetas HTML
    let mut in_tag = false;
    let mut clean = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            clean.push(ch);
        }
    }

    // Decodificar entidades comunes
    clean = clean
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");

    // Limpiar saltos de línea repetidos y recortar espacios
    let lines: Vec<&str> = clean
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

impl MoodleClient {
    /// Consulta el estado de entrega y calificación de un estudiante para una tarea (`mod_assign_get_submission_status`).
    pub async fn fetch_user_submission(
        &self,
        wstoken: &str,
        assign_id: i64,
        user_id: i64,
        priority: Priority,
    ) -> Result<SubmissionStatus> {
        let assign_id_str = assign_id.to_string();
        let user_id_str = user_id.to_string();
        let params = [
            ("wstoken", wstoken),
            ("wsfunction", "mod_assign_get_submission_status"),
            ("moodlewsrestformat", "json"),
            ("assignid", &assign_id_str),
            ("userid", &user_id_str),
        ];

        let raw: SubmissionStatusResponse = self.post_webservice(&params, priority).await?;

        let mut status = None;
        let mut graded = false;
        let mut due_override = None;

        if let Some(la) = raw.lastattempt {
            status = la
                .submission
                .and_then(|s| s.status)
                .or_else(|| la.teamsubmission.and_then(|ts| ts.status));
            graded = la.graded.unwrap_or(false) || la.gradingstatus.as_deref() == Some("graded");
            if let Some(ext) = la.extensionduedate {
                if ext > 0 {
                    due_override = Some(ext);
                }
            }
        }

        let mut grade = None;
        let mut feedback_text = None;

        if let Some(fb) = raw.feedback {
            grade = fb
                .gradefordisplay
                .filter(|g| !g.trim().is_empty())
                .or_else(|| fb.grade.and_then(|g| g.grade));

            if let Some(plugins) = fb.plugins {
                for plugin in plugins {
                    if let Some(fields) = plugin.editorfields {
                        for field in fields {
                            if let Some(raw_text) = field.text {
                                let stripped = strip_html_to_plain_text(&raw_text);
                                if !stripped.is_empty() {
                                    feedback_text = Some(stripped);
                                    break;
                                }
                            }
                        }
                    }
                    if feedback_text.is_some() {
                        break;
                    }
                }
            }
        }

        Ok(SubmissionStatus {
            status,
            graded,
            grade,
            feedback_text,
            due_override,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moodle::envelope::decode_envelope;

    #[test]
    fn test_strip_html_to_plain_text() {
        let html = "<p>Excelente trabajo.<br />Faltó incluir la bibliografía.</p>";
        let plain = strip_html_to_plain_text(html);
        assert_eq!(plain, "Excelente trabajo.\nFaltó incluir la bibliografía.");

        let html_entities = "<p>Puntaje:&nbsp;&gt;= 90% &amp; &lt; 100%</p>";
        let plain_entities = strip_html_to_plain_text(html_entities);
        assert_eq!(plain_entities, "Puntaje: >= 90% & < 100%");
    }

    #[test]
    fn test_decode_submission_fixture() {
        let fixture = include_str!("../../tests/fixtures/submission_status.json");
        let raw: SubmissionStatusResponse = decode_envelope(fixture).unwrap();

        assert!(raw.lastattempt.is_some());
        let la = raw.lastattempt.unwrap();
        assert_eq!(la.graded, Some(true));
        assert_eq!(la.gradingstatus.as_deref(), Some("graded"));

        assert!(raw.feedback.is_some());
        let fb = raw.feedback.unwrap();
        assert_eq!(fb.gradefordisplay.as_deref(), Some("30 / 30"));
    }
}
