use robertos_watching_bot::moodle::{
    CircuitBreaker, CircuitState, MoodleClient, MoodleClientBuilder, MoodleError, MoodleGate,
    Priority, RequestCounts,
};
use robertos_watching_bot::storage::DonorToken;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[tokio::test]
async fn test_concurrency_gate_limits_global_and_background() {
    let server = MockServer::start().await;

    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_global_seen = Arc::new(AtomicUsize::new(0));
    let in_flight_bg = Arc::new(AtomicUsize::new(0));
    let max_bg_seen = Arc::new(AtomicUsize::new(0));

    // Custom responder de WireMock que rastrea concurrencia real recibida en el servidor
    struct TrackingResponder {
        in_flight: Arc<AtomicUsize>,
        max_global_seen: Arc<AtomicUsize>,
        in_flight_bg: Arc<AtomicUsize>,
        max_bg_seen: Arc<AtomicUsize>,
    }

    impl wiremock::Respond for TrackingResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body = String::from_utf8_lossy(&request.body);
            let is_bg = body.contains("userid=100"); // ID de tareas de background

            let cur_global = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_global_seen.fetch_max(cur_global, Ordering::SeqCst);

            if is_bg {
                let cur_bg = self.in_flight_bg.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_bg_seen.fetch_max(cur_bg, Ordering::SeqCst);
            }

            std::thread::sleep(Duration::from_millis(30));

            if is_bg {
                self.in_flight_bg.fetch_sub(1, Ordering::SeqCst);
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);

            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "courses": []
            }))
        }
    }

    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(TrackingResponder {
            in_flight: in_flight.clone(),
            max_global_seen: max_global_seen.clone(),
            in_flight_bg: in_flight_bg.clone(),
            max_bg_seen: max_bg_seen.clone(),
        })
        .mount(&server)
        .await;

    // Gate estricto: global 4, background 3, sin jitter para sincronización exacta
    let gate = MoodleGate::new(4, 3).without_jitter();
    let client = MoodleClient::from_builder(
        MoodleClientBuilder::new()
            .base_url(server.uri())
            .gate(gate)
            .max_retries(0),
    );

    let mut handles = Vec::new();

    // 10 tareas de background concurrentes
    for i in 0..10 {
        let client_c = client.clone();
        handles.push(tokio::spawn(async move {
            client_c
                .user_courses(&format!("tok_{i}"), 1000 + i, Priority::Background)
                .await
        }));
    }

    // 4 tareas interactivas concurrentes
    for i in 0..4 {
        let client_c = client.clone();
        handles.push(tokio::spawn(async move {
            client_c
                .user_courses(&format!("interactive_{i}"), 2000 + i, Priority::Interactive)
                .await
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    // (a) Concurrencia global en vuelo nunca debe superar 4
    let max_global = max_global_seen.load(Ordering::SeqCst);
    assert!(
        max_global <= 4,
        "La concurrencia global hacia Moodle excedió 4 (fue: {max_global})"
    );

    // (b) Concurrencia de background nunca debe superar 3
    let max_bg = max_bg_seen.load(Ordering::SeqCst);
    assert!(
        max_bg <= 3,
        "La concurrencia de background excedió 3 (fue: {max_bg})"
    );
}

#[tokio::test]
async fn test_failover_skips_invalid_tokens_and_reports_invalid_list() {
    let server = MockServer::start().await;

    // Custom responder que examina el token en el form body
    struct TokenDispatcher;

    impl wiremock::Respond for TokenDispatcher {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body = String::from_utf8_lossy(&request.body);
            if body.contains("bad_tok_1") || body.contains("bad_tok_2") {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "exception": "core\\exception\\moodle_exception",
                    "errorcode": "invalidtoken",
                    "message": "Ficha no válida"
                }))
            } else if body.contains("good_tok_3") {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "courses": [
                        {
                            "id": 505,
                            "assignments": [
                                {
                                    "id": 8801,
                                    "course": 505,
                                    "name": "Proyecto Final",
                                    "duedate": 1780000000
                                }
                            ]
                        }
                    ]
                }))
            } else {
                ResponseTemplate::new(400)
            }
        }
    }

    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(TokenDispatcher)
        .mount(&server)
        .await;

    let gate = MoodleGate::new(4, 3).without_jitter();
    let client = MoodleClient::from_builder(
        MoodleClientBuilder::new()
            .base_url(server.uri())
            .gate(gate)
            .max_retries(0),
    );

    let donors = vec![
        DonorToken {
            chat_id: 101,
            token: "bad_tok_1".to_string(),
        },
        DonorToken {
            chat_id: 102,
            token: "bad_tok_2".to_string(),
        },
        DonorToken {
            chat_id: 103,
            token: "good_tok_3".to_string(),
        },
    ];

    let outcome = client
        .fetch_course_with_failover(&donors, 505, Priority::Background)
        .await
        .expect("Debe rotar exitosamente hasta el donante 103");

    // (c) Failover salta tokens inválidos y reporta cuáles
    assert_eq!(outcome.donor.chat_id, 103);
    assert_eq!(outcome.donor.token, "good_tok_3");
    assert_eq!(outcome.invalid, vec![101, 102]);
    assert_eq!(outcome.data.len(), 1);
    assert_eq!(outcome.data[0].id, 8801);
}

#[tokio::test]
async fn test_timeout_maps_to_moodle_error_timeout() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(200)))
        .mount(&server)
        .await;

    // Timeout ultracorto de 50ms para disparar MoodleError::Timeout
    let gate = MoodleGate::new(4, 3).without_jitter();
    let client = MoodleClient::from_builder(
        MoodleClientBuilder::new()
            .base_url(server.uri())
            .timeout(Duration::from_millis(50))
            .gate(gate)
            .max_retries(0),
    );

    let res = client
        .get_site_info("any_token", Priority::Interactive)
        .await;

    // (d) Timeout -> MoodleError::Timeout
    assert!(
        matches!(res, Err(MoodleError::Timeout)),
        "Esperaba MoodleError::Timeout pero obtuve: {:?}",
        res
    );
}

#[tokio::test]
async fn test_circuit_breaker_trips_and_recovers() {
    let server = MockServer::start().await;

    // Respuesta 503 Service Unavailable
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let breaker = CircuitBreaker::new(3, Duration::from_millis(50));
    let gate = MoodleGate::new(4, 3).without_jitter();
    let client = MoodleClient::from_builder(
        MoodleClientBuilder::new()
            .base_url(server.uri())
            .breaker(breaker)
            .gate(gate)
            .max_retries(0),
    );

    assert_eq!(client.breaker().state(), CircuitState::Up);

    // 1er y 2do fallo no abren el circuito
    let _ = client.get_site_info("tok", Priority::Interactive).await;
    let _ = client.get_site_info("tok", Priority::Interactive).await;
    assert_eq!(client.breaker().state(), CircuitState::Up);

    // 3er fallo consecutivo abre el breaker (Up -> Down)
    let _ = client.get_site_info("tok", Priority::Interactive).await;
    assert_eq!(client.breaker().state(), CircuitState::Down);
    assert!(client.breaker().is_open());

    // Petición subsecuente es rechazada de inmediato sin contactar a Moodle
    let fast_fail = client.get_site_info("tok", Priority::Interactive).await;
    assert!(matches!(fast_fail, Err(MoodleError::Maintenance)));

    // Esperar a que expire el cooldown de 50ms para pasar a HalfOpen
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(client.breaker().state(), CircuitState::HalfOpen);

    // Reemplazar mock con respuesta exitosa 200 OK
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "userid": 1234,
            "fullname": "Alumno Recuperado"
        })))
        .mount(&server)
        .await;

    // (e) Breaker se recupera con éxito (HalfOpen -> Up)
    let recovered = client.get_site_info("tok", Priority::Interactive).await;
    assert!(recovered.is_ok());
    assert_eq!(client.breaker().state(), CircuitState::Up);
    assert!(!client.breaker().is_open());
}

#[tokio::test]
async fn test_request_counters_distinguish_priorities_and_count_retries() {
    let server = MockServer::start().await;

    let assign_attempts = Arc::new(AtomicUsize::new(0));
    let assign_attempts_clone = Arc::clone(&assign_attempts);

    struct RetryOnceResponder {
        attempts: Arc<AtomicUsize>,
    }

    impl wiremock::Respond for RetryOnceResponder {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                // Primer intento falla con error transitorio 500 (fuerza 1 reintento)
                ResponseTemplate::new(500)
            } else {
                // Siguiente intento responde 200 OK con asignaciones
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "courses": []
                }))
            }
        }
    }

    // Mock para get_site_info (Interactive)
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=core_webservice_get_site_info",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "userid": 1234,
            "fullname": "Test User"
        })))
        .mount(&server)
        .await;

    // Mock para user_courses (Interactive)
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=core_enrol_get_users_courses",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    // Mock para fetch_course_assignments (Background con 1 reintento forzado)
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=mod_assign_get_assignments",
        ))
        .respond_with(RetryOnceResponder {
            attempts: assign_attempts_clone,
        })
        .mount(&server)
        .await;

    // Mock para fetch_course_contents (Background sin reintentos)
    Mock::given(method("POST"))
        .and(path("/webservice/rest/server.php"))
        .and(wiremock::matchers::body_string_contains(
            "wsfunction=core_course_get_contents",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;

    let gate = MoodleGate::new(4, 3).without_jitter();
    let client = MoodleClient::from_builder(
        MoodleClientBuilder::new()
            .base_url(server.uri())
            .gate(gate)
            .max_retries(2),
    );

    // Estado inicial: 0 peticiones
    assert_eq!(client.snapshot(), RequestCounts::default());
    assert_eq!(client.gate().snapshot(), RequestCounts::default());

    // 1. Dos peticiones interactivas
    let site_info = client
        .get_site_info("token_test", Priority::Interactive)
        .await
        .unwrap();
    assert_eq!(site_info.userid, 1234);

    let courses = client
        .user_courses("token_test", 1234, Priority::Interactive)
        .await
        .unwrap();
    assert!(courses.is_empty());

    let s1 = client.snapshot();
    assert_eq!(s1.interactive, 2);
    assert_eq!(s1.background, 0);
    assert_eq!(s1.total(), 2);
    assert_eq!(client.gate().snapshot(), s1);

    // 2. Una petición de background con reintento forzado (500 -> 200, total 2 intentos HTTP)
    let assignments = client
        .fetch_course_assignments("token_test", 101, Priority::Background)
        .await
        .unwrap();
    assert!(assignments.is_empty());
    assert_eq!(assign_attempts.load(Ordering::SeqCst), 2);

    let s2 = client.snapshot();
    assert_eq!(s2.interactive, 2);
    assert_eq!(s2.background, 2);
    assert_eq!(s2.total(), 4);

    let diff1 = s2.diff(&s1);
    assert_eq!(diff1.interactive, 0);
    assert_eq!(diff1.background, 2);
    assert_eq!(diff1.total(), 2);

    // 3. Una petición de background adicional sin reintentos (1 intento HTTP)
    let contents = client
        .fetch_course_contents("token_test", 101, Priority::Background)
        .await
        .unwrap();
    assert!(contents.is_empty());

    let s3 = client.snapshot();
    assert_eq!(s3.interactive, 2);
    assert_eq!(s3.background, 3);
    assert_eq!(s3.total(), 5);

    let diff2 = s3.diff(&s2);
    assert_eq!(diff2.interactive, 0);
    assert_eq!(diff2.background, 1);
    assert_eq!(diff2.total(), 1);

    // 4. Probar reset(): devuelve la última instantánea y resetea a 0 tanto en client como en gate
    let reset_val = client.reset();
    assert_eq!(reset_val, s3);

    let s_after = client.snapshot();
    assert_eq!(s_after.interactive, 0);
    assert_eq!(s_after.background, 0);
    assert_eq!(s_after.total(), 0);
    assert_eq!(client.gate().snapshot(), s_after);
}
