use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

#[derive(PartialEq, Eq, Clone)]
pub struct Config {
    pub bot_token: String,
    pub admin_chat_id: i64,
    pub database_url: String,
    pub max_users_default: u32,
    pub bot_tz: chrono_tz::Tz,
    pub rust_log: String,
    pub scan_interval_min: u32,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bot_token", &"***")
            .field("admin_chat_id", &self.admin_chat_id)
            .field("database_url", &self.database_url)
            .field("max_users_default", &self.max_users_default)
            .field("bot_tz", &self.bot_tz)
            .field("rust_log", &self.rust_log)
            .field("scan_interval_min", &self.scan_interval_min)
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    MissingVar(&'static str),
    InvalidToken(String),
    InvalidAdminChatId(String),
    InvalidDatabaseUrl(String),
    InvalidMaxUsers(String),
    InvalidTimezone(String),
    InvalidScanInterval(String),
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
            ConfigError::InvalidScanInterval(msg) => {
                write!(f, "SCAN_INTERVAL_MIN inválido: {msg}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        dotenvy::dotenv().ok();
        Self::from_entries(std::env::vars())
    }

    pub fn from_entries<I, K, V>(iter: I) -> Result<Self, ConfigError>
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

        let scan_interval_min = match map.get("SCAN_INTERVAL_MIN").map(|s| s.trim()) {
            Some(val) if !val.is_empty() => {
                let parsed = val.parse::<u32>().map_err(|_| {
                    ConfigError::InvalidScanInterval(format!(
                        "no es un número entero válido: '{val}'"
                    ))
                })?;
                if parsed == 0 {
                    return Err(ConfigError::InvalidScanInterval(
                        "SCAN_INTERVAL_MIN debe ser mayor que 0".to_string(),
                    ));
                }
                if parsed > 1440 {
                    return Err(ConfigError::InvalidScanInterval(
                        "SCAN_INTERVAL_MIN no debe exceder 1440 minutos (24 horas)".to_string(),
                    ));
                }
                parsed
            }
            _ => 30,
        };

        Ok(Config {
            bot_token,
            admin_chat_id,
            database_url,
            max_users_default,
            bot_tz,
            rust_log,
            scan_interval_min,
        })
    }

    pub fn scan_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.scan_interval_min as u64 * 60)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_iter<I, K, V>(iter: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Self::from_entries(iter)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_env() -> HashMap<&'static str, &'static str> {
        let mut map = HashMap::new();
        map.insert("TELEGRAM_BOT_TOKEN", "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ");
        map.insert("ADMIN_CHAT_ID", "1125347763");
        map.insert("DATABASE_URL", "sqlite://robertos_watching.db?mode=rwc");
        map.insert("BOT_TZ", "America/Managua");
        map
    }

    #[test]
    fn test_config_ok_with_defaults() {
        let env = valid_env();
        let config = Config::from_iter(env).expect("Configuración válida falló");
        assert_eq!(config.bot_token, "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ");
        assert_eq!(config.admin_chat_id, 1125347763);
        assert_eq!(
            config.database_url,
            "sqlite://robertos_watching.db?mode=rwc"
        );
        assert_eq!(config.max_users_default, 300);
        assert_eq!(config.bot_tz, chrono_tz::America::Managua);
        assert_eq!(config.rust_log, "info,robertos_watching_bot=debug");
        assert_eq!(config.scan_interval_min, 30);
    }

    #[test]
    fn test_config_ok_custom_values() {
        let mut env = valid_env();
        env.insert("MAX_USERS_DEFAULT", "500");
        env.insert("RUST_LOG", "trace");
        env.insert("SCAN_INTERVAL_MIN", "45");
        let config = Config::from_iter(env).expect("Configuración personalizada falló");
        assert_eq!(config.max_users_default, 500);
        assert_eq!(config.rust_log, "trace");
        assert_eq!(config.scan_interval_min, 45);
        assert_eq!(config.scan_interval().as_secs(), 45 * 60);
    }

    #[test]
    fn test_config_missing_bot_token() {
        let mut env = valid_env();
        env.remove("TELEGRAM_BOT_TOKEN");
        let err = Config::from_iter(env).unwrap_err();
        assert_eq!(err, ConfigError::MissingVar("TELEGRAM_BOT_TOKEN"));
    }

    #[test]
    fn test_config_missing_admin_chat_id() {
        let mut env = valid_env();
        env.remove("ADMIN_CHAT_ID");
        let err = Config::from_iter(env).unwrap_err();
        assert_eq!(err, ConfigError::MissingVar("ADMIN_CHAT_ID"));
    }

    #[test]
    fn test_config_missing_database_url() {
        let mut env = valid_env();
        env.remove("DATABASE_URL");
        let err = Config::from_iter(env).unwrap_err();
        assert_eq!(err, ConfigError::MissingVar("DATABASE_URL"));
    }

    #[test]
    fn test_config_missing_bot_tz() {
        let mut env = valid_env();
        env.remove("BOT_TZ");
        let err = Config::from_iter(env).unwrap_err();
        assert_eq!(err, ConfigError::MissingVar("BOT_TZ"));
    }

    #[test]
    fn test_config_invalid_token_no_colon() {
        let mut env = valid_env();
        env.insert("TELEGRAM_BOT_TOKEN", "123456789ABCdef");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidToken(_)));
    }

    #[test]
    fn test_config_invalid_token_non_numeric_id() {
        let mut env = valid_env();
        env.insert("TELEGRAM_BOT_TOKEN", "invalid_id:ABCdef");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidToken(_)));
    }

    #[test]
    fn test_config_invalid_token_empty_secret() {
        let mut env = valid_env();
        env.insert("TELEGRAM_BOT_TOKEN", "123456789: ");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidToken(_)));
    }

    #[test]
    fn test_config_invalid_admin_chat_id() {
        let mut env = valid_env();
        env.insert("ADMIN_CHAT_ID", "not_a_number");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidAdminChatId(_)));
    }

    #[test]
    fn test_config_invalid_database_url() {
        let mut env = valid_env();
        env.insert("DATABASE_URL", "postgres://localhost/db");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidDatabaseUrl(_)));
    }

    #[test]
    fn test_config_invalid_max_users_zero() {
        let mut env = valid_env();
        env.insert("MAX_USERS_DEFAULT", "0");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidMaxUsers(_)));
    }

    #[test]
    fn test_config_invalid_timezone() {
        let mut env = valid_env();
        env.insert("BOT_TZ", "Mars/Phobos");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidTimezone(_)));
    }

    #[test]
    fn test_config_invalid_scan_interval_zero() {
        let mut env = valid_env();
        env.insert("SCAN_INTERVAL_MIN", "0");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidScanInterval(_)));
    }

    #[test]
    fn test_config_invalid_scan_interval_too_large() {
        let mut env = valid_env();
        env.insert("SCAN_INTERVAL_MIN", "1441");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidScanInterval(_)));
    }

    #[test]
    fn test_config_invalid_scan_interval_not_a_number() {
        let mut env = valid_env();
        env.insert("SCAN_INTERVAL_MIN", "not_a_number");
        let err = Config::from_iter(env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidScanInterval(_)));
    }

    #[test]
    fn test_config_debug_redacts_token() {
        let env = valid_env();
        let config = Config::from_iter(env).expect("Configuración válida");
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("bot_token: \"***\""));
        assert!(!debug_str.contains("ABCdefGhIJKlmNoPQRsTUVwxyZ"));
    }
}
