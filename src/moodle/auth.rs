use crate::moodle::client::MoodleClient;
use crate::moodle::envelope::decode_envelope;
use crate::moodle::error::{MoodleError, Result};
use crate::moodle::gate::Priority;
use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

/// Información resultante de una autenticación exitosa en Moodle (A-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginInfo {
    pub token: String,
    pub userid: i64,
    pub fullname: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    token: Option<String>,
}

/// Datos del sitio y del usuario devueltos por `core_webservice_get_site_info`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SiteInfo {
    pub sitename: Option<String>,
    pub username: Option<String>,
    pub firstname: Option<String>,
    pub lastname: Option<String>,
    pub fullname: String,
    pub userid: i64,
}

// Estructura para POST a token.php.
// PROHIBIDO derivar Debug para asegurar que la clave nunca se imprima accidentalmente (A-09).
#[derive(serde::Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
    service: &'a str,
}

impl MoodleClient {
    /// Autentica a un estudiante contra Moodle intercambiando credenciales por un wstoken móvil (A-09).
    ///
    /// 1. Envía POST a `login/token.php` con `username`, `password` y `service="moodle_mobile_app"`.
    /// 2. Llama a `.zeroize()` explícitamente sobre la contraseña en cuanto se recibe la respuesta.
    /// 3. Llama a `core_webservice_get_site_info` con el token obtenido para rescatar `userid` y `fullname`.
    pub async fn login(&self, cif: &str, mut pass: Zeroizing<String>) -> Result<LoginInfo> {
        if self.breaker().is_open() {
            return Err(MoodleError::Maintenance);
        }

        tracing::info!(cif = %cif, "Iniciando autenticación de usuario en Moodle");

        let token_url = self.token_url();
        let login_body = LoginRequest {
            username: cif,
            password: pass.as_str(),
            service: "moodle_mobile_app",
        };

        // Adquirir permiso de gate con prioridad interactiva y liberarlo antes de get_site_info (R-01)
        let (status, body_text) = {
            let _permit = self.gate().acquire(Priority::Interactive).await?;
            self.counters().inc(Priority::Interactive);
            let res = self.http().post(&token_url).form(&login_body).send().await;

            // Sobrescribir inmediatamente la contraseña en memoria (RF-01, A-09)
            pass.zeroize();

            let response = match res {
                Ok(resp) => resp,
                Err(err) => {
                    let moodle_err = MoodleError::from(err);
                    if moodle_err.is_transient() {
                        self.breaker().record_failure();
                    }
                    return Err(moodle_err);
                }
            };

            let status = response.status();
            let body_text = response.text().await.map_err(MoodleError::from)?;
            (status, body_text)
        };

        if status.as_u16() == 503 || status.is_server_error() {
            self.breaker().record_failure();
            return Err(MoodleError::Maintenance);
        }

        let token_res: TokenResponse = decode_envelope(&body_text)?;
        self.breaker().record_success();

        let wstoken = token_res.token.ok_or_else(|| {
            MoodleError::Protocol("Respuesta de token.php no contiene token".into())
        })?;

        // 2da llamada: obtener site_info para extraer userid y fullname.
        // El permiso previo ya fue liberado para evitar deadlock con concurrencia restringida (R-01).
        let site_info = self.get_site_info(&wstoken, Priority::Interactive).await?;

        // R-15: Evitar loguear fullname (PII) en nivel info
        tracing::info!(
            cif = %cif,
            userid = site_info.userid,
            "Autenticación exitosa en Moodle"
        );
        tracing::debug!(
            cif = %cif,
            userid = site_info.userid,
            fullname = %site_info.fullname,
            "Detalles de sesión autenticada en Moodle"
        );

        Ok(LoginInfo {
            token: wstoken,
            userid: site_info.userid,
            fullname: site_info.fullname,
        })
    }

    /// Obtiene información básica del sitio y del usuario llamando a `core_webservice_get_site_info`.
    pub async fn get_site_info(&self, wstoken: &str, priority: Priority) -> Result<SiteInfo> {
        let params = [
            ("wstoken", wstoken),
            ("wsfunction", "core_webservice_get_site_info"),
            ("moodlewsrestformat", "json"),
        ];

        self.post_webservice(&params, priority).await
    }
}
