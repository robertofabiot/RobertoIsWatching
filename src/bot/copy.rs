//! Textos canónicos de mensajería y copywriting del sistema (Plan Maestro §1.2).

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
