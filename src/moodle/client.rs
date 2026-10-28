use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://uamvirtual.uam.edu.ni/grado";
pub const DEFAULT_USER_AGENT: &str =
    "RobertoIsWatchingBot/0.2.0 (UAM Virtual Companion; +contact:roberto@uamv.edu.ni)";
pub const CONNECT_TIMEOUT_SECS: u64 = 10;
pub const REQUEST_TIMEOUT_SECS: u64 = 20;

/// Constructor configurable del cliente HTTP de Moodle (A-10).
/// Utiliza Rustls, timeouts estrictos y User-Agent propio, sin cookie store compartido.
#[derive(Debug, Clone)]
pub struct MoodleClientBuilder {
    base_url: String,
    user_agent: String,
    connect_timeout: Duration,
    timeout: Duration,
}

impl Default for MoodleClientBuilder {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            connect_timeout: Duration::from_secs(CONNECT_TIMEOUT_SECS),
            timeout: Duration::from_secs(REQUEST_TIMEOUT_SECS),
        }
    }
}

impl MoodleClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Permite sobreescribir la URL base de Moodle (esencial para pruebas con WireMock).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        let mut u = url.into();
        if u.ends_with('/') {
            u.pop();
        }
        self.base_url = u;
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Construye el `reqwest::Client` configurado.
    pub fn build_http(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(self.connect_timeout)
            .timeout(self.timeout)
            .user_agent(&self.user_agent)
            .build()
            .expect("Fallo al construir reqwest::Client con rustls")
    }

    /// URL base configurada.
    pub fn get_base_url(&self) -> &str {
        &self.base_url
    }
}

/// Cliente de alto nivel para interactuar con la API REST y autenticación de Moodle.
#[derive(Clone)]
pub struct MoodleClient {
    http: reqwest::Client,
    base_url: Arc<String>,
}

impl MoodleClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let builder = MoodleClientBuilder::new().base_url(base_url);
        let http = builder.build_http();
        Self {
            http,
            base_url: Arc::new(builder.base_url),
        }
    }

    pub fn from_builder(builder: MoodleClientBuilder) -> Self {
        let http = builder.build_http();
        Self {
            http,
            base_url: Arc::new(builder.base_url),
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Retorna la URL del endpoint de autenticación `login/token.php`.
    pub fn token_url(&self) -> String {
        format!("{}/login/token.php", self.base_url)
    }

    /// Retorna la URL del endpoint del servicio web `webservice/rest/server.php`.
    pub fn rest_url(&self) -> String {
        format!("{}/webservice/rest/server.php", self.base_url)
    }
}
