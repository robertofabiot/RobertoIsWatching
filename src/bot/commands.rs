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
    #[command(description = "Cierre de sesión y purga de datos")]
    Logout,
}

/// Comandos exclusivos para el administrador (`ADMIN_CHAT_ID`) (RF-10).
#[derive(BotCommands, Clone, Debug, PartialEq, Eq)]
#[command(rename_rule = "lowercase")]
pub enum AdminCommand {
    AdminStats,
    ToggleRegistro,
    SetMaxUsers(u32),
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
        assert_eq!(Command::parse("/logout", "").unwrap(), Command::Logout);
    }
}
