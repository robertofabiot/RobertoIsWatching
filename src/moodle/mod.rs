pub mod client;
pub mod error;

pub use client::{MoodleClient, MoodleClientBuilder, DEFAULT_BASE_URL, DEFAULT_USER_AGENT};
pub use error::{MoodleError, Result};
