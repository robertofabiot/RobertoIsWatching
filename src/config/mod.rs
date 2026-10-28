use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Config {
    pub bot_token: String,
    pub admin_chat_id: i64,
    pub database_url: String,
    pub max_users_default: u32,
    pub bot_tz: chrono_tz::Tz,
    pub rust_log: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    MissingVar(&'static str),
    InvalidToken(String),
    InvalidAdminChatId(String),
    InvalidDatabaseUrl(String),
    InvalidMaxUsers(String),
    InvalidTimezone(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::MissingVar(var) => {
                write!(f, "Variable de entorno requerida ausente: {var}")
            }
            ConfigError::InvalidToken(msg) => write!(f, "TELEGRAM_BOT_TOKEN inválido: {msg}"),
            ConfigError::InvalidAdminChatId(msg) => write!(f, "ADMIN_CHAT_ID inválido: {msg}"),
            ConfigError::InvalidDatabaseUrl(msg) => write!(f, "DATABASE_URL inválida: {msg}"),
            ConfigError::InvalidMaxUsers(msg) => write!(f, "MAX_USERS_DEFAULT inválido: {msg}"),
            ConfigError::InvalidTimezone(tz) => write!(
                f,
                "BOT_TZ inválida: '{tz}' no es una zona horaria IANA válida"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        dotenvy::dotenv().ok();
        Self::from_iter(std::env::vars())
    }

    pub fn from_iter<I, K, V>(iter: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let map: HashMap<String, String> = iter
            .into_iter()
            .map(|(k, v)| (k.as_ref().to_string(), v.as_ref().to_string()))
            .collect();

        let bot_token = map
            .get("TELEGRAM_BOT_TOKEN")
            .filter(|s| !s.trim().is_empty())
            .ok_or(ConfigError::MissingVar("TELEGRAM_BOT_TOKEN"))?
            .trim()
            .to_string();

        validate_bot_token(&bot_token)?;

        let admin_chat_id_raw = map
            .get("ADMIN_CHAT_ID")
            .filter(|s| !s.trim().is_empty())
            .ok_or(ConfigError::MissingVar("ADMIN_CHAT_ID"))?
            .trim();

        let admin_chat_id = admin_chat_id_raw
            .parse::<i64>()
            .map_err(|_| ConfigError::InvalidAdminChatId(admin_chat_id_raw.to_string()))?;

        let database_url = map
            .get("DATABASE_URL")
            .filter(|s| !s.trim().is_empty())
            .ok_or(ConfigError::MissingVar("DATABASE_URL"))?
            .trim()
            .to_string();

        if !database_url.starts_with("sqlite:") {
            return Err(ConfigError::InvalidDatabaseUrl(
                "DATABASE_URL debe comenzar con 'sqlite:'".to_string(),
            ));
        }

        let max_users_default = match map.get("MAX_USERS_DEFAULT").map(|s| s.trim()) {
            Some(val) if !val.is_empty() => val
                .parse::<u32>()
                .map_err(|_| ConfigError::InvalidMaxUsers(val.to_string()))?,
            _ => 300,
        };

        if max_users_default == 0 {
            return Err(ConfigError::InvalidMaxUsers(
                "MAX_USERS_DEFAULT debe ser mayor que 0".to_string(),
            ));
        }

        let bot_tz_raw = map
            .get("BOT_TZ")
            .filter(|s| !s.trim().is_empty())
            .ok_or(ConfigError::MissingVar("BOT_TZ"))?
            .trim();

        let bot_tz = chrono_tz::Tz::from_str(bot_tz_raw)
            .map_err(|_| ConfigError::InvalidTimezone(bot_tz_raw.to_string()))?;

        let rust_log = map
            .get("RUST_LOG")
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "info,robertos_watching_bot=debug".to_string());

        Ok(Config {
            bot_token,
            admin_chat_id,
            database_url,
            max_users_default,
            bot_tz,
            rust_log,
        })
    }
}

fn validate_bot_token(token: &str) -> Result<(), ConfigError> {
    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() != 2 {
        return Err(ConfigError::InvalidToken(
            "El token debe contener exactamente un delimitador ':' separando ID y secreto"
                .to_string(),
        ));
    }

    let (bot_id, secret) = (parts[0], parts[1]);
    if bot_id.is_empty() || !bot_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(ConfigError::InvalidToken(
            "La primera parte del token debe ser un ID numérico válido".to_string(),
        ));
    }

    if secret.trim().is_empty() {
        return Err(ConfigError::InvalidToken(
            "La parte secreta del token no puede estar vacía".to_string(),
        ));
    }

    Ok(())
}
