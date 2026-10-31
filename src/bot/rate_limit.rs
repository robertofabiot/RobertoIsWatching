use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_FAILURES: usize = 3;
const WINDOW_DURATION: Duration = Duration::from_secs(600); // 10 minutos (A-11, R-12)

#[derive(Default)]
struct RateLimiterState {
    by_chat: HashMap<i64, Vec<Instant>>,
    by_cif: HashMap<String, Vec<Instant>>,
}

impl RateLimiterState {
    /// Elimina timestamps expirados de cada vector y purga entradas con historial vacío del HashMap (R-12).
    fn purge(&mut self, now: Instant) {
        self.by_chat.retain(|_, history| {
            history.retain(|&t| now.duration_since(t) < WINDOW_DURATION);
            !history.is_empty()
        });
        self.by_cif.retain(|_, history| {
            history.retain(|&t| now.duration_since(t) < WINDOW_DURATION);
            !history.is_empty()
        });
    }

    fn is_chat_limited(&self, chat_id: i64, now: Instant) -> bool {
        if let Some(history) = self.by_chat.get(&chat_id) {
            let active = history
                .iter()
                .filter(|&&t| now.duration_since(t) < WINDOW_DURATION)
                .count();
            active >= MAX_FAILURES
        } else {
            false
        }
    }

    fn is_cif_limited(&self, cif: &str, now: Instant) -> bool {
        if let Some(history) = self.by_cif.get(cif) {
            let active = history
                .iter()
                .filter(|&&t| now.duration_since(t) < WINDOW_DURATION)
                .count();
            active >= MAX_FAILURES
        } else {
            false
        }
    }
}

/// Error indicando que se ha superado el límite de intentos de login.
#[derive(Debug, PartialEq, Eq)]
pub struct RateLimitExceeded;

/// Limitador de tasa de intentos de `/login` en memoria por `chat_id` y `cif` (A-11, R-12).
/// Aplica una política de máx 3 intentos fallidos cada 10 minutos para proteger
/// al bot de convertirse en un oráculo de fuerza bruta contra cuentas institucionales.
#[derive(Clone, Default)]
pub struct LoginRateLimiter {
    state: Arc<Mutex<RateLimiterState>>,
}

impl LoginRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Comprueba si el `chat_id` se encuentra actualmente bloqueado por exceso de intentos (A-11).
    pub fn is_rate_limited(&self, chat_id: i64) -> bool {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.purge(now);
        state.is_chat_limited(chat_id, now)
    }

    /// Comprueba si un `cif` se encuentra actualmente bloqueado por exceso de intentos (R-12).
    pub fn is_cif_rate_limited(&self, cif: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.purge(now);
        state.is_cif_limited(cif, now)
    }

    /// Comprueba si `chat_id` o `cif` se encuentran bloqueados.
    pub fn is_any_rate_limited(&self, chat_id: i64, cif: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.purge(now);
        state.is_chat_limited(chat_id, now) || state.is_cif_limited(cif, now)
    }

    /// Comprueba de forma atómica bajo el mismo lock si `chat_id` o `cif` excedieron el límite.
    /// Si NO están limitados, registra el intento actual en ambos y retorna `Ok(())`.
    /// Si alguno excede el límite, retorna `Err(RateLimitExceeded)` sin registrar nuevo intento (R-12).
    pub fn check_and_record(&self, chat_id: i64, cif: &str) -> Result<(), RateLimitExceeded> {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.purge(now);

        if state.is_chat_limited(chat_id, now) || state.is_cif_limited(cif, now) {
            return Err(RateLimitExceeded);
        }

        state.by_chat.entry(chat_id).or_default().push(now);
        state.by_cif.entry(cif.to_string()).or_default().push(now);
        Ok(())
    }

    /// Revierte el último intento registrado en caso de errores de red o mantenimiento (no imputables a clave incorrecta).
    pub fn rollback(&self, chat_id: i64, cif: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(hist) = state.by_chat.get_mut(&chat_id) {
            hist.pop();
        }
        if let Some(hist) = state.by_cif.get_mut(cif) {
            hist.pop();
        }
        let now = Instant::now();
        state.purge(now);
    }

    /// Registra un intento de login fallido por chat_id (compatibilidad A-11).
    pub fn record_failure(&self, chat_id: i64) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.purge(now);
        state.by_chat.entry(chat_id).or_default().push(now);
    }

    /// Registra un intento de login fallido por chat_id y cif (R-12).
    pub fn record_failure_both(&self, chat_id: i64, cif: &str) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.purge(now);
        state.by_chat.entry(chat_id).or_default().push(now);
        state.by_cif.entry(cif.to_string()).or_default().push(now);
    }

    /// Limpia los intentos registrados de un chat tras inicio de sesión exitoso.
    pub fn clear(&self, chat_id: i64) {
        let mut state = self.state.lock().unwrap();
        state.by_chat.remove(&chat_id);
    }

    /// Limpia los intentos registrados tanto de chat_id como de cif tras inicio de sesión exitoso (R-12).
    pub fn clear_both(&self, chat_id: i64, cif: &str) {
        let mut state = self.state.lock().unwrap();
        state.by_chat.remove(&chat_id);
        state.by_cif.remove(cif);
    }

    /// Retorna el número de entradas activas de chat_id y cif (usado para tests de purga).
    #[cfg(test)]
    pub fn entries_count(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.by_chat.len(), state.by_cif.len())
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

    #[test]
    fn test_login_rate_limiter_by_cif_across_different_chats() {
        let limiter = LoginRateLimiter::new();
        let cif = "24019999";

        // Chat 1 falla 2 veces
        assert!(limiter.check_and_record(101, cif).is_ok());
        assert!(limiter.check_and_record(101, cif).is_ok());

        // Chat 2 (distinto atacante) falla 1 vez para el mismo CIF
        assert!(limiter.check_and_record(102, cif).is_ok());

        // Ahora el CIF tiene 3 intentos fallidos registrados
        assert!(limiter.is_cif_rate_limited(cif));

        // Chat 3 intenta atacar el mismo CIF => debe ser bloqueado por CIF
        assert!(limiter.check_and_record(103, cif).is_err());

        // Pero Chat 3 puede intentar otro CIF distinto
        assert!(limiter.check_and_record(103, "24018888").is_ok());
    }

    #[test]
    fn test_check_and_record_atomic_and_rollback() {
        let limiter = LoginRateLimiter::new();
        let chat_id = 555;
        let cif = "24015555";

        assert!(limiter.check_and_record(chat_id, cif).is_ok());
        assert!(limiter.check_and_record(chat_id, cif).is_ok());

        // Simular que el segundo intento falló por timeout de red (MoodleMaintenance) y se revierte
        limiter.rollback(chat_id, cif);

        // Queda 1 intento
        assert!(limiter.check_and_record(chat_id, cif).is_ok()); // 2do
        assert!(limiter.check_and_record(chat_id, cif).is_ok()); // 3ro

        // 4to intento debe ser bloqueado
        assert!(limiter.check_and_record(chat_id, cif).is_err());
    }

    #[test]
    fn test_rate_limiter_purges_expired_entries() {
        let limiter = LoginRateLimiter::new();
        let chat_id = 777;
        let cif = "24017777";

        limiter.record_failure(chat_id);
        limiter.record_failure_both(chat_id, cif);

        let (chats, cifs) = limiter.entries_count();
        assert_eq!(chats, 1);
        assert_eq!(cifs, 1);

        // Manipular manualmente el timestamp para simular 11 minutos pasados
        {
            let mut state = limiter.state.lock().unwrap();
            let expired = Instant::now()
                .checked_sub(Duration::from_secs(660))
                .unwrap();
            if let Some(hist) = state.by_chat.get_mut(&chat_id) {
                *hist = vec![expired];
            }
            if let Some(hist) = state.by_cif.get_mut(cif) {
                *hist = vec![expired];
            }
        }

        // Al consultar nuevamente, purge() debe limpiar completamente las entradas vacías
        assert!(!limiter.is_rate_limited(chat_id));
        assert!(!limiter.is_cif_rate_limited(cif));

        let (chats_after, cifs_after) = limiter.entries_count();
        assert_eq!(
            chats_after, 0,
            "Entradas vencidas de chats debieron ser purgadas del HashMap"
        );
        assert_eq!(
            cifs_after, 0,
            "Entradas vencidas de CIF debieron ser purgadas del HashMap"
        );
    }
}
