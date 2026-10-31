use crate::moodle::breaker::CircuitBreaker;
use crate::moodle::envelope::decode_envelope;
use crate::moodle::error::{MoodleError, Result};
use crate::moodle::gate::{MoodleGate, Priority};
use crate::moodle::metrics::{RequestCounters, RequestCounts};
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://uamvirtual.uam.edu.ni/grado";
pub const DEFAULT_USER_AGENT: &str =
    "RobertoIsWatchingBot/0.2.0 (UAM Virtual Companion; +contact:roberto@uamv.edu.ni)";
pub const CONNECT_TIMEOUT_SECS: u64 = 10;
pub const REQUEST_TIMEOUT_SECS: u64 = 20;

/// Constructor configurable del cliente HTTP de Moodle (A-10, A-17).
/// Utiliza Rustls, timeouts estrictos y User-Agent propio, sin cookie store compartido.
#[derive(Clone)]
pub struct MoodleClientBuilder {
    base_url: String,
    user_agent: String,
    connect_timeout: Duration,
    timeout: Duration,
    gate: Option<MoodleGate>,
    breaker: Option<CircuitBreaker>,
    counters: Option<Arc<RequestCounters>>,
    max_retries: usize,
}

impl Default for MoodleClientBuilder {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            connect_timeout: Duration::from_secs(CONNECT_TIMEOUT_SECS),
            timeout: Duration::from_secs(REQUEST_TIMEOUT_SECS),
            gate: None,
            breaker: None,
            counters: None,
            max_retries: 2,
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

    pub fn gate(mut self, gate: MoodleGate) -> Self {
        self.gate = Some(gate);
        self
    }

    pub fn breaker(mut self, breaker: CircuitBreaker) -> Self {
        self.breaker = Some(breaker);
        self
    }

    pub fn counters(mut self, counters: Arc<RequestCounters>) -> Self {
        self.counters = Some(counters);
        self
    }

    pub fn max_retries(mut self, retries: usize) -> Self {
        self.max_retries = retries;
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
    gate: Arc<MoodleGate>,
    breaker: Arc<CircuitBreaker>,
    counters: Arc<RequestCounters>,
    max_retries: usize,
}

impl MoodleClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let builder = MoodleClientBuilder::new().base_url(base_url);
        Self::from_builder(builder)
    }

    pub fn from_builder(builder: MoodleClientBuilder) -> Self {
        let http = builder.build_http();
        let counters = builder.counters.unwrap_or_else(|| {
            if let Some(ref g) = builder.gate {
                g.counters_arc()
            } else {
                Arc::new(RequestCounters::new())
            }
        });
        let gate = builder
            .gate
            .unwrap_or_default()
            .with_counters(Arc::clone(&counters));
        let breaker = builder.breaker.unwrap_or_default();
        Self {
            http,
            base_url: Arc::new(builder.base_url),
            gate: Arc::new(gate),
            breaker: Arc::new(breaker),
            counters,
            max_retries: builder.max_retries,
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn gate(&self) -> &MoodleGate {
        &self.gate
    }

    pub fn breaker(&self) -> &CircuitBreaker {
        &self.breaker
    }

    pub fn counters(&self) -> &RequestCounters {
        &self.counters
    }

    /// Instantánea inmutable de las peticiones HTTP realizadas a Moodle por prioridad.
    pub fn snapshot(&self) -> RequestCounts {
        self.counters.snapshot()
    }

    /// Reinicia los contadores de peticiones atómicamente a cero y retorna el valor previo.
    pub fn reset(&self) -> RequestCounts {
        self.counters.reset()
    }

    pub fn max_retries(&self) -> usize {
        self.max_retries
    }

    /// Retorna la URL del endpoint de autenticación `login/token.php`.
    pub fn token_url(&self) -> String {
        format!("{}/login/token.php", self.base_url)
    }

    /// Retorna la URL del endpoint del servicio web `webservice/rest/server.php`.
    pub fn rest_url(&self) -> String {
        format!("{}/webservice/rest/server.php", self.base_url)
    }

    /// Ejecuta una petición POST hacia `webservice/rest/server.php` con control de semáforo,
    /// circuit breaker y reintentos con backoff exponencial FUERA del permiso (A-06, A-17).
    pub async fn post_webservice<T: for<'de> serde::Deserialize<'de>>(
        &self,
        params: &[(&str, &str)],
        priority: Priority,
    ) -> Result<T> {
        let rest_url = self.rest_url();

        // Si el circuit breaker está abierto, fallar de inmediato sin consumir semáforo
        if self.breaker.is_open() {
            return Err(MoodleError::Maintenance);
        }

        let mut attempt = 0;
        loop {
            if attempt > 0 {
                // Backoff exponencial fuera del permiso del semáforo con pequeño jitter
                let backoff_ms =
                    (100 * (1 << (attempt - 1))) + rand::thread_rng().gen_range(10..=50);
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }

            // Adquirir permiso del semáforo de acuerdo a la prioridad
            let permit = self.gate.acquire(priority).await?;

            self.counters.inc(priority);
            let res = self.http.post(&rest_url).form(params).send().await;

            // Liberar el permiso del semáforo de inmediato antes de procesar el body o backoff
            drop(permit);

            match res {
                Ok(response) => {
                    if response.status().as_u16() == 503 || response.status().is_server_error() {
                        let err = MoodleError::Maintenance;
                        if attempt < self.max_retries {
                            attempt += 1;
                            tracing::warn!(
                                attempt,
                                "Moodle devolvió 5xx/mantenimiento. Reintentando..."
                            );
                            continue;
                        }
                        // R-14: Contar 1 fallo por operación agotada, no por reintento
                        self.breaker.record_failure();
                        return Err(err);
                    }

                    let body_text = response.text().await.map_err(MoodleError::from)?;
                    let decode_res: Result<T> = decode_envelope(&body_text);

                    match decode_res {
                        Ok(data) => {
                            self.breaker.record_success();
                            return Ok(data);
                        }
                        Err(err) => {
                            if err.is_transient() {
                                if attempt < self.max_retries {
                                    attempt += 1;
                                    tracing::warn!(
                                        attempt,
                                        error = %err,
                                        "Error transitorio en payload de Moodle. Reintentando..."
                                    );
                                    continue;
                                }
                                // R-14: Contar 1 fallo por operación agotada, no por reintento
                                self.breaker.record_failure();
                            } else {
                                // Errores no transitorios (ej. invalidtoken, nopermissions):
                                // Moodle respondió correctamente 200, por lo que no es caída del servicio
                                self.breaker.record_success();
                            }
                            return Err(err);
                        }
                    }
                }
                Err(req_err) => {
                    let err = MoodleError::from(req_err);
                    if err.is_transient() {
                        if attempt < self.max_retries {
                            attempt += 1;
                            tracing::warn!(
                                attempt,
                                error = %err,
                                "Error de red/timeout hacia Moodle. Reintentando..."
                            );
                            continue;
                        }
                        // R-14: Contar 1 fallo por operación agotada, no por reintento
                        self.breaker.record_failure();
                    }
                    return Err(err);
                }
            }
        }
    }
}
