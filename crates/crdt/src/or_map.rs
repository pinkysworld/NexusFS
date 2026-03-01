use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A minimal Observed-Remove Map (OR-Map) skeleton.
///
/// This is intentionally simplified for v0. The idea:
/// - Adds are tagged with a unique dot (device_id, counter).
/// - Removes record dots observed for a key.
/// - Merge is set union.
///
/// Later, optimize storage and add ZK-friendly roots.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(bound(
    serialize = "K: Ord + Serialize, V: Serialize",
    deserialize = "K: Ord + Deserialize<'de>, V: Deserialize<'de>"
))]
pub struct OrMap<K, V> {
    pub adds: BTreeMap<K, BTreeMap<Dot, V>>,
    pub removes: BTreeMap<K, BTreeSet<Dot>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Dot {
    pub device_id: u128,
    pub counter: u64,
}

impl<K: Ord + Clone, V: Clone> OrMap<K, V> {
    pub fn add(&mut self, key: K, dot: Dot, value: V) {
        self.adds.entry(key).or_default().insert(dot, value);
    }

    pub fn remove(&mut self, key: &K) {
        if let Some(dots) = self.adds.get(key) {
            let observed: BTreeSet<Dot> = dots.keys().cloned().collect();
            self.removes
                .entry(key.clone())
                .or_default()
                .extend(observed);
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        let adds = self.adds.get(key)?;
        let removed = self.removes.get(key);
        // Return any surviving value deterministically (by max dot).
        let mut candidates: Vec<(Dot, V)> = adds
            .iter()
            .filter(|(d, _)| removed.map(|r| !r.contains(d)).unwrap_or(true))
            .map(|(d, v)| (*d, v.clone()))
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        candidates.last().map(|(_, v)| v.clone())
    }

    pub fn keys(&self) -> Vec<K> {
        self.adds.keys().cloned().collect()
    }

    pub fn merge(&mut self, other: &Self) {
        // union adds
        for (k, vmap) in &other.adds {
            let entry = self.adds.entry(k.clone()).or_default();
            for (dot, v) in vmap {
                entry.entry(*dot).or_insert_with(|| v.clone());
            }
        }
        // union removes
        for (k, rset) in &other.removes {
            self.removes
                .entry(k.clone())
                .or_default()
                .extend(rset.iter().cloned());
        }
    }
}
