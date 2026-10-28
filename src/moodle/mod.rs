pub mod client;
pub mod envelope;
pub mod error;
pub mod gate;

pub use client::{MoodleClient, MoodleClientBuilder, DEFAULT_BASE_URL, DEFAULT_USER_AGENT};
pub use envelope::decode_envelope;
pub use error::{MoodleError, Result};
pub use gate::{GatePermit, MoodleGate, Priority};
