use std::io::IsTerminal;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init(rust_log: &str) {
    let filter = EnvFilter::try_new(rust_log)
        .unwrap_or_else(|_| EnvFilter::new("info,robertos_watching_bot=debug"));

    let is_tty = std::io::stdout().is_terminal();

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().compact().with_ansi(is_tty).with_target(true))
        .init();
}
