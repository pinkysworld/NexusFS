use serde::{Deserialize, Serialize};

/// Cover traffic policy (stub).
///
/// Cover traffic can reduce access-pattern leakage but costs bandwidth/energy.
/// It MUST be scheduler/telemetry-aware in production.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverTrafficPolicy {
    pub enabled: bool,
    pub avg_interval_ms: u64,
    pub max_bytes_per_min: u64,
}

impl Default for CoverTrafficPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            avg_interval_ms: 30_000,
            max_bytes_per_min: 1024 * 1024,
        }
    }
}
