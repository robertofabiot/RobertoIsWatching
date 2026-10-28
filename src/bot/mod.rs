use teloxide::adaptors::throttle::Limits;
use teloxide::adaptors::Throttle;
use teloxide::requests::RequesterExt;

pub mod commands;
pub mod copy;
pub mod dispatcher;
pub mod handlers;

/// Tipo de bot de Telegram envuelto en el adaptador Throttle para control estricto de rate limits (RNF-08).
pub type Bot = Throttle<teloxide::Bot>;

/// Crea una instancia del bot configurado con límites de despacho seguro (RNF-08).
pub fn create_bot(token: &str) -> Bot {
    teloxide::Bot::new(token).throttle(Limits::default())
}
