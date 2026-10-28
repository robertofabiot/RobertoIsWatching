use crate::moodle::error::{MoodleError, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct MoodleErrorEnvelope {
    exception: Option<String>,
    error: Option<String>,
    errorcode: Option<String>,
    message: Option<String>,
}

/// Decodifica la respuesta en crudo de Moodle inspeccionando tanto la estructura JSON
/// como los errores devueltos dentro del envelope HTTP 200 (A-08).
pub fn decode_envelope<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T> {
    let trimmed = body.trim();

    // Si la respuesta es vacía o parece una página HTML (ej. mantenimiento de Moodle o error 503 proxied)
    if trimmed.is_empty()
        || trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || trimmed.starts_with('<')
    {
        return Err(MoodleError::Maintenance);
    }

    // Verificar si el JSON corresponde a un envelope de error de Moodle (A-08)
    if let Ok(err_envelope) = serde_json::from_str::<MoodleErrorEnvelope>(trimmed) {
        if err_envelope.exception.is_some() || err_envelope.errorcode.is_some() {
            let errorcode = err_envelope.errorcode.as_deref().unwrap_or("");
            let message = err_envelope
                .message
                .or(err_envelope.error)
                .unwrap_or_else(|| errorcode.to_string());

            return match errorcode {
                "invalidtoken" | "tokenexpired" | "invalidtokenexception" => {
                    Err(MoodleError::InvalidToken)
                }
                "invalidlogin" | "invalidcredentials" => Err(MoodleError::InvalidLogin),
                "accessexception" | "nopermissions" | "requireloginerror" => {
                    Err(MoodleError::AccessDenied(message))
                }
                "maintenance" => Err(MoodleError::Maintenance),
                _ => Err(MoodleError::Protocol(format!(
                    "Moodle exception: {} (errorcode: {})",
                    message, errorcode
                ))),
            };
        } else if let Some(_err_msg) = err_envelope.error {
            // Caso particular: token.php a veces devuelve solo "error" sin "exception"
            return Err(MoodleError::InvalidLogin);
        }
    }

    // Deserializar payload exitoso
    serde_json::from_str::<T>(trimmed)
        .map_err(|e| MoodleError::Protocol(format!("Fallo al deserializar JSON de Moodle: {e}")))
}
