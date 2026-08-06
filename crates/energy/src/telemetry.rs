//! Device telemetry.
//!
//! Every field is optional or explicitly "unknown", and that is load-bearing: a server
//! with no battery is not a device at 0% charge. Conflating the two would make a
//! machine with no power constraint throttle itself permanently, which is the obvious
//! failure mode of a naive implementation.
//!
//! Sampling shells out on macOS and reads sysfs on Linux. Both are best-effort — an
//! unreadable source yields `Unknown` rather than a guess.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Where the device is drawing power from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PowerSource {
    /// Mains, or a machine with no battery at all. Power is not a constraint.
    Mains,
    /// Running down a battery.
    Battery,
    /// Could not determine. Treated as unconstrained: refusing to work because a
    /// sensor is missing would be worse than the risk of draining a battery.
    #[default]
    Unknown,
}

/// What moving bytes over this link costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LinkCost {
    /// Ordinary broadband or LAN.
    Unmetered,
    /// Tethered, cellular, or otherwise paid for by the byte.
    Metered,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Telemetry {
    pub power: PowerSource,
    /// Charge percentage, when a battery is present and readable.
    pub battery_pct: Option<u8>,
    /// Hottest thermal zone, in degrees Celsius.
    pub temp_c: Option<i16>,
    /// One-minute load average, where available.
    pub cpu_load: Option<f32>,
    pub link: LinkCost,
    pub storage_free_bytes: Option<u64>,
    pub sampled_unix_ms: u64,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            power: PowerSource::Unknown,
            battery_pct: None,
            temp_c: None,
            cpu_load: None,
            link: LinkCost::Unknown,
            storage_free_bytes: None,
            sampled_unix_ms: 0,
        }
    }
}

impl Telemetry {
    /// True only when we know the device is running down a battery.
    pub fn on_battery(&self) -> bool {
        self.power == PowerSource::Battery
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Take a best-effort reading of the current device state.
pub fn sample() -> Telemetry {
    let mut t = platform_sample();
    t.sampled_unix_ms = now_ms();
    t
}

#[cfg(target_os = "macos")]
fn platform_sample() -> Telemetry {
    let mut t = Telemetry::default();

    // `pmset -g batt` names the source, and lists a percentage only when a battery
    // exists. A desktop prints the source alone — which is why absence of a percentage
    // must not be read as "flat".
    if let Ok(out) = std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);

        t.power = if text.contains("'AC Power'") {
            PowerSource::Mains
        } else if text.contains("'Battery Power'") {
            PowerSource::Battery
        } else {
            PowerSource::Unknown
        };

        t.battery_pct = text
            .split_whitespace()
            .find_map(|w| w.strip_suffix("%;").or_else(|| w.strip_suffix('%')))
            .and_then(|n| n.parse::<u8>().ok())
            .map(|pct| pct.min(100));
    }

    t.cpu_load = load_average();
    t
}

#[cfg(target_os = "linux")]
fn platform_sample() -> Telemetry {
    use std::fs;

    let mut t = Telemetry::default();

    if let Ok(entries) = fs::read_dir("/sys/class/power_supply") {
        let mut saw_battery = false;
        let mut mains_online = false;

        for entry in entries.flatten() {
            let path = entry.path();
            let kind = fs::read_to_string(path.join("type")).unwrap_or_default();

            match kind.trim() {
                "Battery" => {
                    saw_battery = true;
                    if let Some(pct) = fs::read_to_string(path.join("capacity"))
                        .ok()
                        .and_then(|c| c.trim().parse::<u8>().ok())
                    {
                        t.battery_pct = Some(pct.min(100));
                    }
                    if fs::read_to_string(path.join("status"))
                        .map(|s| s.trim() == "Discharging")
                        .unwrap_or(false)
                    {
                        t.power = PowerSource::Battery;
                    }
                }
                "Mains" | "USB"
                    if fs::read_to_string(path.join("online"))
                        .map(|s| s.trim() == "1")
                        .unwrap_or(false) =>
                {
                    mains_online = true;
                }
                _ => {}
            }
        }

        if mains_online {
            t.power = PowerSource::Mains;
        } else if !saw_battery {
            t.power = PowerSource::Unknown;
        }
    }

    // Hottest thermal zone. sysfs reports millidegrees.
    if let Ok(zones) = fs::read_dir("/sys/class/thermal") {
        t.temp_c = zones
            .flatten()
            .filter_map(|z| fs::read_to_string(z.path().join("temp")).ok())
            .filter_map(|v| v.trim().parse::<i32>().ok())
            // Clamp rather than cast: a bogus sysfs reading of i32::MAX millidegrees
            // would wrap to a small or negative i16 and could silently satisfy — or
            // silently defeat — the thermal rule.
            .map(|milli| (milli / 1000).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
            .max();
    }

    t.cpu_load = load_average();
    t
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_sample() -> Telemetry {
    // Unknown on every field, which the scheduler reads as unconstrained.
    Telemetry::default()
}

#[cfg(target_os = "linux")]
fn load_average() -> Option<f32> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f32>().ok())
}

#[cfg(target_os = "macos")]
fn load_average() -> Option<f32> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()?;
    // Prints "{ 1.83 2.01 2.10 }"; the first number is the one-minute average.
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<f32>().ok())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn load_average() -> Option<f32> {
    None
}
