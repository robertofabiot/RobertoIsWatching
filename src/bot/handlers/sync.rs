use crate::bot::{copy, Bot};
use crate::moodle::gate::Priority;
use crate::moodle::MoodleClient;
use crate::services::{session, sync};
use crate::storage::{CoursesRepository, UsersRepository};
use chrono::Utc;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Cooldown mínimo de 10 minutos entre escaneos interactivos a Moodle (RF-08).
pub const SYNC_COOLDOWN_SECONDS: i64 = 600;

/// Conjunto en memoria de chat_ids con sincronización interactiva en curso (R-15).
pub(crate) static ACTIVE_SYNCS: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Registro en memoria del último escaneo interactivo completado por chat_id (R-15).
pub(crate) static SYNC_COOLDOWNS: LazyLock<Mutex<HashMap<i64, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Guardián RAII para asegurar que el chat_id se libere de ACTIVE_SYNCS al finalizar o fallar.
struct SyncGuard(i64);

impl Drop for SyncGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = ACTIVE_SYNCS.lock() {
            set.remove(&self.0);
        }
    }
}

/// Maneja el comando `/sync` forzando sincronización con Moodle respetando el cooldown anti-ansiedad (RF-08).
pub async fn handle_sync(
    bot: Bot,
    msg: Message,
    pool: SqlitePool,
    moodle: MoodleClient,
) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let user = UsersRepository::get(&pool, chat_id).await?;

    if user.as_ref().map(|u| !u.is_active).unwrap_or(true) {
        bot.send_message(
            msg.chat.id,
            "ℹ️ No tienes ninguna sesión activa en Roberto's Watching.\n\
             Escribe <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code> para activar la sincronización.",
        )
        .parse_mode(ParseMode::Html)
        .await?;
        return Ok(());
    }

    // Prevención de TOCTOU (R-15): adquirir lock atómico por chat_id antes de verificar cooldown
    let already_running = {
        let mut set = ACTIVE_SYNCS.lock().unwrap();
        !set.insert(chat_id)
    };
    if already_running {
        // Ya hay una sincronización interactiva en curso para este usuario
        let cooldown_msg = copy::sync_cooldown_message(1);
        bot.send_message(msg.chat.id, cooldown_msg)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }
    let _guard = SyncGuard(chat_id);

    // 1. Comprobar cooldown anti-ansiedad en memoria de escaneos interactivos recientes (R-15)
    let in_memory_cooldown = {
        let mut cd_map = SYNC_COOLDOWNS.lock().unwrap();
        let now = Instant::now();
        cd_map.retain(|_, &mut t| {
            now.duration_since(t) < Duration::from_secs(SYNC_COOLDOWN_SECONDS as u64)
        });
        if let Some(&last_t) = cd_map.get(&chat_id) {
            let elapsed_secs = now.duration_since(last_t).as_secs() as i64;
            if elapsed_secs < SYNC_COOLDOWN_SECONDS {
                Some((elapsed_secs / 60).max(1))
            } else {
                None
            }
        } else {
            None
        }
    };

    if let Some(mins_ago) = in_memory_cooldown {
        let cooldown_msg = copy::sync_cooldown_message(mins_ago);
        bot.send_message(msg.chat.id, cooldown_msg)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    // 2. Comprobar cooldown anti-ansiedad de 10 minutos desde SQLite (escaneo de fondo previo) (A-08, RF-08)
    let latest_scan = CoursesRepository::latest_scan_for_user(&pool, chat_id).await?;

    if let Some(last_dt) = latest_scan {
        let now_utc = Utc::now().naive_utc();
        let elapsed = now_utc.signed_duration_since(last_dt);

        if elapsed.num_seconds() >= 0 && elapsed.num_seconds() < SYNC_COOLDOWN_SECONDS {
            let minutes_ago = elapsed.num_minutes().max(1);
            let cooldown_msg = copy::sync_cooldown_message(minutes_ago);
            bot.send_message(msg.chat.id, cooldown_msg)
                .parse_mode(ParseMode::Html)
                .await?;
            return Ok(());
        }
    }

    // 3. Cooldown superado o primer escaneo: Enviar mensaje de búsqueda
    bot.send_message(msg.chat.id, copy::SYNC_SCANNING)
        .parse_mode(ParseMode::Html)
        .await?;

    // 4. Ejecutar pipeline de refresco interactivo
    match sync::refresh_user(&pool, &moodle, chat_id, Priority::Interactive).await {
        Ok(summary) => {
            // Registrar marca de tiempo del escaneo exitoso en el limitador en memoria
            if let Ok(mut cd_map) = SYNC_COOLDOWNS.lock() {
                cd_map.insert(chat_id, Instant::now());
            }

            if !summary.has_changes() {
                bot.send_message(msg.chat.id, copy::SYNC_NO_CHANGES)
                    .parse_mode(ParseMode::Html)
                    .await?;
            } else {
                let report = format!(
                    "✅ <b>Sincronización completada:</b>\n\
                     • <b>{}</b> tareas nuevas detectadas\n\
                     • <b>{}</b> plazos actualizados\n\
                     • <b>{}</b> calificaciones nuevas registradas\n\n\
                     Escribe <code>/pendientes</code> para ver tus entregas.",
                    summary.new_assignments, summary.updated_assignments, summary.new_grades
                );
                bot.send_message(msg.chat.id, report)
                    .parse_mode(ParseMode::Html)
                    .await?;
            }
        }
        Err(err) => {
            tracing::warn!(chat_id = chat_id, error = %err, "Error al sincronizar con Moodle vía /sync");

            if let Some(moodle_err) = err.downcast_ref::<crate::moodle::error::MoodleError>() {
                if moodle_err.is_token_error() {
                    let _ = session::expire_user(&pool, &bot, chat_id).await;
                    return Ok(());
                }
            }

            bot.send_message(msg.chat.id, copy::UAM_MAINTENANCE)
                .parse_mode(ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_guard_prevents_concurrent_runs() {
        let chat_id = 999123;
        let guard = {
            let mut set = ACTIVE_SYNCS.lock().unwrap();
            assert!(set.insert(chat_id));
            SyncGuard(chat_id)
        };
        // Segundo intento simultáneo para el mismo chat_id debe fallar
        {
            let mut set = ACTIVE_SYNCS.lock().unwrap();
            assert!(!set.insert(chat_id));
        }
        // Tras soltar el guard, el chat_id queda libre nuevamente
        drop(guard);
        let set = ACTIVE_SYNCS.lock().unwrap();
        assert!(!set.contains(&chat_id));
    }

    #[test]
    fn test_sync_cooldown_prevents_rapid_repeated_syncs() {
        let chat_id = 999456;
        let now = Instant::now();

        // 1. Simular registro de sincronización completada exitosamente
        {
            let mut cd_map = SYNC_COOLDOWNS.lock().unwrap();
            cd_map.insert(chat_id, now);
        }

        // 2. Verificar que el cooldown detecta la sincronización reciente
        {
            let cd_map = SYNC_COOLDOWNS.lock().unwrap();
            let last_t = cd_map.get(&chat_id).unwrap();
            let elapsed = now.duration_since(*last_t).as_secs() as i64;
            assert!(elapsed < SYNC_COOLDOWN_SECONDS);
        }

        // 3. Simular que transcurrieron 601 segundos (> 10 minutos)
        {
            let mut cd_map = SYNC_COOLDOWNS.lock().unwrap();
            cd_map.insert(chat_id, now - Duration::from_secs(601));
            let past_t = cd_map.get(&chat_id).unwrap();
            let elapsed = now.duration_since(*past_t).as_secs() as i64;
            assert!(elapsed >= SYNC_COOLDOWN_SECONDS);
        }
    }
}
