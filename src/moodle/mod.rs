pub mod auth;
pub mod breaker;
pub mod client;
pub mod courses;
pub mod envelope;
pub mod error;
pub mod gate;
pub mod submissions;

pub use auth::{LoginInfo, SiteInfo};
pub use breaker::{CircuitBreaker, CircuitState};
pub use client::{MoodleClient, MoodleClientBuilder, DEFAULT_BASE_URL, DEFAULT_USER_AGENT};
pub use courses::{FailoverOutcome, ModuleInfo, MoodleAssignment, MoodleCourse};
pub use envelope::decode_envelope;
pub use error::{MoodleError, Result};
pub use gate::{GatePermit, MoodleGate, Priority};
pub use submissions::{strip_html_to_plain_text, SubmissionStatus};
