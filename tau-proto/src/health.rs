//! `health` method — daemon status check. Request has no params.

use serde::{Deserialize, Serialize};

pub const METHOD_HEALTH: &str = "health";

/// Result of the `health` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResult {
    pub version: String,
    pub uptime_ms: u64,
    pub pid: u32,
}
