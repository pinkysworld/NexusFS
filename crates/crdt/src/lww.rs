use serde::{Deserialize, Serialize};

/// Last-Writer-Wins register.
///
/// The ordering key is `(ts, writer_id, seq)`:
/// - `ts` is the wall-clock intent of the writer,
/// - `writer_id` breaks ties between different devices deterministically,
/// - `seq` breaks ties *within* one device, so a device writing twice in the same
///   millisecond still has its later write win.
///
/// Without `seq`, two writes from one device sharing a timestamp would resolve by
/// arrival order, and replicas that received them in different orders would diverge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LwwReg<T> {
    pub value: T,
    pub ts: u64,
    pub writer_id: u128,
    pub seq: u64,
}

impl<T: Clone> LwwReg<T> {
    pub fn new(value: T, ts: u64, writer_id: u128, seq: u64) -> Self {
        Self {
            value,
            ts,
            writer_id,
            seq,
        }
    }

    fn order_key(&self) -> (u64, u128, u64) {
        (self.ts, self.writer_id, self.seq)
    }

    /// Returns true when `other` won and `self` was updated.
    pub fn merge(&mut self, other: &Self) -> bool {
        if other.order_key() > self.order_key() {
            self.value = other.value.clone();
            self.ts = other.ts;
            self.writer_id = other.writer_id;
            self.seq = other.seq;
            true
        } else {
            false
        }
    }
}
