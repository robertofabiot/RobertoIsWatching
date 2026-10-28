use crate::bot::render::send_chunked_html;
use crate::bot::{copy, Bot};
use crate::storage::{SubmissionsRepository, UsersRepository};
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Maneja el comando `/notas` mostrando el desglose de calificaciones y retroalimentación docente (RF-08).
pub async fn handle_notas(bot: Bot, msg: Message, pool: SqlitePool) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let user = UsersRepository::get(&pool, chat_id).await?;

    if user.as_ref().map(|u| !u.is_active).unwrap_or(true) {
        bot.send_message(
            msg.chat.id,
            "ℹ️ No tienes ninguna sesión activa en Roberto's Watching.\n\
             Escribe <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code> para consultar tus calificaciones.",
        )
        .parse_mode(ParseMode::Html)
        .await?;
        return Ok(());
    }

    let graded = SubmissionsRepository::graded_for_user_detailed(&pool, chat_id).await?;

    if graded.is_empty() {
        bot.send_message(msg.chat.id, copy::NOTAS_EMPTY)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    let items: Vec<String> = graded
        .into_iter()
        .map(|item| {
            let grade_val = item.grade.as_deref().unwrap_or("Calificado");
            let fb_part = item
                .feedback
                .as_deref()
                .map(copy::clean_feedback_for_html)
                .filter(|s| !s.is_empty())
                .map(|s| format!("\n   💬 <i>{}</i>", s))
                .unwrap_or_default();

            format!(
                "📝 <b>{}</b>\n   📚 <i>{}</i>\n   🎯 Nota: <b>{}</b>{}",
                copy::escape_html(&item.assignment_name),
                copy::escape_html(&item.course_name),
                copy::escape_html(grade_val),
                fb_part
            )
        })
        .collect();

    send_chunked_html(&bot, msg.chat.id, copy::NOTAS_HEADER, &items).await?;
    Ok(())
}
