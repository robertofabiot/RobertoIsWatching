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
        tracing::info!(cif = %cif, "Iniciando autenticación de usuario en Moodle");

        let token_url = self.token_url();
        let login_body = LoginRequest {
            username: cif,
            password: pass.as_str(),
            service: "moodle_mobile_app",
        };

        // Adquirir permiso de gate con prioridad interactiva
        let _permit = self.gate().acquire(Priority::Interactive).await?;

        let res = self.http().post(&token_url).form(&login_body).send().await;

        // Sobrescribir inmediatamente la contraseña en memoria (RF-01, A-09)
        pass.zeroize();

        let response = res.map_err(MoodleError::from)?;
        let status = response.status();
        let body_text = response.text().await.map_err(MoodleError::from)?;

        if status.as_u16() == 503 {
            return Err(MoodleError::Maintenance);
        }

        let token_res: TokenResponse = decode_envelope(&body_text)?;
        let wstoken = token_res.token.ok_or_else(|| {
            MoodleError::Protocol("Respuesta de token.php no contiene token".into())
        })?;

        // 2da llamada: obtener site_info para extraer userid y fullname
        let site_info = self.get_site_info(&wstoken, Priority::Interactive).await?;

        tracing::info!(
            cif = %cif,
            userid = site_info.userid,
            fullname = %site_info.fullname,
            "Autenticación exitosa en Moodle"
        );

        Ok(LoginInfo {
            token: wstoken,
            userid: site_info.userid,
            fullname: site_info.fullname,
        })
    }

    /// Obtiene información básica del sitio y del usuario llamando a `core_webservice_get_site_info`.
    pub async fn get_site_info(&self, wstoken: &str, priority: Priority) -> Result<SiteInfo> {
        let rest_url = self.rest_url();
        let params = [
            ("wstoken", wstoken),
            ("wsfunction", "core_webservice_get_site_info"),
            ("moodlewsrestformat", "json"),
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
        decode_envelope::<SiteInfo>(&body_text)
    }
}
