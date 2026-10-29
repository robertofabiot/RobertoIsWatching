//! Planificador centralizado de plazos de tareas sobre un min-heap (PR #7, Plan Maestro §5.1, A-16).
//! Gestiona una única tarea Tokio con `BinaryHeap` y re-armado reactivo vía canal MPSC.

use crate::bot::Bot;
use crate::config::Config;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Representa un trabajo de alerta programada para una tarea de un alumno.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadlineJob {
    pub chat_id: i64,
    pub assignment_id: i64,
    pub effective_due: i64, // epoch timestamp en segundos
    pub version: u64,
}

/// Elemento en el BinaryHeap ordenado como min-heap por `fire_at` (menor tiempo primero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledDeadline {
    pub fire_at: DateTime<Utc>,
    pub job: DeadlineJob,
}

impl Ord for ScheduledDeadline {
    fn cmp(&self, other: &Self) -> Ordering {
        // Invertido para min-heap: el fire_at más cercano se despacha primero
        other.fire_at.cmp(&self.fire_at)
    }
}

impl PartialOrd for ScheduledDeadline {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Comandos aceptados por el worker del planificador.
#[derive(Debug, Clone)]
pub enum DeadlineCommand {
    /// Agenda un trabajo de alerta crítica (2h).
    Schedule(DeadlineJob),
    /// Reprograma el plazo de una tarea cuando cambia la fecha de entrega.
    Reschedule {
        chat_id: i64,
        assignment_id: i64,
        new_due: i64,
    },
    /// Cancela y purga todos los trabajos de un alumno (ej. /logout o desactivación).
    CancelForUser(i64),
    /// Cancela un trabajo específico (ej. tarea entregada).
    CancelJob { chat_id: i64, assignment_id: i64 },
}

/// Interfaz del planificador que envía comandos al worker en Tokio.
#[derive(Clone)]
pub struct DeadlineScheduler {
    tx: mpsc::Sender<DeadlineCommand>,
}

impl DeadlineScheduler {
    pub fn new(pool: SqlitePool, bot: Bot, config: Arc<Config>) -> (Self, DeadlineWorker) {
        let (tx, rx) = mpsc::channel(256);
        let worker = DeadlineWorker::new(pool, bot, config, rx);
        (Self { tx }, worker)
    }

    /// Agenda un nuevo trabajo de alerta crítica.
    pub async fn schedule(
        &self,
        job: DeadlineJob,
    ) -> Result<(), mpsc::error::SendError<DeadlineCommand>> {
        self.tx.send(DeadlineCommand::Schedule(job)).await
    }

    /// Reprograma el plazo efectivo de una tarea.
    pub async fn reschedule(
        &self,
        chat_id: i64,
        assignment_id: i64,
        new_due: i64,
    ) -> Result<(), mpsc::error::SendError<DeadlineCommand>> {
        self.tx
            .send(DeadlineCommand::Reschedule {
                chat_id,
                assignment_id,
                new_due,
            })
            .await
    }

    /// Cancela todos los trabajos de un usuario (al hacer logout o ser desactivado).
    pub async fn cancel_for_user(
        &self,
        chat_id: i64,
    ) -> Result<(), mpsc::error::SendError<DeadlineCommand>> {
        self.tx.send(DeadlineCommand::CancelForUser(chat_id)).await
    }

    /// Cancela un trabajo específico (cuando el usuario entrega la tarea).
    pub async fn cancel_job(
        &self,
        chat_id: i64,
        assignment_id: i64,
    ) -> Result<(), mpsc::error::SendError<DeadlineCommand>> {
        self.tx
            .send(DeadlineCommand::CancelJob {
                chat_id,
                assignment_id,
            })
            .await
    }
}

/// Worker asíncrono que ejecuta la única tarea Tokio del planificador de plazos (A-16).
pub struct DeadlineWorker {
    #[allow(dead_code)]
    pub(crate) pool: SqlitePool,
    #[allow(dead_code)]
    pub(crate) bot: Bot,
    #[allow(dead_code)]
    pub(crate) config: Arc<Config>,
    rx: mpsc::Receiver<DeadlineCommand>,
    heap: BinaryHeap<ScheduledDeadline>,
    versions: HashMap<(i64, i64), u64>,
}

impl DeadlineWorker {
    pub fn new(
        pool: SqlitePool,
        bot: Bot,
        config: Arc<Config>,
        rx: mpsc::Receiver<DeadlineCommand>,
    ) -> Self {
        Self {
            pool,
            bot,
            config,
            rx,
            heap: BinaryHeap::new(),
            versions: HashMap::new(),
        }
    }

    /// Retorna cuántos jobs se encuentran activos en el min-heap.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Inserta un job en el min-heap asignándole un token de versión nuevo.
    pub fn push_job(&mut self, mut job: DeadlineJob) {
        let entry = self
            .versions
            .entry((job.chat_id, job.assignment_id))
            .or_insert(0);
        *entry += 1;
        job.version = *entry;

        // El escalón crítico de 2h debe disparar 2 horas (7200 s) antes de effective_due
        let fire_epoch = job.effective_due - 2 * 3600;
        let fire_at = DateTime::from_timestamp(fire_epoch, 0).unwrap_or_else(Utc::now);

        self.heap.push(ScheduledDeadline { fire_at, job });
    }

    /// Bucle principal de ejecución del planificador (A-16).
    pub async fn run_loop(mut self, mut shutdown_rx: watch::Receiver<bool>) {
        tracing::info!("DeadlineWorker iniciado sobre min-heap.");

        loop {
            let sleep_duration = match self.heap.peek() {
                Some(top) => {
                    let now = Utc::now();
                    if top.fire_at <= now {
                        Duration::ZERO
                    } else {
                        (top.fire_at - now).to_std().unwrap_or(Duration::ZERO)
                    }
                }
                None => {
                    // Si no hay tareas pendientes, esperar hasta nuevo comando o shutdown
                    Duration::from_secs(3600 * 24 * 365)
                }
            };

            let has_scheduled = !self.heap.is_empty();

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    tracing::info!("DeadlineWorker apagado por señal de shutdown.");
                    break;
                }
                cmd_opt = self.rx.recv() => {
                    match cmd_opt {
                        Some(DeadlineCommand::Schedule(job)) => {
                            self.push_job(job);
                        }
                        Some(DeadlineCommand::Reschedule { chat_id, assignment_id, new_due }) => {
                            let job = DeadlineJob {
                                chat_id,
                                assignment_id,
                                effective_due: new_due,
                                version: 0,
                            };
                            self.push_job(job);
                        }
                        Some(DeadlineCommand::CancelForUser(chat_id)) => {
                            self.versions.retain(|&(c_id, _), _| c_id != chat_id);
                        }
                        Some(DeadlineCommand::CancelJob { chat_id, assignment_id }) => {
                            self.versions.remove(&(chat_id, assignment_id));
                        }
                        None => {
                            tracing::info!("Canal de comandos del DeadlineWorker cerrado. Saliendo...");
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(sleep_duration), if has_scheduled => {
                    if let Some(top) = self.heap.pop() {
                        let is_current = self
                            .versions
                            .get(&(top.job.chat_id, top.job.assignment_id))
                            .map(|&v| v == top.job.version)
                            .unwrap_or(false);

                        if !is_current {
                            tracing::debug!(
                                chat_id = top.job.chat_id,
                                assign_id = top.job.assignment_id,
                                "DeadlineJob obsoleto o cancelado; descartando"
                            );
                            continue;
                        }
                    }
                }
            }
        }

        tracing::info!("DeadlineWorker finalizado limpiamente.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_min_heap_ordering() {
        let mut heap = BinaryHeap::new();

        let t1 = DateTime::from_timestamp(1000, 0).unwrap();
        let t2 = DateTime::from_timestamp(2000, 0).unwrap();
        let t3 = DateTime::from_timestamp(1500, 0).unwrap();

        heap.push(ScheduledDeadline {
            fire_at: t2,
            job: DeadlineJob {
                chat_id: 1,
                assignment_id: 2,
                effective_due: 2000 + 7200,
                version: 1,
            },
        });
        heap.push(ScheduledDeadline {
            fire_at: t1,
            job: DeadlineJob {
                chat_id: 1,
                assignment_id: 1,
                effective_due: 1000 + 7200,
                version: 1,
            },
        });
        heap.push(ScheduledDeadline {
            fire_at: t3,
            job: DeadlineJob {
                chat_id: 1,
                assignment_id: 3,
                effective_due: 1500 + 7200,
                version: 1,
            },
        });

        // Min-heap: t1 (1000) sale primero, luego t3 (1500), luego t2 (2000)
        assert_eq!(heap.pop().unwrap().fire_at, t1);
        assert_eq!(heap.pop().unwrap().fire_at, t3);
        assert_eq!(heap.pop().unwrap().fire_at, t2);
    }

    #[tokio::test]
    async fn test_scheduler_worker_shutdown() {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 1125347763,
            database_url: "sqlite::memory:".to_string(),
            max_users_default: 300,
            bot_tz: chrono_tz::America::Managua,
            rust_log: "info".to_string(),
            scan_interval_min: 30,
        });

        let pool = crate::storage::init(&config).await.unwrap();
        let bot = crate::bot::create_bot(&config.bot_token);

        let (scheduler, worker) = DeadlineScheduler::new(pool, bot, config);

        let handle = tokio::spawn(async move {
            worker.run_loop(shutdown_rx).await;
        });

        // Agendar un job futuro
        scheduler
            .schedule(DeadlineJob {
                chat_id: 123,
                assignment_id: 456,
                effective_due: Utc::now().timestamp() + 10000,
                version: 1,
            })
            .await
            .unwrap();

        // Enviar señal de apagado
        shutdown_tx.send(true).unwrap();

        let res = tokio::time::timeout(Duration::from_millis(500), handle).await;
        assert!(res.is_ok(), "Worker no respondió a shutdown a tiempo");
    }
}
