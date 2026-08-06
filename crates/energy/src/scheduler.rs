//! Deciding how much replication a device can currently afford.
//!
//! # The idea the rest of this rests on
//!
//! Operations and content have wildly different costs. An operation is a few hundred
//! bytes describing an intent; the content it refers to can be megabytes. So the
//! interesting throttle is not "sync or don't" but **keep the namespace converged and
//! defer the bytes**.
//!
//! A device that has taken every operation but no content still knows what exists,
//! where, and at what version. It can show a complete listing, answer "has this changed
//! since I last looked", and fetch any particular file on demand the moment someone
//! actually wants it. That is a far better degraded state than falling behind entirely,
//! and it costs almost nothing to maintain.
//!
//! So the ladder is: full sync → metadata plus capped content → metadata only → nothing.
//! Only the last rung stops tracking the filesystem, and it is reserved for a battery
//! low enough that the device is about to die anyway.
//!
//! # Unknown is not bad
//!
//! Every input can be `Unknown`, and unknown always means unconstrained. A server with
//! no battery sensor must not throttle itself forever because it cannot prove it is
//! plugged in.

use serde::{Deserialize, Serialize};

use crate::telemetry::{LinkCost, PowerSource, Telemetry};

/// How much work replication may do on this pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncBudget {
    /// Contact peers at all.
    pub sync: bool,
    /// Transfer chunk content, not just operations.
    pub content: bool,
    /// Cap on content bytes for one pass. `u64::MAX` means uncapped.
    pub max_content_bytes: u64,
    /// Multiplier on the configured poll interval; larger means less often.
    pub interval_scale: f32,
    /// Why this budget was chosen, for logs and the admin console.
    pub reason: String,
}

impl SyncBudget {
    pub fn unlimited() -> Self {
        Self {
            sync: true,
            content: true,
            max_content_bytes: u64::MAX,
            interval_scale: 1.0,
            reason: "no constraints apply".into(),
        }
    }

    fn metadata_only(reason: impl Into<String>, interval_scale: f32) -> Self {
        Self {
            sync: true,
            content: false,
            max_content_bytes: 0,
            interval_scale,
            reason: reason.into(),
        }
    }

    fn capped(reason: impl Into<String>, max_content_bytes: u64, interval_scale: f32) -> Self {
        Self {
            sync: true,
            content: true,
            max_content_bytes,
            interval_scale,
            reason: reason.into(),
        }
    }

    fn paused(reason: impl Into<String>) -> Self {
        Self {
            sync: false,
            content: false,
            max_content_bytes: 0,
            interval_scale: 4.0,
            reason: reason.into(),
        }
    }
}

/// What replication currently has outstanding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacklogView {
    pub pending_ops: u64,
    pub missing_chunks: u64,
}

pub trait Scheduler: Send + Sync {
    fn plan(&self, telemetry: &Telemetry, backlog: &BacklogView) -> SyncBudget;
}

/// Thresholds governing the rule-based scheduler.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// At or below this charge, stop transferring content.
    pub battery_low_pct: u8,
    /// At or below this charge, stop contacting peers entirely.
    pub battery_critical_pct: u8,
    /// At or above this temperature, stop transferring content.
    pub temp_high_c: i16,
    /// Content cap while conserving, in bytes.
    pub conserving_content_bytes: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            battery_low_pct: 20,
            battery_critical_pct: 5,
            temp_high_c: 70,
            conserving_content_bytes: 8 * 1024 * 1024,
        }
    }
}

impl Thresholds {
    /// Derive the critical threshold from the low one when it is not set explicitly.
    ///
    /// Clamped to at most `battery_low_pct`. Without that, a low threshold of 1 would
    /// derive a critical threshold of 2 and invert the ladder: the device would stop
    /// syncing entirely at a charge the operator asked to merely conserve at.
    pub fn from_config(battery_low_pct: u8, temp_high_c: i16) -> Self {
        Self {
            battery_low_pct,
            battery_critical_pct: (battery_low_pct / 4).max(2).min(battery_low_pct),
            temp_high_c,
            ..Self::default()
        }
    }
}

/// The default policy: graded by battery, overridden by heat and metered links.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuleBasedScheduler {
    pub thresholds: Thresholds,
    /// When false, every plan is unlimited. Lets the feature be switched off without
    /// removing it from the call path.
    pub enabled: bool,
}

impl RuleBasedScheduler {
    pub fn new(thresholds: Thresholds) -> Self {
        Self {
            thresholds,
            enabled: true,
        }
    }

    pub fn disabled() -> Self {
        Self {
            thresholds: Thresholds::default(),
            enabled: false,
        }
    }
}

impl Scheduler for RuleBasedScheduler {
    fn plan(&self, telemetry: &Telemetry, backlog: &BacklogView) -> SyncBudget {
        if !self.enabled {
            return SyncBudget::unlimited();
        }

        let t = &self.thresholds;

        // Heat first. Sustained transfer is what generates it, and no amount of battery
        // makes cooking the device acceptable — so this overrides the battery ladder
        // rather than being folded into it.
        if let Some(temp) = telemetry.temp_c {
            if temp >= t.temp_high_c {
                return SyncBudget::metadata_only(
                    format!(
                        "temperature {temp}°C is at or above the {}°C limit",
                        t.temp_high_c
                    ),
                    2.0,
                );
            }
        }

        // A metered link costs money per byte regardless of how much power is left.
        if telemetry.link == LinkCost::Metered {
            return SyncBudget::metadata_only("link is metered", 2.0);
        }

        // Mains and Unknown are both unconstrained; see the module docs on why Unknown
        // must not be treated as a constraint.
        if telemetry.power != PowerSource::Battery {
            return SyncBudget::unlimited();
        }

        let Some(pct) = telemetry.battery_pct else {
            // On battery but the charge is unreadable. Conserve mildly rather than
            // either extreme.
            return SyncBudget::capped(
                "on battery with an unreadable charge level",
                t.conserving_content_bytes,
                1.5,
            );
        };

        if pct <= t.battery_critical_pct {
            return SyncBudget::paused(format!(
                "battery {pct}% is at or below the critical {}%",
                t.battery_critical_pct
            ));
        }

        if pct <= t.battery_low_pct {
            // Metadata still flows: it is nearly free and keeps the namespace current
            // so the device stays useful once power returns.
            return SyncBudget::metadata_only(
                format!(
                    "battery {pct}% is at or below the low threshold {}%",
                    t.battery_low_pct
                ),
                2.0,
            );
        }

        // A comfort band above the low threshold where content still moves, capped, so
        // a large backlog cannot drain the remaining charge in one pass.
        let comfortable = t.battery_low_pct.saturating_mul(2).min(100);
        if pct <= comfortable && backlog.missing_chunks > 0 {
            return SyncBudget::capped(
                format!("battery {pct}% is within the conserving band up to {comfortable}%"),
                t.conserving_content_bytes,
                1.0,
            );
        }

        SyncBudget::unlimited()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched() -> RuleBasedScheduler {
        RuleBasedScheduler::new(Thresholds {
            battery_low_pct: 20,
            battery_critical_pct: 5,
            temp_high_c: 70,
            conserving_content_bytes: 1024,
        })
    }

    fn telemetry(power: PowerSource, pct: Option<u8>, temp: Option<i16>) -> Telemetry {
        Telemetry {
            power,
            battery_pct: pct,
            temp_c: temp,
            ..Default::default()
        }
    }

    fn backlog() -> BacklogView {
        BacklogView {
            pending_ops: 0,
            missing_chunks: 10,
        }
    }

    #[test]
    fn mains_is_unconstrained() {
        let b = sched().plan(&telemetry(PowerSource::Mains, Some(10), None), &backlog());
        assert!(b.sync && b.content);
        assert_eq!(b.max_content_bytes, u64::MAX);
    }

    #[test]
    fn unknown_power_is_unconstrained() {
        // A server with no battery sensor must not throttle itself forever.
        let b = sched().plan(&telemetry(PowerSource::Unknown, None, None), &backlog());
        assert!(b.sync && b.content);
        assert_eq!(b.max_content_bytes, u64::MAX);
    }

    #[test]
    fn a_healthy_battery_is_unconstrained() {
        let b = sched().plan(&telemetry(PowerSource::Battery, Some(90), None), &backlog());
        assert!(b.sync && b.content);
        assert_eq!(b.max_content_bytes, u64::MAX);
    }

    #[test]
    fn the_conserving_band_caps_content_without_stopping_it() {
        let b = sched().plan(&telemetry(PowerSource::Battery, Some(30), None), &backlog());
        assert!(b.sync && b.content);
        assert_eq!(b.max_content_bytes, 1024);
    }

    #[test]
    fn a_low_battery_keeps_metadata_and_drops_content() {
        // The central claim: falling behind on content is fine, falling behind on the
        // namespace is not.
        let b = sched().plan(&telemetry(PowerSource::Battery, Some(15), None), &backlog());
        assert!(b.sync, "operations should still flow");
        assert!(!b.content, "content should not");
        assert!(b.interval_scale > 1.0);
    }

    #[test]
    fn a_critical_battery_stops_everything() {
        let b = sched().plan(&telemetry(PowerSource::Battery, Some(3), None), &backlog());
        assert!(!b.sync);
        assert!(b.reason.contains("critical"));
    }

    #[test]
    fn heat_overrides_a_full_battery() {
        let b = sched().plan(
            &telemetry(PowerSource::Mains, Some(100), Some(85)),
            &backlog(),
        );
        assert!(b.sync, "metadata is cheap enough to keep flowing");
        assert!(!b.content, "sustained transfer is what generates heat");
        assert!(b.reason.contains("temperature"));
    }

    #[test]
    fn a_metered_link_drops_content_on_mains() {
        let mut t = telemetry(PowerSource::Mains, None, None);
        t.link = LinkCost::Metered;
        let b = sched().plan(&t, &backlog());
        assert!(b.sync && !b.content);
        assert!(b.reason.contains("metered"));
    }

    #[test]
    fn an_unreadable_charge_conserves_rather_than_guessing() {
        let b = sched().plan(&telemetry(PowerSource::Battery, None, None), &backlog());
        assert!(b.sync && b.content);
        assert_eq!(b.max_content_bytes, 1024);
    }

    #[test]
    fn disabling_the_scheduler_removes_every_limit() {
        let b = RuleBasedScheduler::disabled().plan(
            &telemetry(PowerSource::Battery, Some(1), Some(99)),
            &backlog(),
        );
        assert!(b.sync && b.content);
        assert_eq!(b.max_content_bytes, u64::MAX);
    }
}
