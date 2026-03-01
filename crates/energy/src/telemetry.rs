use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Telemetry {
    pub battery_pct: Option<u8>,
    pub charging: bool,
    pub temp_c: Option<i16>,
    pub cpu_load: f32,
    pub link_cost: f32,
    pub storage_free_bytes: u64,
}

/// Best-effort sampler.
///
/// v0: returns defaults unless implemented for the platform.
/// Later: implement Linux `/sys/class/power_supply` and thermal zones.
pub fn sample() -> Telemetry {
    // TODO: platform-specific sampling.
    Telemetry {
        battery_pct: None,
        charging: false,
        temp_c: None,
        cpu_load: 0.0,
        link_cost: 1.0,
        storage_free_bytes: 0,
    }
}
