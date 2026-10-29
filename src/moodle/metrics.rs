//! Contadores atómicos y métricas de peticiones HTTP hacia Moodle discriminadas por prioridad (PR-B).

use crate::moodle::gate::Priority;
use std::sync::atomic::{AtomicU64, Ordering};

/// Instantánea inmutable del recuento de peticiones HTTP enviadas a Moodle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RequestCounts {
    pub interactive: u64,
    pub background: u64,
}

impl RequestCounts {
    /// Retorna la suma total de peticiones HTTP realizadas en ambas prioridades.
    pub fn total(&self) -> u64 {
        self.interactive + self.background
    }

    /// Calcula la diferencia neta entre esta instantánea (`self`) y una previa (`earlier`).
    pub fn diff(&self, earlier: &RequestCounts) -> RequestCounts {
        RequestCounts {
            interactive: self.interactive.saturating_sub(earlier.interactive),
            background: self.background.saturating_sub(earlier.background),
        }
    }
}

/// Contadores atómicos thread-safe asociados a una instancia de `MoodleClient` o `MoodleGate`.
#[derive(Debug, Default)]
pub struct RequestCounters {
    interactive: AtomicU64,
    background: AtomicU64,
}

impl RequestCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Incrementa en 1 el contador correspondiente a la prioridad indicada.
    pub fn inc(&self, priority: Priority) {
        match priority {
            Priority::Interactive => self.interactive.fetch_add(1, Ordering::Relaxed),
            Priority::Background => self.background.fetch_add(1, Ordering::Relaxed),
        };
    }

    /// Obtiene una instantánea actual de los contadores.
    pub fn snapshot(&self) -> RequestCounts {
        RequestCounts {
            interactive: self.interactive.load(Ordering::Relaxed),
            background: self.background.load(Ordering::Relaxed),
        }
    }

    /// Reinicia los contadores atómicamente a cero y retorna el valor previo al reinicio.
    pub fn reset(&self) -> RequestCounts {
        RequestCounts {
            interactive: self.interactive.swap(0, Ordering::Relaxed),
            background: self.background.swap(0, Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counters_inc_and_snapshot() {
        let counters = RequestCounters::new();
        counters.inc(Priority::Interactive);
        counters.inc(Priority::Interactive);
        counters.inc(Priority::Background);

        let snap = counters.snapshot();
        assert_eq!(snap.interactive, 2);
        assert_eq!(snap.background, 1);
        assert_eq!(snap.total(), 3);

        let prev = RequestCounts {
            interactive: 1,
            background: 0,
        };
        let diff = snap.diff(&prev);
        assert_eq!(diff.interactive, 1);
        assert_eq!(diff.background, 1);
        assert_eq!(diff.total(), 2);

        let reset = counters.reset();
        assert_eq!(reset, snap);

        let after = counters.snapshot();
        assert_eq!(after.interactive, 0);
        assert_eq!(after.background, 0);
        assert_eq!(after.total(), 0);
    }
}
