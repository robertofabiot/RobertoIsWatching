use robertos_watching_bot::moodle::{MoodleClient, MoodleGate};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zeroize::Zeroizing;

#[derive(Clone)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBuffer {
    type Writer = SharedBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn test_login_never_leaks_password_into_tracing_on_success_and_error() {
    let log_buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = SharedBuffer(log_buffer.clone());

    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::TRACE)
        .finish();

    let _guard = tracing::subscriber::set_default(subscriber);

    let server = MockServer::start().await;
    let sentinel_pass = "SUPER_SECRET_CENTINELA_PASS_XYZ999";

    // 1. Mock de login/token.php exitoso
    Mock::given(method("POST"))
        .and(path("/login/token.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "token": "tok_mock_12345"
        })))
        .mount(&server)
        .await;

    // 2. Mock de core_webservice_get_site_info
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "userid": 999,
            "fullname": "Estudiante Centinela"
        })))
        .mount(&server)
        .await;

    let gate = MoodleGate::new(4, 3).without_jitter();
    let builder = robertos_watching_bot::moodle::MoodleClientBuilder::new()
        .base_url(server.uri())
        .gate(gate);
    let moodle = MoodleClient::from_builder(builder);

    // Ejecutar login con clave centinela
    let login_res = moodle
        .login("24019999", Zeroizing::new(sentinel_pass.to_string()))
        .await;
    assert!(login_res.is_ok());

    // Verificar logs capturados
    let logs = {
        let bytes = log_buffer.lock().unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    };

    assert!(
        !logs.contains(sentinel_pass),
        "¡Fuga de seguridad! La contraseña centinela apareció en los logs de tracing: {logs}"
    );

    // Caso de error: servidor responde invalidlogin
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/login/token.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": "Credenciales inválidas",
            "errorcode": "invalidlogin"
        })))
        .mount(&server)
        .await;

    let err_pass = "ANOTHER_FORBIDDEN_PASSWORD_ERR";
    let login_err = moodle
        .login("24019999", Zeroizing::new(err_pass.to_string()))
        .await;
    assert!(login_err.is_err());

    let updated_logs = {
        let bytes = log_buffer.lock().unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    };

    assert!(
        !updated_logs.contains(err_pass),
        "¡Fuga de seguridad en error! La contraseña apareció en los logs de tracing: {updated_logs}"
    );
}
