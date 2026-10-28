use teloxide::adaptors::throttle::Limits;
use teloxide::adaptors::Throttle;
use teloxide::requests::RequesterExt;

pub mod commands;
pub mod copy;
pub mod dispatcher;
pub mod handlers;
pub mod rate_limit;
pub mod render;

pub use commands::register_commands;

/// Tipo de bot de Telegram envuelto en el adaptador Throttle para control estricto de rate limits (RNF-08).
pub type Bot = Throttle<teloxide::Bot>;

/// Crea una instancia del bot configurado con límites de despacho seguro (RNF-08).
pub fn create_bot(token: &str) -> Bot {
    teloxide::Bot::new(token).throttle(Limits::default())
}

/// Crea una instancia del bot apuntando a una URL personalizada (ideal para mock servers y tests).
pub fn create_bot_with_url(token: &str, api_url: reqwest::Url) -> Bot {
    teloxide::Bot::new(token)
        .set_api_url(api_url)
        .throttle(Limits::default())
}
