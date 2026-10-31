use crate::bot::{copy, Bot};
use crate::storage::UsersRepository;
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

pub fn parse_json_days(json_str: &str) -> Vec<i64> {
    serde_json::from_str::<Vec<i64>>(json_str).unwrap_or_else(|_| vec![1, 2, 3, 4, 5])
}

pub fn parse_json_times(json_str: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(json_str)
        .unwrap_or_else(|_| vec!["07:30".to_string(), "20:00".to_string()])
}

fn day_name(day: i64) -> &'static str {
    match day {
        1 => "Lun",
        2 => "Mar",
        3 => "Mié",
        4 => "Jue",
        5 => "Vie",
        6 => "Sáb",
        7 | 0 => "Dom",
        _ => "?",
    }
}

pub fn format_days_label(days: &[i64]) -> String {
    if days.is_empty() {
        return "Ninguno".to_string();
    }
    if days.len() == 7 {
        return "Todos los días (Lun-Dom)".to_string();
    }
    if days == [1, 2, 3, 4, 5] {
        return "Lunes a Viernes".to_string();
    }
    let mut sorted = days.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted
        .iter()
        .map(|d| day_name(*d))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn format_times_label(times: &[String]) -> String {
    if times.is_empty() {
        return "Sin horarios asignados".to_string();
    }
    format!("{} ({}× al día)", times.join(", "), times.len())
}

/// Construye la vista de texto y teclado del menú principal de configuración.
pub fn build_main_config_view(
    enabled: bool,
    times: &[String],
    days: &[i64],
) -> (String, InlineKeyboardMarkup) {
    let state_str = if enabled {
        "🟢 <b>Activo</b>"
    } else {
        "🔴 <b>Pausado</b>"
    };
    let toggle_btn_text = if enabled {
        "⏸️ Pausar Resumen"
    } else {
        "▶️ Activar Resumen"
    };

    let text = format!(
        "⚙️ <b>ROBERTO'S WATCHING — CONFIGURACIÓN PERSONAL</b>\n\n\
         📊 <b>Resumen Diario:</b> {}\n\
         ⏰ <b>Horarios:</b> {}\n\
         📅 <b>Días:</b> {}\n\n\
         Selecciona una opción para ajustar tus preferencias:",
        state_str,
        format_times_label(times),
        format_days_label(days)
    );

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("⏰ Modificar Horas", "cfg:menu_hours"),
            InlineKeyboardButton::callback("📅 Modificar Días", "cfg:menu_days"),
        ],
        vec![
            InlineKeyboardButton::callback("🔁 Conmutar Repetición", "cfg:repeat"),
            InlineKeyboardButton::callback(toggle_btn_text, "cfg:toggle_pause"),
        ],
        vec![InlineKeyboardButton::callback(
            "🚪 Cerrar Sesión (/logout)",
            "cfg:logout",
        )],
    ]);

    (text, keyboard)
}

/// Construye el submenú de selección de días activos.
pub fn build_days_menu_view(days: &[i64]) -> (String, InlineKeyboardMarkup) {
    let text = format!(
        "📅 <b>CONFIGURACIÓN DE DÍAS DE RESUMEN</b>\n\n\
         Días activos actuales: <b>{}</b>\n\
         Toca un día para activarlo o desactivarlo:",
        format_days_label(days)
    );

    let mut row1 = Vec::new();
    for d in 1..=4 {
        let active = days.contains(&d);
        let icon = if active { "✅" } else { "❌" };
        let label = format!("{} {}", icon, day_name(d));
        row1.push(InlineKeyboardButton::callback(
            label,
            format!("cfg:day:{}", d),
        ));
    }

    let mut row2 = Vec::new();
    for d in 5..=7 {
        let active = days.contains(&d) || (d == 7 && days.contains(&0));
        let icon = if active { "✅" } else { "❌" };
        let label = format!("{} {}", icon, day_name(d));
        row2.push(InlineKeyboardButton::callback(
            label,
            format!("cfg:day:{}", d),
        ));
    }

    let keyboard = InlineKeyboardMarkup::new(vec![
        row1,
        row2,
        vec![InlineKeyboardButton::callback("⬅️ Volver", "cfg:main")],
    ]);

    (text, keyboard)
}

/// Construye el submenú de selección de horarios fijos.
pub fn build_hours_menu_view(times: &[String]) -> (String, InlineKeyboardMarkup) {
    let text = format!(
        "⏰ <b>CONFIGURACIÓN DE HORARIOS DE RESUMEN</b>\n\n\
         Horarios actuales: <b>{}</b>\n\
         Toca un horario para agregarlo o removerlo:",
        format_times_label(times)
    );

    let available_hours = [
        "07:00", "07:30", "08:00", "12:00", "13:30", "18:00", "20:00", "21:30",
    ];

    let mut row1 = Vec::new();
    for &h in &available_hours[0..3] {
        let active = times.contains(&h.to_string());
        let icon = if active { "✅" } else { "⬜" };
        row1.push(InlineKeyboardButton::callback(
            format!("{} {}", icon, h),
            format!("cfg:hour:{}", h),
        ));
    }

    let mut row2 = Vec::new();
    for &h in &available_hours[3..5] {
        let active = times.contains(&h.to_string());
        let icon = if active { "✅" } else { "⬜" };
        row2.push(InlineKeyboardButton::callback(
            format!("{} {}", icon, h),
            format!("cfg:hour:{}", h),
        ));
    }

    let mut row3 = Vec::new();
    for &h in &available_hours[5..8] {
        let active = times.contains(&h.to_string());
        let icon = if active { "✅" } else { "⬜" };
        row3.push(InlineKeyboardButton::callback(
            format!("{} {}", icon, h),
            format!("cfg:hour:{}", h),
        ));
    }

    let keyboard = InlineKeyboardMarkup::new(vec![
        row1,
        row2,
        row3,
        vec![InlineKeyboardButton::callback("⬅️ Volver", "cfg:main")],
    ]);

    (text, keyboard)
}

/// Maneja el comando interactivo `/config` (RF-07, RF-08).
pub async fn handle_config(bot: Bot, msg: Message, pool: SqlitePool) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id.0;
    let user = UsersRepository::get(&pool, chat_id).await?;

    let user = match user {
        Some(u) if u.is_active => u,
        _ => {
            bot.send_message(
                msg.chat.id,
                "ℹ️ No tienes ninguna sesión activa en Roberto's Watching.\n\
                 Escribe <code>/login &lt;CIF&gt; &lt;CONTRASEÑA&gt;</code> para configurar tus resúmenes.",
            )
            .parse_mode(ParseMode::Html)
            .await?;
            return Ok(());
        }
    };

    let times = parse_json_times(&user.summary_times);
    let days = parse_json_days(&user.summary_days);
    let (text, keyboard) = build_main_config_view(user.summary_enabled, &times, &days);

    bot.send_message(msg.chat.id, text)
        .reply_markup(keyboard)
        .parse_mode(ParseMode::Html)
        .await?;

    Ok(())
}

/// Maneja las interacciones de botones inline (`callback_query`) para `/config` (RF-07, RF-08).
pub async fn handle_config_callback(
    bot: Bot,
    q: CallbackQuery,
    pool: SqlitePool,
) -> Result<(), anyhow::Error> {
    let data = match &q.data {
        Some(d) => d.as_str(),
        None => {
            let _ = bot.answer_callback_query(q.id).await;
            return Ok(());
        }
    };

    if !data.starts_with("cfg:") {
        return Ok(());
    }

    // Responder siempre de inmediato para quitar el spinner de Telegram
    let _ = bot.answer_callback_query(q.id).await;

    let chat_id = q.from.id.0 as i64;
    let user = match UsersRepository::get(&pool, chat_id).await? {
        Some(u) if u.is_active => u,
        _ => return Ok(()),
    };

    let msg = match q.message {
        Some(m) => m,
        None => return Ok(()),
    };

    let mut enabled = user.summary_enabled;
    let mut times = parse_json_times(&user.summary_times);
    let mut days = parse_json_days(&user.summary_days);

    match data {
        "cfg:main" => {
            let (text, keyboard) = build_main_config_view(enabled, &times, &days);
            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .reply_markup(keyboard)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        "cfg:toggle_pause" => {
            enabled = !enabled;
            UsersRepository::set_summary_enabled(&pool, chat_id, enabled).await?;
            let (text, keyboard) = build_main_config_view(enabled, &times, &days);
            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .reply_markup(keyboard)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        "cfg:repeat" => {
            // Rotar 1x -> 2x -> 3x -> 1x
            times = match times.len() {
                1 => vec!["07:30".to_string(), "20:00".to_string()],
                2 => vec![
                    "07:30".to_string(),
                    "13:30".to_string(),
                    "20:00".to_string(),
                ],
                _ => vec!["07:30".to_string()],
            };
            let times_json = serde_json::to_string(&times).unwrap_or_default();
            UsersRepository::set_summary_times(&pool, chat_id, &times_json).await?;
            let (text, keyboard) = build_main_config_view(enabled, &times, &days);
            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .reply_markup(keyboard)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        "cfg:menu_days" => {
            let (text, keyboard) = build_days_menu_view(&days);
            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .reply_markup(keyboard)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        "cfg:menu_hours" => {
            let (text, keyboard) = build_hours_menu_view(&times);
            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .reply_markup(keyboard)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        "cfg:logout" => {
            let _ = crate::bot::handlers::auth::execute_logout(&pool, None, chat_id).await;
            bot.edit_message_text(msg.chat().id, msg.id(), copy::LOGOUT_SUCCESS)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        other if other.starts_with("cfg:day:") => {
            if let Ok(d) = other.trim_start_matches("cfg:day:").parse::<i64>() {
                if let Some(pos) = days.iter().position(|&x| x == d || (d == 7 && x == 0)) {
                    // Mantener al menos 1 día activo
                    if days.len() > 1 {
                        days.remove(pos);
                    }
                } else {
                    days.push(d);
                    days.sort();
                }
                let days_json = serde_json::to_string(&days).unwrap_or_default();
                UsersRepository::set_summary_days(&pool, chat_id, &days_json).await?;
                let (text, keyboard) = build_days_menu_view(&days);
                bot.edit_message_text(msg.chat().id, msg.id(), text)
                    .reply_markup(keyboard)
                    .parse_mode(ParseMode::Html)
                    .await?;
            }
        }
        other if other.starts_with("cfg:hour:") => {
            let h = other.trim_start_matches("cfg:hour:").to_string();
            if let Some(pos) = times.iter().position(|x| x == &h) {
                // Mantener al menos 1 horario activo
                if times.len() > 1 {
                    times.remove(pos);
                }
            } else {
                times.push(h);
                times.sort();
            }
            let times_json = serde_json::to_string(&times).unwrap_or_default();
            UsersRepository::set_summary_times(&pool, chat_id, &times_json).await?;
            let (text, keyboard) = build_hours_menu_view(&times);
            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .reply_markup(keyboard)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_format_config() {
        let days_json = "[1, 2, 3, 4, 5]";
        let days = parse_json_days(days_json);
        assert_eq!(days, vec![1, 2, 3, 4, 5]);
        assert_eq!(format_days_label(&days), "Lunes a Viernes");

        let all_days = vec![1, 2, 3, 4, 5, 6, 7];
        assert_eq!(format_days_label(&all_days), "Todos los días (Lun-Dom)");

        let custom_days = vec![1, 3, 5];
        assert_eq!(format_days_label(&custom_days), "Lun, Mié, Vie");

        let times_json = r#"["07:30", "20:00"]"#;
        let times = parse_json_times(times_json);
        assert_eq!(times, vec!["07:30", "20:00"]);
        assert_eq!(format_times_label(&times), "07:30, 20:00 (2× al día)");
    }

    #[test]
    fn test_build_main_config_view() {
        let times = vec!["07:30".to_string(), "20:00".to_string()];
        let days = vec![1, 2, 3, 4, 5];
        let (text, markup) = build_main_config_view(true, &times, &days);
        assert!(text.contains("🟢 <b>Activo</b>"));
        assert!(text.contains("Lunes a Viernes"));
        assert!(text.contains("07:30, 20:00"));

        for row in markup.inline_keyboard {
            for btn in row {
                if let teloxide::types::InlineKeyboardButtonKind::CallbackData(data) = btn.kind {
                    assert!(data.len() <= 64, "callback_data excede 64 bytes: {}", data);
                }
            }
        }
    }
}
