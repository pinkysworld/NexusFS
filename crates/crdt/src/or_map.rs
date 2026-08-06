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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(
    serialize = "K: Ord + Serialize, V: Serialize",
    deserialize = "K: Ord + Deserialize<'de>, V: Deserialize<'de>"
))]
pub struct OrMap<K, V> {
    pub adds: BTreeMap<K, BTreeMap<Dot, V>>,
    pub removes: BTreeMap<K, BTreeSet<Dot>>,
}

/// Hand-written rather than derived: `#[derive(Default)]` would demand
/// `K: Default, V: Default`, which an empty map plainly does not need.
impl<K, V> Default for OrMap<K, V> {
    fn default() -> Self {
        Self {
            adds: BTreeMap::new(),
            removes: BTreeMap::new(),
        }
    }
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

    /// Remove everything currently visible for `key`.
    ///
    /// Only meaningful against a local, fully-observed view — an operation that has to
    /// converge across replicas must name its dots explicitly with [`remove_dots`],
    /// because "currently visible" differs by arrival order.
    ///
    /// [`remove_dots`]: Self::remove_dots
    pub fn observed_dots(&self, key: &K) -> Vec<Dot> {
        self.adds
            .get(key)
            .map(|dots| dots.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn remove(&mut self, key: &K) {
        let observed = self.observed_dots(key);
        self.remove_dots(key, observed);
    }

    /// Record a removal of exactly `dots`.
    ///
    /// The dots need not be present yet. A removal that arrives before the addition it
    /// refers to still suppresses it, because `get_all` filters against this set rather
    /// than consulting it only at insertion time — which is what makes the result
    /// independent of arrival order.
    pub fn remove_dots(&mut self, key: &K, dots: impl IntoIterator<Item = Dot>) {
        let mut iter = dots.into_iter().peekable();
        if iter.peek().is_none() {
            return;
        }
        self.removes.entry(key.clone()).or_default().extend(iter);
    }

    pub fn get(&self, key: &K) -> Option<V> {
        // Deterministic single-value read: highest surviving dot wins.
        self.get_all(key).pop().map(|(_, v)| v)
    }

    /// Every value for `key` that has not been observed-removed, ordered by dot.
    ///
    /// `get` collapses this to one value, which hides concurrent adds. Callers that
    /// must *detect* a conflict — rather than silently pick a winner — need to see
    /// all survivors so they can derive a deterministic conflict name.
    pub fn get_all(&self, key: &K) -> Vec<(Dot, V)> {
        let Some(adds) = self.adds.get(key) else {
            return Vec::new();
        };
        let removed = self.removes.get(key);
        let mut survivors: Vec<(Dot, V)> = adds
            .iter()
            .filter(|(d, _)| removed.map(|r| !r.contains(d)).unwrap_or(true))
            .map(|(d, v)| (*d, v.clone()))
            .collect();
        survivors.sort_by_key(|(dot, _)| *dot);
        survivors
    }

    /// Keys that still have at least one surviving value.
    ///
    /// `keys` returns every key ever added, including fully-removed ones (tombstones
    /// are retained so that concurrent re-adds still converge).
    pub fn live_keys(&self) -> Vec<K> {
        self.adds
            .keys()
            .filter(|k| !self.get_all(k).is_empty())
            .cloned()
            .collect()
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
