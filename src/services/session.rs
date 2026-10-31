use crate::bot::{copy, Bot};
use crate::storage::UsersRepository;
use sqlx::SqlitePool;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Marca al usuario como inactivo por token expirado y le notifica en privado con el copy oficial (A-15, RF-01).
/// Reutilizable de forma compartida por el bot y por el worker de escaneo continuo.
pub async fn expire_user(pool: &SqlitePool, bot: &Bot, chat_id: i64) -> Result<bool, sqlx::Error> {
    tracing::warn!(
        chat_id = chat_id,
        "Expirando sesión de usuario por wstoken revocado"
    );

    // 1. Marcar como inactivo en SQLite y limpiar wstoken por seguridad (§0.3, A-15)
    let deactivated = UsersRepository::mark_inactive(pool, chat_id, "token_expired").await?;

    // 2. Enviar aviso privado de re-autenticación
    let _ = bot
        .send_message(ChatId(chat_id), copy::TOKEN_EXPIRED_ALERT)
        .parse_mode(ParseMode::Html)
        .await;

    Ok(deactivated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::create_bot;
    use crate::config::Config;
    use crate::storage::{init, UsersRepository};

    #[tokio::test]
    async fn test_expire_user_marks_inactive_and_clears_token() {
        let rand_id: u64 = rand::random();
        let db_path = std::env::temp_dir().join(format!("test_expire_{rand_id}.db"));
        let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let config = Config {
            bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
            admin_chat_id: 123456789,
            database_url,
            max_users_default: 300,
            bot_tz: chrono_tz::America::Managua,
            rust_log: "info".to_string(),
            scan_interval_min: 30,
        };

        let pool = init(&config).await.expect("Fallo al crear BD");

        // Crear usuario activo
        let user = UsersRepository::upsert_login(
            &pool,
            8888,
            "24018888",
            12345,
            "Alumno Expira",
            "token_valido_inicial",
        )
        .await
        .unwrap();
        assert!(user.is_active);
        assert_eq!(user.wstoken, "token_valido_inicial");

        let bot = create_bot(&config.bot_token);

        let ok = expire_user(&pool, &bot, 8888).await.unwrap();
        assert!(ok);

        let expired = UsersRepository::get(&pool, 8888).await.unwrap().unwrap();
        assert!(!expired.is_active);
        assert_eq!(expired.inactive_reason.as_deref(), Some("token_expired"));
        assert_eq!(expired.wstoken, "");

        let _ = std::fs::remove_file(&db_path);
    }
}
