//! Test guardián de higiene y anonimización de fixtures (PR-C).
//!
//! Recorre todos los archivos `tests/fixtures/*.json` y verifica que no contengan:
//! 1. Patrones de correo electrónico (`usuario@dominio.tld`).
//! 2. Parámetros de URL con tokens (`token=<valor>` o `wstoken=<valor>`).
//! 3. Cadenas hexadecimales de 32 caracteres (formato canónico de tokens Moodle y claves MD5).
//! 4. Claves `privatetoken` con valores no vacíos.
//!
//! Lista de excepciones explícitas documentada abajo (tolerancia cero por defecto).

use std::fs;
use std::path::{Path, PathBuf};

/// Lista de excepciones explícitas permitidas en los fixtures.
///
/// Todas las fixtures han sido anonimizadas con datos sintéticos y no requieren
/// excepciones para tokens reales, correos personales ni claves privadas de acceso.
///
/// Nota de diseño (auditoría PR-C):
/// - Los nombres de cursos (ej. "ADM0111 - ESTADISTICA I - GRUPO 16") y códigos `idnumber` (ej. "96103")
///   pertenecen al catálogo académico público de la universidad y no son datos personales de alumnos.
/// - Los nombres de funciones Moodle (ej. "core_calendar_get_calendar_export_token") son identificadores
///   del API del sistema y no representan valores de token.
const ALLOWED_EXCEPTIONS: &[&str] = &[
    // Actualmente la lista es vacía: cualquier token hex32, email o wstoken es considerado violación.
];

/// Comprueba si un valor detectado está en la lista de excepciones permitidas.
fn is_allowed(val: &str) -> bool {
    ALLOWED_EXCEPTIONS.contains(&val)
}

/// Detecta cadenas hexadecimales exactas de 32 caracteres (tokens de Moodle, hashes MD5).
fn find_32_hex_strings(text: &str) -> Vec<String> {
    let mut results = Vec::new();
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        if bytes[i].is_ascii_hexdigit() {
            let start = i;
            while i < n && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            let len = i - start;
            if len == 32 {
                let candidate = &text[start..i];
                if !is_allowed(candidate) {
                    results.push(candidate.to_string());
                }
            }
        } else {
            i += 1;
        }
    }
    results
}

/// Detecta parámetros de URL con tokens (`token=` o `wstoken=`) que contengan valores no vacíos.
fn find_url_token_params(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut results = Vec::new();

    for pattern in &["wstoken=", "token="] {
        let mut start_search = 0;
        while let Some(idx) = lower[start_search..].find(pattern) {
            let pos = start_search + idx + pattern.len();
            let val: String = text[pos..]
                .chars()
                .take_while(|c| {
                    *c != '&'
                        && *c != '"'
                        && *c != '\''
                        && *c != ' '
                        && *c != '\n'
                        && *c != '\r'
                        && *c != '}'
                        && *c != ']'
                })
                .collect();

            if !val.is_empty() && !is_allowed(&val) {
                results.push(format!("{}{}", pattern, val));
            }
            start_search = pos;
        }
    }
    results
}

/// Detecta patrones de correo electrónico (`usuario@dominio.tld`).
fn find_emails(text: &str) -> Vec<String> {
    let mut emails = Vec::new();
    let delimiters = [
        ' ', '\t', '\n', '\r', '"', '\'', '<', '>', '(', ')', '[', ']', '{', '}', ',', ';',
    ];

    for word in text.split(&delimiters[..]) {
        if let Some(at_idx) = word.find('@') {
            let (user, domain_part) = word.split_at(at_idx);
            let domain = &domain_part[1..]; // saltar '@'

            let valid_user = !user.is_empty()
                && user.chars().all(|c| {
                    c.is_alphanumeric() || c == '.' || c == '_' || c == '%' || c == '+' || c == '-'
                });

            if valid_user && domain.contains('.') {
                let parts: Vec<&str> = domain.split('.').collect();
                if parts.len() >= 2 {
                    let tld = parts.last().unwrap();
                    let valid_tld = tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic());
                    let valid_domain_parts = parts.iter().all(|p| {
                        !p.is_empty()
                            && p.chars().all(|c| c.is_alphanumeric() || c == '-')
                            && !p.starts_with('-')
                            && !p.ends_with('-')
                    });

                    if valid_tld && valid_domain_parts && !is_allowed(word) {
                        emails.push(word.to_string());
                    }
                }
            }
        }
    }
    emails
}

/// Busca recursivamente en un `serde_json::Value` claves `privatetoken` con valores no vacíos.
fn find_privatetokens(value: &serde_json::Value, path: &str, results: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let cur_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };

                if k.eq_ignore_ascii_case("privatetoken") {
                    let is_empty = match v {
                        serde_json::Value::Null => true,
                        serde_json::Value::String(s) => s.is_empty(),
                        _ => false,
                    };
                    if !is_empty && !is_allowed(&cur_path) {
                        results.push(format!("{cur_path} = {v:?}"));
                    }
                }
                find_privatetokens(v, &cur_path, results);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                find_privatetokens(v, &format!("{path}[{i}]"), results);
            }
        }
        _ => {}
    }
}

fn get_fixture_files() -> Vec<PathBuf> {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    assert!(
        fixtures_dir.is_dir(),
        "El directorio de fixtures debe existir: {:?}",
        fixtures_dir
    );

    let mut files = Vec::new();
    for entry in fs::read_dir(&fixtures_dir).expect("Error al leer tests/fixtures") {
        let entry = entry.expect("Error en entrada de directorio");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            files.push(path);
        }
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "Debe haber al menos un archivo JSON en tests/fixtures"
    );
    files
}

#[test]
fn test_fixtures_hygiene_and_anonymization() {
    let files = get_fixture_files();
    let mut violations = Vec::new();

    for path in &files {
        let filename = path.file_name().unwrap().to_string_lossy();
        let content = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Error al leer {}: {}", path.display(), e));

        // 1. Verificar ausencia de correos electrónicos
        let emails = find_emails(&content);
        if !emails.is_empty() {
            violations.push(format!(
                "[{}] Correos electrónicos encontrados: {:?}",
                filename, emails
            ));
        }

        // 2. Verificar ausencia de tokens en URLs (?token= o &wstoken=)
        let url_tokens = find_url_token_params(&content);
        if !url_tokens.is_empty() {
            violations.push(format!(
                "[{}] Parámetros de token en URL encontrados: {:?}",
                filename, url_tokens
            ));
        }

        // 3. Verificar ausencia de cadenas hexadecimales de 32 caracteres (tokens Moodle)
        let hex32 = find_32_hex_strings(&content);
        if !hex32.is_empty() {
            violations.push(format!(
                "[{}] Cadenas hex de 32 caracteres (tokens) encontradas: {:?}",
                filename, hex32
            ));
        }

        // 4. Verificar ausencia de clave privatetoken no vacía en el JSON
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(json_val) => {
                let mut privatetokens = Vec::new();
                find_privatetokens(&json_val, "", &mut privatetokens);
                if !privatetokens.is_empty() {
                    violations.push(format!(
                        "[{}] Clave privatetoken no vacía encontrada: {:?}",
                        filename, privatetokens
                    ));
                }
            }
            Err(e) => {
                violations.push(format!("[{}] JSON inválido o malformado: {}", filename, e));
            }
        }
    }

    if !violations.is_empty() {
        panic!(
            "FALLO DE HIGIENE EN FIXTURES (Datos sensibles o tokens detectados):\n- {}",
            violations.join("\n- ")
        );
    }
}
