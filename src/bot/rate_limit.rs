use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Limitador de tasa de intentos de `/login` en memoria por `chat_id` (A-11).
/// Aplica una política de máx 3 intentos fallidos cada 10 minutos para proteger
/// al bot de convertirse en un oráculo de fuerza bruta contra cuentas institucionales.
#[derive(Clone, Default)]
pub struct LoginRateLimiter {
    attempts: Arc<Mutex<HashMap<i64, Vec<Instant>>>>,
}

impl LoginRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Comprueba si el `chat_id` se encuentra actualmente bloqueado por exceso de intentos (A-11).
    pub fn is_rate_limited(&self, chat_id: i64) -> bool {
        let mut map = self.attempts.lock().unwrap();
        let now = Instant::now();
        let window = Duration::from_secs(600); // 10 minutos

        if let Some(history) = map.get_mut(&chat_id) {
            history.retain(|&t| now.duration_since(t) < window);
            history.len() >= 3
        } else {
            false
        }
    }

    /// Registra un intento de login fallido.
    pub fn record_failure(&self, chat_id: i64) {
        let mut map = self.attempts.lock().unwrap();
        let now = Instant::now();
        let window = Duration::from_secs(600);

        let history = map.entry(chat_id).or_default();
        history.retain(|&t| now.duration_since(t) < window);
        history.push(now);
    }

    /// Limpia los intentos registrados tras un inicio de sesión exitoso.
    pub fn clear(&self, chat_id: i64) {
        let mut map = self.attempts.lock().unwrap();
        map.remove(&chat_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_rate_limiter_policy() {
        let limiter = LoginRateLimiter::new();
        let chat_id = 123456;

        assert!(!limiter.is_rate_limited(chat_id));

        limiter.record_failure(chat_id);
        assert!(!limiter.is_rate_limited(chat_id));

        limiter.record_failure(chat_id);
        assert!(!limiter.is_rate_limited(chat_id));

        // 3er intento fallido => se activa el rate limit
        limiter.record_failure(chat_id);
        assert!(limiter.is_rate_limited(chat_id));

        // Otro chat no se ve afectado
        assert!(!limiter.is_rate_limited(999999));

        // Éxito limpia el límite
        limiter.clear(chat_id);
        assert!(!limiter.is_rate_limited(chat_id));
    }
}
