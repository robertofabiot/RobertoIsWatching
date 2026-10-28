use thiserror::Error;

/// Errores tipados del cliente Moodle (A-08).
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum MoodleError {
    /// Token inválido, expirado o revocado por Moodle (`invalidtoken`).
    #[error("Token de Moodle inválido o expirado")]
    InvalidToken,

    /// Credenciales incorrectas en el login de token (`invalidlogin`).
    #[error("Credenciales UAM inválidas")]
    InvalidLogin,

    /// Permisos insuficientes o excepción de acceso (`accessexception`).
    #[error("Acceso denegado en Moodle: {0}")]
    AccessDenied(String),

    /// Servidor en mantenimiento, error 503 o respuesta HTML inesperada.
    #[error("Moodle se encuentra en mantenimiento o no disponible")]
    Maintenance,

    /// Tiempo de espera agotado al conectar o recibir respuesta.
    #[error("Tiempo de espera agotado conectando a Moodle")]
    Timeout,

    /// Fallo de conexión TCP/TLS, DNS o caída de red.
    #[error("Error de red conectando a Moodle: {0}")]
    Network(String),

    /// Respuesta con formato JSON anómalo o violación del protocolo esperado.
    #[error("Error de protocolo Moodle: {0}")]
    Protocol(String),
}

impl MoodleError {
    /// Indica si el error se debe a un token no válido que amerita rotación o marcar
    /// al usuario donante como inactivo (A-08, A-14).
    pub fn is_token_error(&self) -> bool {
        matches!(self, MoodleError::InvalidToken)
    }

    /// Indica si el error es transitorio (caída temporal, timeout, red) y apto para
    /// reintento o para computar en el circuit breaker (A-17).
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            MoodleError::Timeout | MoodleError::Network(_) | MoodleError::Maintenance
        )
    }
}

pub type Result<T> = std::result::Result<T, MoodleError>;

impl From<reqwest::Error> for MoodleError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            MoodleError::Timeout
        } else if err.is_connect() || err.is_request() {
            MoodleError::Network(err.to_string())
        } else if let Some(status) = err.status() {
            if status.as_u16() == 503 || status.is_server_error() {
                MoodleError::Maintenance
            } else {
                MoodleError::Network(format!("HTTP {}", status))
            }
        } else {
            MoodleError::Network(err.to_string())
        }
    }
}
