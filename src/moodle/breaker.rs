use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Estado observable del Circuit Breaker (A-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Moodle operativo y respondiendo normalmente.
    Up,
    /// Moodle caído o superado el umbral de fallos consecutivos; peticiones bloqueadas de inmediato.
    Down,
    /// Ventana de prueba tras vencer el cooldown para verificar si Moodle se recuperó.
    HalfOpen,
}

/// Disyuntor (Circuit Breaker) para protección contra caídas masivas de UAM Virtual (A-17).
/// Implementa `Clone` compartiendo el estado interno de forma segura entre hilos.
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    failure_threshold: usize,
    cooldown: Duration,
    failures: Arc<AtomicUsize>,
    inner: Arc<RwLock<BreakerInner>>,
}

#[derive(Debug)]
struct BreakerInner {
    state: CircuitState,
    last_trip_time: Option<Instant>,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(5, Duration::from_secs(60))
    }
}

impl CircuitBreaker {
    pub fn new(failure_threshold: usize, cooldown: Duration) -> Self {
        Self {
            failure_threshold,
            cooldown,
            failures: Arc::new(AtomicUsize::new(0)),
            inner: Arc::new(RwLock::new(BreakerInner {
                state: CircuitState::Up,
                last_trip_time: None,
            })),
        }
    }

    /// Retorna el estado actual del breaker, transicionando a HalfOpen si el cooldown ya expiró.
    pub fn state(&self) -> CircuitState {
        let mut inner = self.inner.write().unwrap();
        if inner.state == CircuitState::Down {
            if let Some(tripped) = inner.last_trip_time {
                if tripped.elapsed() >= self.cooldown {
                    inner.state = CircuitState::HalfOpen;
                }
            }
        }
        inner.state
    }

    /// Indica si el breaker está abierto (Down) bloqueando tráfico.
    pub fn is_open(&self) -> bool {
        self.state() == CircuitState::Down
    }

    /// Registra una operación exitosa, reseteando fallos y cerrando el circuito.
    /// Retorna `true` si ocurrió una transición observable de `Down/HalfOpen` hacia `Up` (A-17).
    pub fn record_success(&self) -> bool {
        self.failures.store(0, Ordering::SeqCst);
        let mut inner = self.inner.write().unwrap();
        let was_down = inner.state != CircuitState::Up;
        if was_down {
            inner.state = CircuitState::Up;
            inner.last_trip_time = None;
            tracing::info!("CircuitBreaker recuperado: transición Down -> Up (Moodle operativo)");
        }
        was_down
    }

    /// Registra un fallo transitorio.
    /// Retorna `true` si este fallo provocó la apertura del circuito (transición observable `Up -> Down`) (A-17).
    pub fn record_failure(&self) -> bool {
        let count = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        let mut inner = self.inner.write().unwrap();
        if count >= self.failure_threshold && inner.state != CircuitState::Down {
            inner.state = CircuitState::Down;
            inner.last_trip_time = Some(Instant::now());
            tracing::warn!(
                failures = count,
                threshold = self.failure_threshold,
                "CircuitBreaker disparado: transición Up -> Down (Moodle caído)"
            );
            return true;
        }
        false
    }

    /// Fuerza la apertura o cierre manual (útil para pruebas unitarias).
    pub fn force_state(&self, state: CircuitState) {
        let mut inner = self.inner.write().unwrap();
        inner.state = state;
        if state == CircuitState::Down {
            inner.last_trip_time = Some(Instant::now());
            self.failures
                .store(self.failure_threshold, Ordering::SeqCst);
        } else {
            inner.last_trip_time = None;
            self.failures.store(0, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_transitions() {
        let breaker = CircuitBreaker::new(3, Duration::from_millis(50));
        assert_eq!(breaker.state(), CircuitState::Up);
        assert!(!breaker.is_open());

        // 1 y 2 fallos no abren el breaker
        assert!(!breaker.record_failure());
        assert!(!breaker.record_failure());
        assert_eq!(breaker.state(), CircuitState::Up);

        // 3er fallo consecutivo abre el breaker (Up -> Down)
        assert!(breaker.record_failure());
        assert_eq!(breaker.state(), CircuitState::Down);
        assert!(breaker.is_open());

        // Esperar cooldown de 50ms para pasar a HalfOpen
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(breaker.state(), CircuitState::HalfOpen);

        // Éxito en HalfOpen recupera a Up
        assert!(breaker.record_success());
        assert_eq!(breaker.state(), CircuitState::Up);
        assert!(!breaker.is_open());
    }
}
