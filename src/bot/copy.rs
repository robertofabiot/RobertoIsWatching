//! Textos canónicos de mensajería y copywriting del sistema (Plan Maestro §1.2).

use chrono::DateTime;
use chrono_tz::Tz;
use teloxide::utils::html::escape;

pub const START_MESSAGE: &str = "👁️ <b>Roberto's Watching.</b>\n\
Radar de entregas, notas y recursos de UAM Virtual.\n\n\
Para iniciar, vincula tu cuenta institucional:\n\
<code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code>\n\n\
<i>Tus credenciales nunca se guardan; se intercambian de inmediato por un token móvil y tu mensaje es eliminado del chat.</i>\n\n\
Escribe <code>/ayuda</code> para ver el catálogo de comandos disponibles.";

pub const HELP_MESSAGE: &str = "👁️ <b>Comandos Disponibles - Roberto's Watching:</b>\n\n\
<code>/login &lt;CIF&gt; &lt;CLAVE&gt;</code> - Vincula tu cuenta UAM Virtual.\n\
<code>/pendientes</code> - Lista de entregas y tareas pendientes.\n\
<code>/notas</code> - Desglose de calificaciones y feedback reciente.\n\
<code>/cursos</code> - Asignaturas matriculadas bajo vigilancia 24/7.\n\
<code>/sync</code> - Fuerza sincronización con Moodle (cooldown de 10 min).\n\
<code>/config</code> - Ajusta horarios y frecuencia de tus resúmenes.\n\
<code>/logout</code> - Cierra sesión y purga tus datos del sistema.\n\
<code>/ayuda</code> - Muestra este catálogo de ayuda.";

pub const WAITLIST_FULL: &str =
    "🛑 <b>Roberto's watching the server capacity.</b> El bot alcanzó el límite de estudiantes activos. Habla con Roberto en persona si quieres que te abra un cupo VIP.";

pub const TOKEN_EXPIRED_ALERT: &str =
    "🔑 <b>Roberto's watching an expired token.</b> Tu sesión de UAM Virtual caducó (o cambiaste tu clave). Escribe <code>/login &lt;CIF&gt; &lt;CLAVE&gt;</code> para reconectar el radar.";

pub const ONLY_PRIVATE_CHAT: &str =
    "⚠️ <b>Por tu seguridad, este comando solo puede utilizarse en un chat privado con el bot.</b> Nunca expongas credenciales en grupos.";

pub const LOGIN_USAGE: &str =
    "ℹ️ Uso: <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code>\n(Tu mensaje se eliminará automáticamente al instante).";

pub const LOGIN_RATE_LIMITED: &str =
    "⏳ <b>Demasiados intentos fallidos.</b> Por seguridad de tu cuenta UAM, espera 10 minutos antes de intentar de nuevo.";

pub const LOGIN_INVALID_CREDENTIALS: &str =
    "❌ Credenciales incorrectas de UAM Virtual. Verifica tu CIF y contraseña.";

pub const LOGIN_SUCCESS_PREFIX: &str =
    "✅ <b>¡Conexión exitosa!</b> Bienvenido a Roberto's Watching, ";

pub const LOGOUT_SUCCESS: &str =
    "👋 <b>Sesión cerrada con éxito.</b> Se han purgado todos tus datos y alertas pendientes. ¡Hasta la próxima!";

pub const LOGOUT_NOT_LOGGED_IN: &str = "ℹ️ No tienes ninguna sesión activa en Roberto's Watching.";

pub const UAM_MAINTENANCE: &str =
    "🔥 <b>Roberto's watching UAM Virtual burn.</b> Los servidores de la UAM dieron timeout o están en mantenimiento. Reintentando en segundo plano...";

// --- COMANDO /sync ---
pub const SYNC_SCANNING: &str =
    "👁️ <b>Roberto's watching the Moodle servers...</b> buscando cambios.";

pub const SYNC_NO_CHANGES: &str =
    "👁️ <b>Roberto's watching</b>, pero tus profesores siguen dormidos. Cero tareas nuevas.";

pub fn sync_cooldown_message(minutes: i64) -> String {
    format!(
        "👁️ <b>Roberto's watching your anxiety.</b> Este curso ya fue escaneado hace {} minutos y no hay cambios. Respira.",
        minutes
    )
}

// --- COMANDO /pendientes ---
pub const PENDIENTES_ALL_CLEAR: &str =
    "👁️ <b>Roberto's watching...</b> y por primera vez no debes nada. Milagro.";

pub fn pendientes_header(count: usize) -> String {
    format!(
        "👁️ <b>Roberto's watching your procrastination.</b> Tienes {} entregas pendientes:\n",
        count
    )
}

// --- COMANDO /notas ---
pub const NOTAS_HEADER: &str =
    "👁️ <b>Roberto's watching your GPA.</b> Aquí está el desglose de tus notas actuales:\n";

pub const NOTAS_EMPTY: &str =
    "👁️ <b>Roberto's watching your grades.</b> Aún no tienes calificaciones registradas en tus asignaturas activas.";

// --- COMANDO /cursos ---
pub const CURSOS_EMPTY: &str = "ℹ️ No tienes ninguna asignatura activa vinculada en este momento.";

pub fn cursos_header(count: usize) -> String {
    format!(
        "👁️ <b>Roberto's watching these {} courses 24/7:</b>\n",
        count
    )
}

// --- ALERTAS PROACTIVAS Y RESÚMENES ---
pub fn alert_new_assignment(task: &str, course: &str, due_date: &str) -> String {
    format!(
        "📢 <b>Roberto's watching a new assignment.</b> Tu profesor acaba de subir <b>[{}]</b> en <b>[{}]</b>. Vence el {}.",
        escape_html(task),
        escape_html(course),
        escape_html(due_date)
    )
}

pub fn alert_new_material(material: &str, course: &str, section: Option<&str>) -> String {
    let section_str = section
        .map(|s| format!(" en la sección <i>{}</i>", escape_html(s)))
        .unwrap_or_default();
    format!(
        "📚 <b>Roberto's watching new materials.</b> Tu profesor acaba de subir <b>[{}]</b> en <b>[{}]</b>{}.",
        escape_html(material),
        escape_html(course),
        section_str
    )
}

pub fn alert_deadline_24h(task: &str) -> String {
    format!(
        "⏰ <b>Roberto's watching the clock (Quedan 24h).</b> Te queda exactamente 1 día para entregar <b>[{}]</b>. Ve abriendo el documento.",
        escape_html(task)
    )
}

pub fn alert_deadline_2h(task: &str, due_time: &str) -> String {
    format!(
        "🚨 <b>ROBERTO'S WATCHING YOU PANIC (Quedan 2h).</b> Si no subes <b>[{}]</b> antes de las {}, se cierra la plataforma. Corre.",
        escape_html(task),
        escape_html(due_time)
    )
}

pub fn alert_new_grade(task: &str, grade: &str, feedback: Option<&str>) -> String {
    let fb_str = feedback
        .map(clean_feedback_for_html)
        .filter(|s| !s.is_empty())
        .map(|s| format!("\nComentario: <i>{}</i>", s))
        .unwrap_or_default();

    format!(
        "📝 <b>Roberto's watching your grades.</b> Ya calificaron <b>[{}]</b>. Nota: <b>{}</b>.{}",
        escape_html(task),
        escape_html(grade),
        fb_str
    )
}

/// Escapa caracteres especiales de HTML (<, >, &) para compatibilidad segura con Telegram ParseMode::Html (A-13).
pub fn escape_html(input: &str) -> String {
    escape(input)
}

/// Limpia y escapa el texto de retroalimentación de docentes, truncando comentarios excesivos.
pub fn clean_feedback_for_html(feedback: &str) -> String {
    let plain = crate::moodle::submissions::strip_html_to_plain_text(feedback);
    let trimmed = plain.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut truncated = trimmed;
    if truncated.chars().count() > 300 {
        // Truncar de forma segura en caracteres UTF-8
        let byte_idx = truncated
            .char_indices()
            .nth(300)
            .map(|(i, _)| i)
            .unwrap_or(truncated.len());
        truncated = &truncated[..byte_idx];
        format!("{}...", escape_html(truncated))
    } else {
        escape_html(truncated)
    }
}

/// Formatea una fecha epoch a cadena local legible en la zona horaria del bot (`BOT_TZ`).
pub fn format_epoch_local(epoch_secs: i64, tz: Tz) -> String {
    if let Some(utc_dt) = DateTime::from_timestamp(epoch_secs, 0) {
        let local_dt = utc_dt.with_timezone(&tz);
        local_dt.format("%d/%m/%Y %H:%M").to_string()
    } else {
        "Sin fecha".to_string()
    }
}

/// Formatea una fecha epoch con solo hora y minutos en la zona horaria del bot (`BOT_TZ`).
pub fn format_epoch_time_only(epoch_secs: i64, tz: Tz) -> String {
    if let Some(utc_dt) = DateTime::from_timestamp(epoch_secs, 0) {
        let local_dt = utc_dt.with_timezone(&tz);
        local_dt.format("%H:%M").to_string()
    } else {
        "Sin hora".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_html_entities() {
        assert_eq!(
            escape_html("Laboratorio <1> & Final > 2"),
            "Laboratorio &lt;1&gt; &amp; Final &gt; 2"
        );
    }

    #[test]
    fn test_clean_feedback_for_html() {
        let raw = "<p>Excelente trabajo.<br>Revisar punto <strong>2</strong>.</p>";
        let cleaned = clean_feedback_for_html(raw);
        assert_eq!(cleaned, "Excelente trabajo.\nRevisar punto 2.");

        let with_special = "<p>Muy bien &amp; felicitaciones</p>";
        assert_eq!(
            clean_feedback_for_html(with_special),
            "Muy bien &amp; felicitaciones"
        );
    }

    #[test]
    fn test_format_epoch_local() {
        let tz = chrono_tz::America::Managua;
        // 1700000000 = 2023-11-14 22:13:20 UTC = 2023-11-14 16:13:20 CST
        let formatted = format_epoch_local(1700000000, tz);
        assert_eq!(formatted, "14/11/2023 16:13");
    }

    #[test]
    fn test_canonical_copywriting_phrases() {
        assert!(SYNC_SCANNING.contains("buscando cambios"));
        assert!(SYNC_NO_CHANGES.contains("tus profesores siguen dormidos"));
        assert!(sync_cooldown_message(5).contains("hace 5 minutos y no hay cambios. Respira."));
        assert!(PENDIENTES_ALL_CLEAR.contains("por primera vez no debes nada. Milagro."));
    }
}
