use serde::{Deserialize, Serialize};

use nexusfs_storage::Hash;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplicationTask {
    SendOps { peer: u128, count: u32 },
    SendBlobs { peer: u128, hashes: Vec<Hash> },
    RequestOps { peer: u128 },
    RequestBlobs { peer: u128, hashes: Vec<Hash> },
    CompactLocal,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BacklogView {
    pub pending_ops: u64,
    pub pending_blobs: u64,
}

pub trait Scheduler: Send + Sync {
    fn plan(&self, telemetry: &crate::Telemetry, backlog: &BacklogView) -> Vec<ReplicationTask>;
}

/// Baseline rule-based scheduler.
///
/// Intuition:
/// - If battery is low and not charging -> only sync ops.
/// - If temperature high -> avoid compaction/proofs.
/// - Otherwise do normal work.
#[derive(Debug, Clone, Default)]
pub struct RuleBasedScheduler {
    pub battery_low_pct: u8,
    pub temp_high_c: i16,
}

impl Scheduler for RuleBasedScheduler {
    fn plan(&self, telemetry: &crate::Telemetry, backlog: &BacklogView) -> Vec<ReplicationTask> {
        let mut tasks = Vec::new();

        let battery_low = telemetry
            .battery_pct
            .map(|b| b <= self.battery_low_pct)
            .unwrap_or(false);

        let temp_high = telemetry
            .temp_c
            .map(|t| t >= self.temp_high_c)
            .unwrap_or(false);

        // Always allow ops sync if there is backlog.
        if backlog.pending_ops > 0 {
            tasks.push(ReplicationTask::RequestOps { peer: 0 });
        }

        // Only allow blob transfer if energy/thermal budget is ok.
        if !battery_low || telemetry.charging {
            if backlog.pending_blobs > 0 {
                tasks.push(ReplicationTask::RequestBlobs {
                    peer: 0,
                    hashes: vec![],
                });
            }
        }

        if !temp_high {
            tasks.push(ReplicationTask::CompactLocal);
        }

        tasks
    }
}
