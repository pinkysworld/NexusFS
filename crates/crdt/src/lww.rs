use serde::{Deserialize, Serialize};

/// Last-Writer-Wins register.
/// Tie-breaker uses `writer_id` to ensure deterministic convergence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LwwReg<T> {
    pub value: T,
    pub ts: u64,
    pub writer_id: u128,
}

impl<T: Clone> LwwReg<T> {
    pub fn new(value: T, ts: u64, writer_id: u128) -> Self {
        Self {
            value,
            ts,
            writer_id,
        }
    }

    pub fn merge(&mut self, other: &Self) {
        if (other.ts, other.writer_id) > (self.ts, self.writer_id) {
            self.value = other.value.clone();
            self.ts = other.ts;
            self.writer_id = other.writer_id;
        }
    }
}
