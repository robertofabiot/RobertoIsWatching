use robertos_watching_bot::moodle::{decode_envelope, MoodleError};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct DummyPayload {
    id: i64,
    name: String,
}

#[test]
fn test_decode_invalidtoken_fixture() {
    let fixture = include_str!("fixtures/error_invalidtoken.json");
    let result = decode_envelope::<DummyPayload>(fixture);

    assert!(matches!(result, Err(MoodleError::InvalidToken)));
    assert!(result.unwrap_err().is_token_error());
}

#[test]
fn test_decode_invalidlogin_cases() {
    // Caso 1: Con errorcode explícito
    let json_errorcode =
        r#"{"error":"Nombre de usuario o contraseña incorrectos","errorcode":"invalidlogin"}"#;
    let res1 = decode_envelope::<DummyPayload>(json_errorcode);
    assert_eq!(res1, Err(MoodleError::InvalidLogin));

    // Caso 2: Solo campo error (retornado a veces por token.php)
    let json_only_error = r#"{"error":"Invalid login, please check your credentials"}"#;
    let res2 = decode_envelope::<DummyPayload>(json_only_error);
    assert_eq!(res2, Err(MoodleError::InvalidLogin));
}

#[test]
fn test_decode_access_denied_cases() {
    let json = r#"{"exception":"required_capability_exception","errorcode":"nopermissions","message":"No tiene permisos para ver esta tarea"}"#;
    let res = decode_envelope::<DummyPayload>(json);
    assert!(
        matches!(res, Err(MoodleError::AccessDenied(msg)) if msg.contains("No tiene permisos"))
    );
}

#[test]
fn test_decode_maintenance_html_and_empty() {
    let html1 = "<!DOCTYPE html><html><body><h1>Moodle en Mantenimiento</h1></body></html>";
    assert_eq!(
        decode_envelope::<DummyPayload>(html1),
        Err(MoodleError::Maintenance)
    );

    let html2 = "<html><head><title>503 Service Unavailable</title></head></html>";
    assert_eq!(
        decode_envelope::<DummyPayload>(html2),
        Err(MoodleError::Maintenance)
    );

    let empty = "   \n\t  ";
    assert_eq!(
        decode_envelope::<DummyPayload>(empty),
        Err(MoodleError::Maintenance)
    );
}

#[test]
fn test_decode_successful_payload() {
    let valid_json = r#"{"id": 42, "name": "Cálculo I"}"#;
    let res = decode_envelope::<DummyPayload>(valid_json).expect("Debe decodificar JSON válido");
    assert_eq!(
        res,
        DummyPayload {
            id: 42,
            name: "Cálculo I".to_string()
        }
    );
}

#[test]
fn test_decode_malformed_json_returns_protocol_error() {
    let bad_json = r#"{"id": 42, "name": incompleto..."#;
    let res = decode_envelope::<DummyPayload>(bad_json);
    assert!(matches!(res, Err(MoodleError::Protocol(_))));
}
