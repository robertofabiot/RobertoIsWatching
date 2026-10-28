use crate::bot::render::send_chunked_html;
use crate::bot::{copy, Bot};
use crate::storage::{CoursesRepository, UsersRepository};
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Maneja el comando `/cursos` mostrando las asignaturas matriculadas bajo vigilancia activa (RF-08).
pub async fn handle_cursos(bot: Bot, msg: Message, pool: SqlitePool) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let user = UsersRepository::get(&pool, chat_id).await?;

    if user.as_ref().map(|u| !u.is_active).unwrap_or(true) {
        bot.send_message(
            msg.chat.id,
            "ℹ️ No tienes ninguna sesión activa en Roberto's Watching.\n\
             Escribe <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code> para vincular tus cursos.",
        )
        .parse_mode(ParseMode::Html)
        .await?;
        return Ok(());
    }

    let courses = CoursesRepository::courses_for_user(&pool, chat_id).await?;

    if courses.is_empty() {
        bot.send_message(msg.chat.id, copy::CURSOS_EMPTY)
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    let header = copy::cursos_header(courses.len());
    let items: Vec<String> = courses
        .into_iter()
        .map(|c| {
            let short = c
                .shortname
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!(" [<code>{}</code>]", copy::escape_html(&s)))
                .unwrap_or_default();
            format!("📚 <b>{}</b>{}", copy::escape_html(&c.fullname), short)
        })
        .collect();

    send_chunked_html(&bot, msg.chat.id, &header, &items).await?;
    Ok(())
}
