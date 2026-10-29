use crate::moodle::error::{MoodleError, Result};
use crate::moodle::metrics::{RequestCounters, RequestCounts};
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Nivel de prioridad para la ejecución de peticiones hacia Moodle (A-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Peticiones prioritarias e interactivas disparadas directamente por el usuario (ej. /sync, /login).
    /// Solo compite por el semáforo global (4), sin esperar tras las peticiones de fondo.
    Interactive,

    /// Peticiones en segundo plano de sincronización y escaneo continuo del worker.
    /// Limitadas por un sub-semáforo (3) antes de adquirir el permiso global (4).
    Background,
}

/// Permiso del gate de concurrencia.
/// Al salir de ámbito (drop), libera automáticamente las ranuras ocupadas en los semáforos.
pub struct GatePermit {
    _permit: PermitInner,
}

#[allow(dead_code)]
enum PermitInner {
    Interactive(OwnedSemaphorePermit),
    Background {
        _bg: OwnedSemaphorePermit,
        _global: OwnedSemaphorePermit,
    },
}

/// Gate de concurrencia de dos niveles con jitter aleatorio (A-06).
///
/// Arquitectura:
/// - Semáforo global: máx 4 peticiones simultáneas totales hacia los servidores UAM.
/// - Semáforo de background: máx 3 peticiones de worker en segundo plano simultáneas.
/// - Siempre garantiza al menos ≥1 ranura disponible de inmediato para peticiones interactivas.
#[derive(Clone)]
pub struct MoodleGate {
    global: Arc<Semaphore>,
    background: Arc<Semaphore>,
    counters: Arc<RequestCounters>,
    jitter_min_ms: u64,
    jitter_max_ms: u64,
}

impl Default for MoodleGate {
    fn default() -> Self {
        Self::new(4, 3)
    }
}

impl MoodleGate {
    /// Crea un nuevo gate con capacidades dadas para el nivel global y de background.
    pub fn new(global_capacity: usize, background_capacity: usize) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global_capacity)),
            background: Arc::new(Semaphore::new(background_capacity)),
            counters: Arc::new(RequestCounters::new()),
            jitter_min_ms: 100,
            jitter_max_ms: 300,
        }
    }

    /// Asocia contadores de peticiones al gate.
    pub fn with_counters(mut self, counters: Arc<RequestCounters>) -> Self {
        self.counters = counters;
        self
    }

    /// Retorna los contadores de peticiones asociados al gate.
    pub fn counters(&self) -> &RequestCounters {
        &self.counters
    }

    /// Retorna una referencia compartida (`Arc`) a los contadores de peticiones.
    pub fn counters_arc(&self) -> Arc<RequestCounters> {
        Arc::clone(&self.counters)
    }

    /// Incrementa manualmente los contadores asociados al gate.
    pub fn inc(&self, priority: Priority) {
        self.counters.inc(priority);
    }

    /// Retorna un snapshot inmutable de los contadores de peticiones.
    pub fn snapshot(&self) -> RequestCounts {
        self.counters.snapshot()
    }

    /// Reinicia los contadores a cero y retorna el valor previo al reinicio.
    pub fn reset(&self) -> RequestCounts {
        self.counters.reset()
    }

    /// Permite configurar el rango de jitter en milisegundos.
    pub fn with_jitter_range(mut self, min_ms: u64, max_ms: u64) -> Self {
        self.jitter_min_ms = min_ms;
        self.jitter_max_ms = max_ms;
        self
    }

    /// Desactiva el jitter para pruebas unitarias e integración de alta velocidad.
    pub fn without_jitter(mut self) -> Self {
        self.jitter_min_ms = 0;
        self.jitter_max_ms = 0;
        self
    }

    /// Adquiere un permiso en el gate respetando la prioridad y aplicando el jitter configurado.
    pub async fn acquire(&self, priority: Priority) -> Result<GatePermit> {
        let permit = match priority {
            Priority::Interactive => {
                let global_permit = self
                    .global
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| MoodleError::Protocol("Semáforo global cerrado".into()))?;
                GatePermit {
                    _permit: PermitInner::Interactive(global_permit),
                }
            }
            Priority::Background => {
                // Primero se adquiere el permiso de background (máximo 3)
                let bg_permit =
                    self.background.clone().acquire_owned().await.map_err(|_| {
                        MoodleError::Protocol("Semáforo de background cerrado".into())
                    })?;
                // Luego se adquiere el permiso global (máximo 4 totales)
                let global_permit = self
                    .global
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| MoodleError::Protocol("Semáforo global cerrado".into()))?;
                GatePermit {
                    _permit: PermitInner::Background {
                        _bg: bg_permit,
                        _global: global_permit,
                    },
                }
            }
        };

        self.apply_jitter().await;
        Ok(permit)
    }

    /// Permisos libres en el semáforo global.
    pub fn global_available(&self) -> usize {
        self.global.available_permits()
    }

    /// Permisos libres en el semáforo de background.
    pub fn background_available(&self) -> usize {
        self.background.available_permits()
    }

    async fn apply_jitter(&self) {
        if self.jitter_max_ms > 0 && self.jitter_max_ms >= self.jitter_min_ms {
            let delay_ms = if self.jitter_max_ms == self.jitter_min_ms {
                self.jitter_min_ms
            } else {
                rand::thread_rng().gen_range(self.jitter_min_ms..=self.jitter_max_ms)
            };
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gate_priorities_and_capacities() {
        let gate = MoodleGate::new(4, 3).without_jitter();
        assert_eq!(gate.global_available(), 4);
        assert_eq!(gate.background_available(), 3);

        // 3 peticiones background toman 3 ranuras bg y 3 ranuras global
        let p1 = gate.acquire(Priority::Background).await.unwrap();
        let p2 = gate.acquire(Priority::Background).await.unwrap();
        let p3 = gate.acquire(Priority::Background).await.unwrap();

        assert_eq!(gate.background_available(), 0);
        assert_eq!(gate.global_available(), 1);

        // Una petición interactiva puede entrar de inmediato usando la 4ta ranura global
        let p_interactive = gate.acquire(Priority::Interactive).await.unwrap();
        assert_eq!(gate.global_available(), 0);

        drop(p_interactive);
        assert_eq!(gate.global_available(), 1);

        drop(p1);
        drop(p2);
        drop(p3);
        assert_eq!(gate.global_available(), 4);
        assert_eq!(gate.background_available(), 3);
    }
}
