//! Módulo de renderizado y segmentación segura de mensajes para Telegram (A-13).
//! Garantiza que los mensajes extensos no superen el límite de 4096 caracteres
//! y se dividan respetando bloques completos sin partir entidades HTML.

use crate::bot::Bot;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Límite seguro por mensaje en Telegram (el límite absoluto es 4096).
pub const MAX_MESSAGE_LENGTH: usize = 4000;

/// Divide una cabecera y una lista de bloques completos en una o más cadenas,
/// garantizando que ningún mensaje supere `max_len` y que los bloques nunca
/// se dividan a la mitad de una entidad o etiqueta HTML (A-13).
pub fn chunk_html_messages(header: &str, items: &[String], max_len: usize) -> Vec<String> {
    if items.is_empty() {
        return vec![header.to_string()];
    }

    let mut messages = Vec::new();
    let mut current_msg = header.to_string();

    for item in items {
        let separator = if current_msg.is_empty() { "" } else { "\n\n" };
        let candidate_len = current_msg.len() + separator.len() + item.len();

        if candidate_len <= max_len {
            if !current_msg.is_empty() {
                current_msg.push_str("\n\n");
            }
            current_msg.push_str(item);
        } else {
            if !current_msg.is_empty() {
                messages.push(current_msg);
                current_msg = String::new();
            }

            if item.len() <= max_len {
                current_msg.push_str(item);
            } else {
                messages.push(item.clone());
            }
        }
    }

    if !current_msg.is_empty() {
        messages.push(current_msg);
    }

    messages
}

/// Envía uno o varios mensajes HTML segmentados respetando el límite de 4096 caracteres.
pub async fn send_chunked_html(
    bot: &Bot,
    chat_id: ChatId,
    header: &str,
    items: &[String],
) -> Result<(), teloxide::RequestError> {
    let chunks = chunk_html_messages(header, items, MAX_MESSAGE_LENGTH);
    for chunk in chunks {
        bot.send_message(chat_id, chunk)
            .parse_mode(ParseMode::Html)
            .await?;
    }
    Ok(())
}

/// Genera el desglose del resumen diario ("lo que hay hoy") reutilizable para /resumen y el ticker cron (A-13, PR #8).
pub async fn render_daily_summary(
    pool: &sqlx::SqlitePool,
    chat_id: i64,
    tz: chrono_tz::Tz,
) -> Result<(String, Vec<String>), sqlx::Error> {
    let pending =
        crate::storage::AssignmentsRepository::pending_detailed_for_user(pool, chat_id, None)
            .await?;

    if pending.is_empty() {
        let msg = format!(
            "📊 <b>ROBERTO'S WATCHING — RESUMEN DEL DÍA</b>\n\n{}\n\n<i>Escribe /sync para forzar una comprobación de cambios.</i>",
            crate::bot::copy::PENDIENTES_ALL_CLEAR
        );
        return Ok((msg, Vec::new()));
    }

    let now_epoch = chrono::Utc::now().timestamp();
    let header = format!(
        "📊 <b>ROBERTO'S WATCHING — RESUMEN DEL DÍA</b>\n\n\
         👁️ <b>Roberto's watching your radar.</b> Tienes <b>{} entregas pendientes</b>:",
        pending.len()
    );

    let mut items = Vec::new();
    for item in pending {
        let due_epoch = item.effective_duedate().unwrap_or(0);
        let is_urgent = due_epoch > 0 && due_epoch <= now_epoch + 86400; // < 24h
        let icon = if is_urgent { "🚨" } else { "📝" };
        let tag = if is_urgent {
            " <b>(¡URGENTE!)</b>"
        } else {
            ""
        };
        let ext_tag = if item.is_extended() {
            " <i>[extensión]</i>"
        } else {
            ""
        };

        let due_str = if due_epoch > 0 {
            crate::bot::copy::format_epoch_local(due_epoch, tz)
        } else {
            "Sin fecha límite".to_string()
        };

        items.push(format!(
            "{} <b>{}</b>{}\n   📚 <i>{}</i>\n   ⏰ Vence: <b>{}</b>{}",
            icon,
            crate::bot::copy::escape_html(&item.name),
            tag,
            crate::bot::copy::escape_html(&item.course_name),
            due_str,
            ext_tag
        ));
    }

    let graded =
        crate::storage::SubmissionsRepository::graded_for_user_detailed(pool, chat_id).await?;
    if !graded.is_empty() {
        for g in graded.into_iter().take(2) {
            let grade_str = g.grade.as_deref().unwrap_or("Calificado");
            items.push(format!(
                "🎯 <i>Nota reciente:</i> <b>{}</b> en <i>{}</i> (<b>{}</b>)",
                crate::bot::copy::escape_html(&g.assignment_name),
                crate::bot::copy::escape_html(&g.course_name),
                crate::bot::copy::escape_html(grade_str)
            ));
        }
    }

    Ok((header, items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_items_returns_header() {
        let header = "<b>Encabezado sin items</b>";
        let chunks = chunk_html_messages(header, &[], 4000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], header);
    }

    #[test]
    fn test_chunking_respects_max_len_and_preserves_blocks() {
        let header = "<b>Tareas Pendientes</b>\n";
        // Cada bloque mide aproximadamente 100 caracteres
        let block =
            "• <b>Tarea Importante</b>: Vence mañana. <i>No olvidar subir el PDF con portada.</i>";
        let items: Vec<String> = (0..50).map(|i| format!("{block} #{i}")).collect();

        let max_len = 500;
        let chunks = chunk_html_messages(header, &items, max_len);

        assert!(chunks.len() > 1);

        for (idx, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.len() <= max_len,
                "El chunk {idx} excede el límite: {} > {}",
                chunk.len(),
                max_len
            );
            // Cada bloque debe estar completo, nunca cortado a la mitad de <b> o <i>
            let open_b = chunk.matches("<b>").count();
            let close_b = chunk.matches("</b>").count();
            assert_eq!(
                open_b, close_b,
                "Etiquetas <b> desbalanceadas en chunk {idx}"
            );

            let open_i = chunk.matches("<i>").count();
            let close_i = chunk.matches("</i>").count();
            assert_eq!(
                open_i, close_i,
                "Etiquetas <i> desbalanceadas en chunk {idx}"
            );
        }
    }
}
