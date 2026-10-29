//! Planificador centralizado de plazos de tareas sobre un min-heap (PR #7, Plan Maestro §5.1, A-16).
//! Gestiona una única tarea Tokio con `BinaryHeap` y re-armado reactivo vía canal MPSC.

use crate::bot::{copy, Bot};
use crate::config::Config;
use crate::services::alerts::AlertService;
use crate::storage::assignments::AssignmentsRepository;
use crate::storage::submissions::SubmissionsRepository;
use crate::storage::users::UsersRepository;
use crate::storage::AlertsRepository;
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

/// Elemento en el BinaryHeap ordenado como min-heap por `fire_instant` (menor tiempo primero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledDeadline {
    pub fire_instant: tokio::time::Instant,
    pub fire_at: DateTime<Utc>,
    pub job: DeadlineJob,
}

impl Ord for ScheduledDeadline {
    fn cmp(&self, other: &Self) -> Ordering {
        // Invertido para min-heap: el fire_instant más cercano se despacha primero
        other.fire_instant.cmp(&self.fire_instant)
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
    pub(crate) pool: SqlitePool,
    pub(crate) bot: Bot,
    pub(crate) config: Arc<Config>,
    rx: mpsc::Receiver<DeadlineCommand>,
    heap: BinaryHeap<ScheduledDeadline>,
    versions: HashMap<(i64, i64), u64>,
    booted: bool,
    start_instant: tokio::time::Instant,
    start_epoch: i64,
}

/// Resumen de operaciones realizadas durante la reconstrucción inicial del min-heap (boot lookahead).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BootLookaheadSummary {
    pub jobs_scheduled: usize,
    pub alerts_dispatched: usize,
    pub expired_discarded: usize,
    pub already_claimed_skipped: usize,
}

#[derive(Debug, sqlx::FromRow)]
struct BootLookaheadRow {
    chat_id: i64,
    assignment_id: i64,
    assignment_name: String,
    effective_due: i64,
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
            booted: false,
            start_instant: tokio::time::Instant::now(),
            start_epoch: Utc::now().timestamp(),
        }
    }

    /// Retorna el epoch timestamp actual correlacionado con el reloj virtual/real de Tokio.
    pub fn current_epoch(&self) -> i64 {
        let elapsed = tokio::time::Instant::now().saturating_duration_since(self.start_instant);
        self.start_epoch + elapsed.as_secs() as i64
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

        let now_epoch = self.current_epoch();
        let fire_instant = if fire_epoch <= now_epoch {
            tokio::time::Instant::now()
        } else {
            let delay_secs = (fire_epoch - now_epoch) as u64;
            tokio::time::Instant::now() + Duration::from_secs(delay_secs)
        };

        self.heap.push(ScheduledDeadline {
            fire_instant,
            fire_at,
            job,
        });
    }

    /// Reconstruye el min-heap desde SQLite al arrancar el bot (boot lookahead, Plan Maestro §5.1.3).
    /// Las tareas dentro de la ventana de 2h no enviadas se disparan; las tareas futuras se agendan en el heap;
    /// las tareas vencidas se descartan silenciosamente.
    pub async fn boot_lookahead(&mut self) -> Result<BootLookaheadSummary, anyhow::Error> {
        if self.booted {
            return Ok(BootLookaheadSummary::default());
        }
        self.booted = true;
        tracing::info!("Iniciando reconstrucción del min-heap desde SQLite (boot lookahead)...");
        let mut summary = BootLookaheadSummary::default();

        let rows = sqlx::query_as::<_, BootLookaheadRow>(
            "SELECT \
                uc.chat_id, \
                a.id as assignment_id, \
                a.name as assignment_name, \
                COALESCE(us.due_override, a.duedate) as effective_due \
             FROM assignments a \
             INNER JOIN user_courses uc ON a.course_id = uc.course_id \
             INNER JOIN users u ON uc.chat_id = u.chat_id \
             LEFT JOIN user_submissions us ON a.id = us.assignment_id AND us.chat_id = uc.chat_id \
             WHERE u.is_active = 1 \
               AND COALESCE(us.due_override, a.duedate) IS NOT NULL \
               AND COALESCE(us.due_override, a.duedate) > 0 \
               AND (us.status IS NULL OR us.status != 'submitted') \
               AND (us.graded IS NULL OR us.graded = 0) \
               AND (us.grade IS NULL)",
        )
        .fetch_all(&self.pool)
        .await?;

        let now_epoch = self.current_epoch();

        for row in rows {
            if row.effective_due <= now_epoch {
                // Tarea vencida: se descarta (§5.1.3)
                summary.expired_discarded += 1;
                continue;
            }

            let remaining = row.effective_due - now_epoch;

            if remaining <= 2 * 3600 {
                // Dentro de la ventana crítica de 2h
                let alert_key = format!("deadline:{}:{}:2h", row.chat_id, row.assignment_id);
                let already_claimed = AlertsRepository::is_claimed(&self.pool, &alert_key).await?;

                if already_claimed {
                    summary.already_claimed_skipped += 1;
                } else {
                    let due_time_str =
                        copy::format_epoch_time_only(row.effective_due, self.config.bot_tz);
                    let msg = copy::alert_deadline_2h(&row.assignment_name, &due_time_str);

                    let dispatched = AlertService::dispatch(
                        &self.pool,
                        &self.bot,
                        &alert_key,
                        "deadline_2h",
                        row.chat_id,
                        &msg,
                    )
                    .await?;

                    if dispatched {
                        summary.alerts_dispatched += 1;
                    }
                }
            } else {
                // Tarea futura fuera de la ventana de 2h: agendar en min-heap
                self.push_job(DeadlineJob {
                    chat_id: row.chat_id,
                    assignment_id: row.assignment_id,
                    effective_due: row.effective_due,
                    version: 0,
                });
                summary.jobs_scheduled += 1;
            }
        }

        tracing::info!(
            jobs_scheduled = summary.jobs_scheduled,
            alerts_dispatched = summary.alerts_dispatched,
            expired_discarded = summary.expired_discarded,
            already_claimed = summary.already_claimed_skipped,
            "Boot lookahead completado exitosamente"
        );

        Ok(summary)
    }

    /// Bucle principal de ejecución del planificador (A-16).
    pub async fn run_loop(mut self, mut shutdown_rx: watch::Receiver<bool>) {
        tracing::info!("DeadlineWorker iniciado sobre min-heap.");

        if let Err(e) = self.boot_lookahead().await {
            tracing::error!(error = %e, "Error al ejecutar boot lookahead al iniciar DeadlineWorker");
        }

        loop {
            let next_instant = match self.heap.peek() {
                Some(top) => top.fire_instant,
                None => tokio::time::Instant::now() + Duration::from_secs(3600 * 24 * 365),
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
                _ = tokio::time::sleep_until(next_instant), if has_scheduled => {
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

                        // Re-verificación en SQLite antes de despachar alerta crítica de 2h
                        if let Err(e) = self.process_deadline_alert(top.job).await {
                            tracing::error!(
                                error = %e,
                                "Error al procesar alerta crítica de plazo de 2h"
                            );
                        }
                    }
                }
            }
        }

        tracing::info!("DeadlineWorker finalizado limpiamente.");
    }

    /// Re-verifica en SQLite las condiciones de la tarea y el alumno antes de emitir la alerta crítica (2h).
    pub async fn process_deadline_alert(&self, job: DeadlineJob) -> Result<(), anyhow::Error> {
        // 1. ¿Alumno sigue activo?
        let user_opt = UsersRepository::get(&self.pool, job.chat_id).await?;
        let user = match user_opt {
            Some(u) if u.is_active => u,
            _ => {
                tracing::debug!(
                    chat_id = job.chat_id,
                    "Usuario inactivo o no existe; omitiendo alerta de 2h"
                );
                return Ok(());
            }
        };

        // 2. ¿Tarea entregada o calificada?
        let sub_opt =
            SubmissionsRepository::get(&self.pool, job.chat_id, job.assignment_id).await?;
        if let Some(ref sub) = sub_opt {
            let is_submitted = sub.status.as_deref() == Some("submitted");
            let is_graded = sub.graded || sub.grade.is_some();
            if is_submitted || is_graded {
                tracing::debug!(
                    chat_id = job.chat_id,
                    assign_id = job.assignment_id,
                    "Tarea ya entregada o calificada; omitiendo alerta de 2h"
                );
                return Ok(());
            }
        }

        // 3. ¿Existe la tarea y se mantuvo el plazo?
        let assign_opt = AssignmentsRepository::get(&self.pool, job.assignment_id).await?;
        let assign = match assign_opt {
            Some(a) => a,
            None => {
                tracing::debug!(
                    assign_id = job.assignment_id,
                    "Tarea no encontrada en BD; omitiendo alerta de 2h"
                );
                return Ok(());
            }
        };

        let effective_due = sub_opt
            .as_ref()
            .and_then(|s| s.due_override)
            .or(assign.duedate)
            .unwrap_or(0);

        if effective_due != job.effective_due {
            tracing::debug!(
                chat_id = job.chat_id,
                assign_id = job.assignment_id,
                old_due = job.effective_due,
                new_due = effective_due,
                "El plazo de la tarea cambió; descartando job desactualizado"
            );
            return Ok(());
        }

        let now_epoch = self.current_epoch();
        // Si ya está totalmente vencida (ej. se despertó tarde tras caída prolongada), no enviar
        if now_epoch >= effective_due {
            tracing::debug!(
                chat_id = job.chat_id,
                assign_id = job.assignment_id,
                "Plazo ya vencido; descartando alerta de 2h"
            );
            return Ok(());
        }

        // 4. Despachar alerta de 2h
        let due_time_str = copy::format_epoch_time_only(effective_due, self.config.bot_tz);
        let msg = copy::alert_deadline_2h(&assign.name, &due_time_str);
        let alert_key = format!("deadline:{}:{}:2h", job.chat_id, job.assignment_id);

        let dispatched = AlertService::dispatch(
            &self.pool,
            &self.bot,
            &alert_key,
            "deadline_2h",
            user.chat_id,
            &msg,
        )
        .await?;

        if dispatched {
            tracing::info!(
                chat_id = job.chat_id,
                assign_id = job.assignment_id,
                "Alerta crítica de 2h despachada con éxito"
            );
        }

        Ok(())
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

        let now_inst = tokio::time::Instant::now();

        heap.push(ScheduledDeadline {
            fire_instant: now_inst + Duration::from_secs(2000),
            fire_at: t2,
            job: DeadlineJob {
                chat_id: 1,
                assignment_id: 2,
                effective_due: 2000 + 7200,
                version: 1,
            },
        });
        heap.push(ScheduledDeadline {
            fire_instant: now_inst + Duration::from_secs(1000),
            fire_at: t1,
            job: DeadlineJob {
                chat_id: 1,
                assignment_id: 1,
                effective_due: 1000 + 7200,
                version: 1,
            },
        });
        heap.push(ScheduledDeadline {
            fire_instant: now_inst + Duration::from_secs(1500),
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

    #[tokio::test]
    async fn test_reschedule_invalidates_earlier_version() {
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
        let (_scheduler, mut worker) = DeadlineScheduler::new(pool, bot, config);

        // Agendar versión 1 con due = 10000 (fire_at = 2800)
        worker.push_job(DeadlineJob {
            chat_id: 1,
            assignment_id: 10,
            effective_due: 10000,
            version: 0,
        });
        assert_eq!(worker.versions.get(&(1, 10)), Some(&1));

        // Reprogramar fecha con due = 20000 (fire_at = 12800) -> versión 2
        worker.push_job(DeadlineJob {
            chat_id: 1,
            assignment_id: 10,
            effective_due: 20000,
            version: 0,
        });
        assert_eq!(worker.versions.get(&(1, 10)), Some(&2));

        // El heap tiene 2 elementos: el primero que sale es el de due=10000 (versión 1)
        assert_eq!(worker.len(), 2);
        let popped_1 = worker.heap.pop().unwrap();
        assert_eq!(popped_1.job.version, 1);
        // Debe ser inválido porque la versión actual en el mapa es 2
        let is_current_1 = worker
            .versions
            .get(&(popped_1.job.chat_id, popped_1.job.assignment_id))
            .map(|&v| v == popped_1.job.version)
            .unwrap_or(false);
        assert!(
            !is_current_1,
            "El job con versión 1 debe considerarse obsoleto"
        );

        // El segundo que sale es el de due=20000 (versión 2)
        let popped_2 = worker.heap.pop().unwrap();
        assert_eq!(popped_2.job.version, 2);
        let is_current_2 = worker
            .versions
            .get(&(popped_2.job.chat_id, popped_2.job.assignment_id))
            .map(|&v| v == popped_2.job.version)
            .unwrap_or(false);
        assert!(
            is_current_2,
            "El job con versión 2 debe ser válido y vigente"
        );
    }

    #[tokio::test]
    async fn test_process_deadline_alert_verification_in_sqlite() {
        use crate::bot::create_bot_with_url;
        use crate::storage::submissions::SubmissionData;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let telegram_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 999,
                    "date": 1700000000,
                    "chat": { "id": 1001, "type": "private" },
                    "text": "alerta deadline 2h"
                }
            })))
            .mount(&telegram_server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_deadline_verify_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 1125347763,
            database_url,
            max_users_default: 300,
            bot_tz: chrono_tz::America::Managua,
            rust_log: "info".to_string(),
            scan_interval_min: 30,
        });

        let pool = crate::storage::init(&config).await.unwrap();
        let bot = create_bot_with_url(
            &config.bot_token,
            reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
        );

        let (_scheduler, worker) = DeadlineScheduler::new(pool.clone(), bot, config);

        // Curso 50
        crate::storage::CoursesRepository::upsert_course(&pool, 50, "Curso 50", Some("C50"))
            .await
            .unwrap();

        // Alumno 1001 (Activo)
        UsersRepository::upsert_login(&pool, 1001, "24011001", 101, "Alumno 1", "tok_1")
            .await
            .unwrap();
        // Alumno 1002 (Inactivo)
        UsersRepository::upsert_login(&pool, 1002, "24011002", 102, "Alumno 2", "tok_2")
            .await
            .unwrap();
        UsersRepository::mark_inactive(&pool, 1002, "bot_blocked")
            .await
            .unwrap();

        let now = Utc::now().timestamp();
        let due = now + 7200; // vence en 2 horas

        // Caso A: Tarea 2001 (Pendiente, vence en 2h) -> DEBE despachar
        AssignmentsRepository::upsert(&pool, 2001, Some(20010), 50, "Tarea 2001", Some(due), None)
            .await
            .unwrap();
        let job_a = DeadlineJob {
            chat_id: 1001,
            assignment_id: 2001,
            effective_due: due,
            version: 1,
        };
        worker.process_deadline_alert(job_a).await.unwrap();

        let claimed_a =
            crate::storage::AlertsRepository::is_claimed(&pool, "deadline:1001:2001:2h")
                .await
                .unwrap();
        assert!(
            claimed_a,
            "Alerta 2h debió despacharse para tarea pendiente"
        );

        // Caso B: Tarea 2002 (Ya entregada) -> DEBE omitir
        AssignmentsRepository::upsert(&pool, 2002, Some(20020), 50, "Tarea 2002", Some(due), None)
            .await
            .unwrap();
        SubmissionsRepository::upsert(
            &pool,
            SubmissionData {
                chat_id: 1001,
                assignment_id: 2002,
                status: Some("submitted"),
                graded: false,
                grade: None,
                feedback: None,
                due_override: None,
            },
        )
        .await
        .unwrap();

        let job_b = DeadlineJob {
            chat_id: 1001,
            assignment_id: 2002,
            effective_due: due,
            version: 1,
        };
        worker.process_deadline_alert(job_b).await.unwrap();

        let claimed_b =
            crate::storage::AlertsRepository::is_claimed(&pool, "deadline:1001:2002:2h")
                .await
                .unwrap();
        assert!(
            !claimed_b,
            "Alerta 2h debió omitirse porque ya fue entregada"
        );

        // Caso C: Tarea 2003 pero Alumno inactivo (1002) -> DEBE omitir
        AssignmentsRepository::upsert(&pool, 2003, Some(20030), 50, "Tarea 2003", Some(due), None)
            .await
            .unwrap();
        let job_c = DeadlineJob {
            chat_id: 1002,
            assignment_id: 2003,
            effective_due: due,
            version: 1,
        };
        worker.process_deadline_alert(job_c).await.unwrap();

        let claimed_c =
            crate::storage::AlertsRepository::is_claimed(&pool, "deadline:1002:2003:2h")
                .await
                .unwrap();
        assert!(!claimed_c, "Alerta 2h debió omitirse para usuario inactivo");

        // Caso D: Tarea 2004 pero el plazo fue modificado en la BD -> DEBE omitir job viejo
        AssignmentsRepository::upsert(
            &pool,
            2004,
            Some(20040),
            50,
            "Tarea 2004",
            Some(due + 3600), // Se extendió 1 hora
            None,
        )
        .await
        .unwrap();
        let job_d = DeadlineJob {
            chat_id: 1001,
            assignment_id: 2004,
            effective_due: due, // job con plazo anterior
            version: 1,
        };
        worker.process_deadline_alert(job_d).await.unwrap();

        let claimed_d =
            crate::storage::AlertsRepository::is_claimed(&pool, "deadline:1001:2004:2h")
                .await
                .unwrap();
        assert!(
            !claimed_d,
            "Alerta 2h debió omitirse porque el plazo cambió en la base de datos"
        );
    }

    #[tokio::test]
    async fn test_boot_lookahead_rebuilds_heap_and_dispatches_window_alerts() {
        use crate::bot::create_bot_with_url;
        use crate::storage::submissions::SubmissionData;
        use crate::storage::CoursesRepository;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let telegram_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(wiremock::matchers::path_regex(r"(?i).*sendmessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 888,
                    "date": 1700000000,
                    "chat": { "id": 5001, "type": "private" },
                    "text": "alerta deadline 2h lookahead"
                }
            })))
            .mount(&telegram_server)
            .await;

        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_boot_lookahead_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Arc::new(Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 1125347763,
            database_url,
            max_users_default: 300,
            bot_tz: chrono_tz::America::Managua,
            rust_log: "info".to_string(),
            scan_interval_min: 30,
        });

        let pool = crate::storage::init(&config).await.unwrap();
        let bot = create_bot_with_url(
            &config.bot_token,
            reqwest::Url::parse(&format!("{}/", telegram_server.uri())).unwrap(),
        );

        // Curso 300
        CoursesRepository::upsert_course(&pool, 300, "Sistemas Operativos", Some("SO"))
            .await
            .unwrap();

        // Alumno 5001
        UsersRepository::upsert_login(&pool, 5001, "24015001", 501, "Estudiante SO", "tok_so")
            .await
            .unwrap();
        CoursesRepository::link_user(&pool, 5001, 300)
            .await
            .unwrap();

        let now = Utc::now().timestamp();

        // 1. Tarea 3001: vence en 5 horas -> FUERA de la ventana 2h -> debe agendarse en el heap
        AssignmentsRepository::upsert(
            &pool,
            3001,
            Some(30010),
            300,
            "Proyecto SO (5h)",
            Some(now + 5 * 3600),
            None,
        )
        .await
        .unwrap();

        // 2. Tarea 3002: vence en 1 hora -> DENTRO de la ventana 2h, no enviada -> debe dispararse
        AssignmentsRepository::upsert(
            &pool,
            3002,
            Some(30020),
            300,
            "Lab 1 SO (1h)",
            Some(now + 3600),
            None,
        )
        .await
        .unwrap();

        // 3. Tarea 3003: vence en 1 hora -> DENTRO de la ventana 2h, pero YA ENVIADA previamente
        AssignmentsRepository::upsert(
            &pool,
            3003,
            Some(30030),
            300,
            "Lab 2 SO (1h enviado)",
            Some(now + 3600),
            None,
        )
        .await
        .unwrap();
        AlertsRepository::try_claim(&pool, "deadline:5001:3003:2h", "deadline_2h", Some(5001))
            .await
            .unwrap();

        // 4. Tarea 3004: vencida hace 2 horas -> DEBE DESCARTARSE (§5.1.3)
        AssignmentsRepository::upsert(
            &pool,
            3004,
            Some(30040),
            300,
            "Lab Vencido (-2h)",
            Some(now - 7200),
            None,
        )
        .await
        .unwrap();

        // 5. Tarea 3005: vence en 5 horas pero ya entregada
        AssignmentsRepository::upsert(
            &pool,
            3005,
            Some(30050),
            300,
            "Lab Entregado (5h)",
            Some(now + 5 * 3600),
            None,
        )
        .await
        .unwrap();
        SubmissionsRepository::upsert(
            &pool,
            SubmissionData {
                chat_id: 5001,
                assignment_id: 3005,
                status: Some("submitted"),
                graded: false,
                grade: None,
                feedback: None,
                due_override: None,
            },
        )
        .await
        .unwrap();

        let (_scheduler, mut worker) = DeadlineScheduler::new(pool.clone(), bot, config);

        // Ejecutar boot lookahead
        let summary = worker.boot_lookahead().await.unwrap();

        assert_eq!(summary.jobs_scheduled, 1, "Debe agendar solo 3001 en heap");
        assert_eq!(
            summary.alerts_dispatched, 1,
            "Debe despachar 3002 dentro de 2h"
        );
        assert_eq!(
            summary.already_claimed_skipped, 1,
            "Debe omitir 3003 ya enviada"
        );
        assert_eq!(
            summary.expired_discarded, 1,
            "Debe descartar 3004 ya vencida"
        );

        // El heap debe contener exactamente 1 job (3001)
        assert_eq!(worker.len(), 1);

        // La alerta de 3002 debe estar registrada en sent_alerts
        let claimed_3002 = AlertsRepository::is_claimed(&pool, "deadline:5001:3002:2h")
            .await
            .unwrap();
        assert!(
            claimed_3002,
            "Alerta 3002 debe estar en sent_alerts tras boot lookahead"
        );
    }

    #[tokio::test]
    async fn test_drop_jobs_on_cancel_for_user() {
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
        let (_scheduler, mut worker) = DeadlineScheduler::new(pool, bot, config);

        // Agendar 2 jobs para alumno 6001
        worker.push_job(DeadlineJob {
            chat_id: 6001,
            assignment_id: 11,
            effective_due: 10000,
            version: 0,
        });
        worker.push_job(DeadlineJob {
            chat_id: 6001,
            assignment_id: 12,
            effective_due: 15000,
            version: 0,
        });

        // Agendar 1 job para alumno 6002
        worker.push_job(DeadlineJob {
            chat_id: 6002,
            assignment_id: 13,
            effective_due: 12000,
            version: 0,
        });

        assert_eq!(worker.versions.len(), 3);

        // Cancelar todos los trabajos del alumno 6001 (logout o desactivación)
        worker.versions.retain(|&(c_id, _), _| c_id != 6001);

        assert_eq!(worker.versions.len(), 1);
        assert!(!worker.versions.contains_key(&(6001, 11)));
        assert!(!worker.versions.contains_key(&(6001, 12)));
        assert_eq!(worker.versions.get(&(6002, 13)), Some(&1));

        // Al sacar los elementos del heap:
        // Los de 6001 deben evaluarse como obsoletos/cancelados
        while let Some(top) = worker.heap.pop() {
            let is_current = worker
                .versions
                .get(&(top.job.chat_id, top.job.assignment_id))
                .map(|&v| v == top.job.version)
                .unwrap_or(false);

            if top.job.chat_id == 6001 {
                assert!(
                    !is_current,
                    "Los jobs de 6001 deben considerarse cancelados"
                );
            } else if top.job.chat_id == 6002 {
                assert!(is_current, "Los jobs de 6002 deben mantenerse vigentes");
            }
        }
    }
}
