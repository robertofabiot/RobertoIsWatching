use crate::bot::Bot;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

/// Catálogo de comandos públicos para estudiantes (RF-08).
#[derive(BotCommands, Clone, Debug, PartialEq, Eq)]
#[command(
    rename_rule = "lowercase",
    description = "Comandos disponibles en Roberto's Watching:"
)]
pub enum Command {
    #[command(description = "Mensaje de bienvenida e inicio")]
    Start,
    #[command(description = "Catálogo de ayuda y comandos")]
    Ayuda,
    #[command(description = "Vinculación segura: /login <CIF> <CONTRASEÑA>")]
    Login(String),
    #[command(description = "Asignaturas matriculadas bajo vigilancia 24/7")]
    Cursos,
    #[command(description = "Lista de entregas y tareas pendientes")]
    Pendientes,
    #[command(description = "Desglose de calificaciones y feedback reciente")]
    Notas,
    #[command(description = "Resumen académico del día")]
    Resumen,
    #[command(description = "Fuerza sincronización con Moodle (cooldown de 10 min)")]
    Sync,
    #[command(description = "Ajusta horarios y frecuencia de tus resúmenes")]
    Config,
    #[command(description = "Cierre de sesión y purga de datos")]
    Logout,
}

/// Comandos exclusivos para el administrador (`ADMIN_CHAT_ID`) (RF-10).
#[derive(BotCommands, Clone, Debug, PartialEq, Eq)]
#[command(rename_rule = "snake_case")]
pub enum AdminCommand {
    AdminStats,
    ToggleRegistro,
    SetMaxUsers(u32),
}

/// Registra el catálogo público de comandos en la interfaz del cliente Telegram (`set_my_commands`) (RF-08).
pub async fn register_commands(bot: &Bot) -> Result<(), teloxide::RequestError> {
    let mut commands = Command::bot_commands();
    // Telegram API exige que el campo 'command' no tenga el prefijo '/'
    for cmd in &mut commands {
        if cmd.command.starts_with('/') {
            cmd.command = cmd.command.trim_start_matches('/').to_string();
        }
    }
    bot.set_my_commands(commands).await?;
    tracing::info!("Menú de comandos de Telegram registrado exitosamente.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_commands() {
        assert_eq!(Command::parse("/start", "").unwrap(), Command::Start);
        assert_eq!(Command::parse("/ayuda", "").unwrap(), Command::Ayuda);
        assert_eq!(
            Command::parse("/login 24014121 MiClave123", "").unwrap(),
            Command::Login("24014121 MiClave123".to_string())
        );
        assert_eq!(Command::parse("/cursos", "").unwrap(), Command::Cursos);
        assert_eq!(
            Command::parse("/pendientes", "").unwrap(),
            Command::Pendientes
        );
        assert_eq!(Command::parse("/notas", "").unwrap(), Command::Notas);
        assert_eq!(Command::parse("/resumen", "").unwrap(), Command::Resumen);
        assert_eq!(Command::parse("/sync", "").unwrap(), Command::Sync);
        assert_eq!(Command::parse("/config", "").unwrap(), Command::Config);
        assert_eq!(Command::parse("/logout", "").unwrap(), Command::Logout);
    }

    #[test]
    fn test_bot_commands_metadata() {
        let mut cmds = Command::bot_commands();
        for cmd in &mut cmds {
            if cmd.command.starts_with('/') {
                cmd.command = cmd.command.trim_start_matches('/').to_string();
            }
        }
        let cmd_names: Vec<&str> = cmds.iter().map(|c| c.command.as_str()).collect();
        assert!(cmd_names.contains(&"start"));
        assert!(cmd_names.contains(&"ayuda"));
        assert!(cmd_names.contains(&"login"));
        assert!(cmd_names.contains(&"cursos"));
        assert!(cmd_names.contains(&"pendientes"));
        assert!(cmd_names.contains(&"notas"));
        assert!(cmd_names.contains(&"resumen"));
        assert!(cmd_names.contains(&"sync"));
        assert!(cmd_names.contains(&"config"));
        assert!(cmd_names.contains(&"logout"));
    }

    #[test]
    fn test_parse_admin_commands() {
        assert_eq!(
            AdminCommand::parse("/admin_stats", "").unwrap(),
            AdminCommand::AdminStats
        );
        assert_eq!(
            AdminCommand::parse("/toggle_registro", "").unwrap(),
            AdminCommand::ToggleRegistro
        );
        assert_eq!(
            AdminCommand::parse("/set_max_users 50", "").unwrap(),
            AdminCommand::SetMaxUsers(50)
        );
    }
}
