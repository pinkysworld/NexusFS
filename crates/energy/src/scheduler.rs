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
    /// Free space to leave alone, in bytes.
    ///
    /// Not a threshold to throttle at but a floor to stay above: replication is a
    /// background job filling someone else's disk, and the last gigabyte belongs to
    /// whatever the machine is actually for.
    pub storage_reserve_bytes: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            battery_low_pct: 20,
            battery_critical_pct: 5,
            temp_high_c: 70,
            conserving_content_bytes: 8 * 1024 * 1024,
            storage_reserve_bytes: 1024 * 1024 * 1024,
        }
    }
}

impl Thresholds {
    /// Derive the critical threshold from the low one when it is not set explicitly.
    ///
    /// Clamped to at most `battery_low_pct`. Without that, a low threshold of 1 would
    /// derive a critical threshold of 2 and invert the ladder: the device would stop
    /// syncing entirely at a charge the operator asked to merely conserve at.
    pub fn from_config(battery_low_pct: u8, temp_high_c: i16, storage_reserve_mb: u64) -> Self {
        Self {
            battery_low_pct,
            battery_critical_pct: (battery_low_pct / 4).max(2).min(battery_low_pct),
            temp_high_c,
            // Saturating rather than wrapping: an operator who writes a reserve larger
            // than any disk gets "always metadata only", which is at least a coherent
            // reading of what they asked for.
            storage_reserve_bytes: storage_reserve_mb.saturating_mul(1024 * 1024),
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

impl RuleBasedScheduler {
    /// Hold content back to whatever room is left above the reserve.
    ///
    /// Applied after the grade rather than beside it, because disk is not the same kind
    /// of reading as the others. Battery, heat and link cost are tradeoffs the ladder
    /// weighs; free space is a wall. A budget that says "transfer 8MB" while 2MB
    /// remain is not a policy, it is a failed write — so this narrows whatever the
    /// ladder decided, and never widens it.
    fn limit_to_headroom(&self, budget: SyncBudget, telemetry: &Telemetry) -> SyncBudget {
        // Unknown never constrains, exactly as for every other reading: a host whose
        // `df` could not be read is not a host with no disk.
        let Some(free) = telemetry.storage_free_bytes else {
            return budget;
        };
        // Already not taking content. Nothing to narrow, and the reason already given
        // is the more specific one.
        if !budget.content {
            return budget;
        }

        let reserve = self.thresholds.storage_reserve_bytes;
        let Some(spare) = free.checked_sub(reserve).filter(|s| *s > 0) else {
            return SyncBudget::metadata_only(
                format!(
                    "{} free is at or below the {} reserve",
                    human_bytes(free),
                    human_bytes(reserve)
                ),
                // Not backed off: operations are tiny, a full disk will not empty
                // itself by being asked about less often, and the namespace staying
                // current is what makes the node still worth having.
                budget.interval_scale,
            );
        };

        if spare >= budget.max_content_bytes {
            return budget;
        }

        // A cap that binds is a constraint, and the reason says so — even on a healthy
        // node with hundreds of gigabytes spare. The first draft of this hid the reason
        // when the room was ample, on the grounds that such a cap binds no real pass;
        // that produced a budget carrying a 216GB ceiling while explaining itself as
        // "no constraints apply", which is the kind of quiet disagreement between a
        // number and its explanation that makes a console untrustworthy. Reporting an
        // uninteresting truth beats reporting a tidy contradiction.
        let reason = if budget.max_content_bytes == u64::MAX {
            format!(
                "content held to the {} free above the reserve",
                human_bytes(spare)
            )
        } else {
            format!(
                "{}, and further held to the {} free above the reserve",
                budget.reason,
                human_bytes(spare)
            )
        };
        SyncBudget::capped(reason, spare, budget.interval_scale)
    }
}

/// Bytes as an operator would write them, for reasons shown in logs and the console.
///
/// Public so the CLI's `status` formats a budget the same way the reason strings inside
/// it do; two spellings of the same number on one screen is its own small confusion.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1024 * 1024 * 1024 * 1024, "TB"),
        (1024 * 1024 * 1024, "GB"),
        (1024 * 1024, "MB"),
        (1024, "KB"),
    ];
    for (scale, unit) in UNITS {
        if n >= scale {
            // One decimal place: "1.5GB" is worth the character, "1.53GB" is not.
            return format!("{:.1}{unit}", n as f64 / scale as f64);
        }
    }
    format!("{n}B")
}

impl Scheduler for RuleBasedScheduler {
    fn plan(&self, telemetry: &Telemetry, backlog: &BacklogView) -> SyncBudget {
        if !self.enabled {
            return SyncBudget::unlimited();
        }
        let graded = self.grade(telemetry, backlog);
        self.limit_to_headroom(graded, telemetry)
    }
}

impl RuleBasedScheduler {
    /// The battery ladder, and the overrides that outrank it.
    fn grade(&self, telemetry: &Telemetry, backlog: &BacklogView) -> SyncBudget {
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
            storage_reserve_bytes: 0,
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

    /// A scheduler that reserves a gigabyte, for the headroom tests.
    fn sched_reserving_1gb() -> RuleBasedScheduler {
        RuleBasedScheduler::new(Thresholds {
            storage_reserve_bytes: 1024 * 1024 * 1024,
            ..Thresholds {
                battery_low_pct: 20,
                battery_critical_pct: 5,
                temp_high_c: 70,
                conserving_content_bytes: 1024,
                storage_reserve_bytes: 0,
            }
        })
    }

    fn with_free(mut t: Telemetry, free: Option<u64>) -> Telemetry {
        t.storage_free_bytes = free;
        t
    }

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn unreadable_free_space_constrains_nothing() {
        // The rule every reading in this crate follows: a host whose `df` could not be
        // read is not a host with no disk. Getting this backwards would make every
        // platform without a probe refuse content forever.
        let b = sched_reserving_1gb().plan(
            &with_free(telemetry(PowerSource::Mains, None, None), None),
            &backlog(),
        );
        assert!(b.content);
        assert_eq!(b.max_content_bytes, u64::MAX);
    }

    #[test]
    fn a_disk_at_the_reserve_keeps_metadata_and_drops_content() {
        // On mains at full charge, so the battery ladder cannot be what fired.
        for free in [0, GB / 2, GB] {
            let b = sched_reserving_1gb().plan(
                &with_free(telemetry(PowerSource::Mains, Some(100), None), Some(free)),
                &backlog(),
            );
            assert!(b.sync, "operations are tiny and keep the namespace current");
            assert!(!b.content, "{free} free is at or below the 1GB reserve");
            assert!(
                b.reason.contains("reserve"),
                "the reason should name what fired, got {:?}",
                b.reason
            );
        }
    }

    #[test]
    fn a_full_disk_does_not_back_off_the_poll_interval() {
        // Unlike heat and battery, waiting longer does not help: the disk will not
        // empty itself, and the operations that still flow are what keep the node
        // worth having.
        let b = sched_reserving_1gb().plan(
            &with_free(telemetry(PowerSource::Mains, Some(100), None), Some(0)),
            &backlog(),
        );
        assert_eq!(b.interval_scale, 1.0);
    }

    #[test]
    fn content_is_held_to_the_room_above_the_reserve() {
        let b = sched_reserving_1gb().plan(
            &with_free(
                telemetry(PowerSource::Mains, Some(100), None),
                Some(GB + 5 * 1024 * 1024),
            ),
            &backlog(),
        );
        assert!(b.content);
        assert_eq!(b.max_content_bytes, 5 * 1024 * 1024);
    }

    #[test]
    fn a_cap_that_binds_is_explained_even_when_it_is_generous() {
        // The number and the reason must agree. A budget carrying a 499GB ceiling while
        // saying "no constraints apply" is a console disagreeing with itself.
        let b = sched_reserving_1gb().plan(
            &with_free(
                telemetry(PowerSource::Mains, Some(100), None),
                Some(500 * GB),
            ),
            &backlog(),
        );
        assert_eq!(b.max_content_bytes, 499 * GB);
        assert!(b.content);
        assert!(
            b.reason.contains("499.0GB") && b.reason.contains("reserve"),
            "got {:?}",
            b.reason
        );
    }

    #[test]
    fn a_tight_disk_says_so() {
        // Once the room left is small enough to bind a real pass, it becomes the
        // explanation as well as the number.
        let b = sched_reserving_1gb().plan(
            &with_free(
                telemetry(PowerSource::Mains, Some(100), None),
                Some(GB + 512),
            ),
            &backlog(),
        );
        assert_eq!(b.max_content_bytes, 512);
        assert!(
            b.reason.contains("free above the reserve"),
            "got {:?}",
            b.reason
        );
    }

    #[test]
    fn headroom_narrows_a_battery_cap_but_never_widens_it() {
        // The conserving band caps at 1024 bytes. Plenty of disk must not undo that,
        // and a tighter disk must win.
        let roomy = sched_reserving_1gb().plan(
            &with_free(
                telemetry(PowerSource::Battery, Some(30), None),
                Some(500 * GB),
            ),
            &backlog(),
        );
        assert_eq!(
            roomy.max_content_bytes, 1024,
            "disk must not widen a power cap"
        );

        let tight = sched_reserving_1gb().plan(
            &with_free(
                telemetry(PowerSource::Battery, Some(30), None),
                Some(GB + 512),
            ),
            &backlog(),
        );
        assert_eq!(tight.max_content_bytes, 512, "the tighter limit wins");
        assert!(
            tight.reason.contains("battery") && tight.reason.contains("reserve"),
            "both limits should be visible, got {:?}",
            tight.reason
        );
    }

    #[test]
    fn a_paused_budget_is_left_paused_by_the_headroom_check() {
        // Critical battery stops everything. Free disk is not a reason to resume.
        let b = sched_reserving_1gb().plan(
            &with_free(
                telemetry(PowerSource::Battery, Some(2), None),
                Some(500 * GB),
            ),
            &backlog(),
        );
        assert!(!b.sync && !b.content);
        assert!(b.reason.contains("critical"));
    }

    #[test]
    fn bytes_are_reported_in_units_an_operator_recognises() {
        assert_eq!(human_bytes(0), "0B");
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(1024), "1.0KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0MB");
        assert_eq!(human_bytes(GB + GB / 2), "1.5GB");
        assert_eq!(human_bytes(3 * 1024 * GB), "3.0TB");
    }

    #[test]
    fn an_absurd_reserve_does_not_wrap_around() {
        // u64 megabytes overflows bytes long before it overflows the field. Saturating
        // gives "always metadata only", which is at least a coherent reading of what
        // was asked for; wrapping would give a tiny reserve and silently ignore it.
        let t = Thresholds::from_config(20, 70, u64::MAX);
        assert_eq!(t.storage_reserve_bytes, u64::MAX);
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
