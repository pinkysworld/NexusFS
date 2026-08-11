//! A Merkle commitment over the inode map, with inclusion proofs.
//!
//! # What this buys over a flat hash
//!
//! The previous commitment was one BLAKE3 over the whole sorted `(inode, hash)` list.
//! That is enough to say "two replicas agree", and nothing more: to convince anyone of a
//! single fact about the state, you had to hand them the entire state.
//!
//! A Merkle tree makes one fact provable on its own. Given a root, an inode, its object
//! hash, and O(log n) sibling hashes, anyone can check that the entry really is in the
//! state that root commits to — without holding the filesystem, and without learning
//! anything about the other entries beyond their hashes.
//!
//! # What it is not
//!
//! This is a commitment scheme, not a zero-knowledge proof. A verifier learns the inode
//! and the object hash being proved; it just does not learn the rest of the tree. Naming
//! the mode `ZkCommit` reflects that it is the *commitment* half of what a SNARK would
//! need — the sibling path is exactly the witness a circuit would consume — not that any
//! proving system is involved.
//!
//! # Hashing rules
//!
//! Leaves and interior nodes are domain-separated by a distinct prefix byte. Without
//! that, an attacker who can choose entry values could present an interior node's
//! preimage as a leaf and prove membership of something that was never in the tree.
//!
//! An odd node at any level is promoted unchanged to the next level rather than hashed
//! with a copy of itself. Duplicating the last node is the classic Merkle malleability
//! bug: two different leaf sets produce the same root.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub type Hash = [u8; 32];

const LEAF_TAG: u8 = 0x00;
const NODE_TAG: u8 = 0x01;
const DOMAIN: &[u8] = b"nexusfs/inode-merkle/v1";

/// Root of an empty map.
///
/// A distinct constant rather than zero, so "no state" cannot be confused with an
/// uninitialised or truncated hash.
pub fn empty_root() -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(b"empty");
    *hasher.finalize().as_bytes()
}

fn leaf_hash(inode: u128, value: &Hash) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[LEAF_TAG]);
    hasher.update(DOMAIN);
    hasher.update(&inode.to_be_bytes());
    hasher.update(value);
    *hasher.finalize().as_bytes()
}

fn node_hash(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[NODE_TAG]);
    hasher.update(DOMAIN);
    hasher.update(left);
    hasher.update(right);
    *hasher.finalize().as_bytes()
}

/// One step up the tree: a sibling and which side it sits on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathStep {
    pub sibling: Hash,
    /// True when the sibling is the *left* child and the running hash is the right one.
    pub sibling_is_left: bool,
}

/// Everything needed to check that one entry is in a committed map.
///
/// Self-contained on purpose: a verifier needs no filesystem, no network and no prior
/// state, only this and the root it wants to check against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InclusionProof {
    pub inode: u128,
    pub value: Hash,
    pub steps: Vec<PathStep>,
}

impl InclusionProof {
    /// Recompute the root this proof implies.
    pub fn compute_root(&self) -> Hash {
        let mut running = leaf_hash(self.inode, &self.value);
        for step in &self.steps {
            running = if step.sibling_is_left {
                node_hash(&step.sibling, &running)
            } else {
                node_hash(&running, &step.sibling)
            };
        }
        running
    }

    /// Whether this proof holds against `root`.
    pub fn verify(&self, root: &Hash) -> bool {
        // Constant-time comparison is unnecessary: both sides are public commitments,
        // and an attacker who could learn the root by timing already has it.
        self.compute_root() == *root
    }
}

/// The commitment over `entries`, which must be sorted by inode and free of duplicates.
///
/// Sorted-and-unique is the caller's job because the map it comes from is a `BTreeMap`,
/// which guarantees both — re-sorting here would hide a caller that had neither.
pub fn commit(entries: &[(u128, Hash)]) -> Hash {
    if entries.is_empty() {
        return empty_root();
    }
    let mut level: Vec<Hash> = entries
        .iter()
        .map(|(inode, value)| leaf_hash(*inode, value))
        .collect();

    while level.len() > 1 {
        level = fold(&level);
    }
    level[0]
}

/// One level of the tree, promoting a lone trailing node rather than doubling it.
fn fold(level: &[Hash]) -> Vec<Hash> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    let mut pairs = level.chunks_exact(2);
    for pair in pairs.by_ref() {
        next.push(node_hash(&pair[0], &pair[1]));
    }
    if let [odd] = pairs.remainder() {
        next.push(*odd);
    }
    next
}

/// Build an inclusion proof for `inode`.
///
/// Returns `None` when the inode is not in the map. Proving *absence* needs a different
/// construction — the standard trick is to prove the two entries that bracket the gap,
/// which this sorted layout supports but which nothing needs yet.
pub fn prove(entries: &[(u128, Hash)], inode: u128) -> Option<InclusionProof> {
    let index = entries.iter().position(|(i, _)| *i == inode)?;
    let value = entries[index].1;

    let mut steps = Vec::new();
    let mut level: Vec<Hash> = entries
        .iter()
        .map(|(inode, value)| leaf_hash(*inode, value))
        .collect();
    let mut position = index;

    while level.len() > 1 {
        // A promoted odd node has no sibling at this level, so it contributes no step.
        let sibling = if position % 2 == 0 {
            level.get(position + 1).map(|h| PathStep {
                sibling: *h,
                sibling_is_left: false,
            })
        } else {
            Some(PathStep {
                sibling: level[position - 1],
                sibling_is_left: true,
            })
        };
        if let Some(step) = sibling {
            steps.push(step);
        }

        position /= 2;
        level = fold(&level);
    }

    Some(InclusionProof {
        inode,
        value,
        steps,
    })
}

/// Check a proof and report *why* it failed, for tooling that has to explain itself.
pub fn check(proof: &InclusionProof, root: &Hash) -> Result<()> {
    // A path longer than this could only come from a map with more entries than the
    // tree-node ceiling allows, so it is malformed rather than merely wrong. Bounding it
    // stops a hostile proof from costing unbounded work to reject.
    const MAX_STEPS: usize = 64;
    if proof.steps.len() > MAX_STEPS {
        bail!(
            "proof has {} steps, more than the {MAX_STEPS} any real map produces",
            proof.steps.len()
        );
    }
    let computed = proof.compute_root();
    if computed != *root {
        bail!(
            "proof does not match the root: it commits to {} but was checked against {}",
            hex_short(&computed),
            hex_short(root)
        );
    }
    Ok(())
}

fn hex_short(h: &Hash) -> String {
    let full = hex_encode(h);
    format!("{}…", &full[..12])
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: u128) -> Vec<(u128, Hash)> {
        (1..=n).map(|i| (i, [i as u8; 32])).collect()
    }

    #[test]
    fn the_empty_map_has_its_own_root() {
        assert_eq!(commit(&[]), empty_root());
        assert_ne!(empty_root(), [0u8; 32]);
    }

    #[test]
    fn the_commitment_is_deterministic() {
        assert_eq!(commit(&entries(9)), commit(&entries(9)));
    }

    #[test]
    fn changing_any_value_changes_the_root() {
        let base = commit(&entries(8));
        for i in 0..8usize {
            let mut altered = entries(8);
            altered[i].1[0] ^= 0xff;
            assert_ne!(commit(&altered), base, "leaf {i} did not affect the root");
        }
    }

    #[test]
    fn every_entry_can_prove_itself_at_every_size() {
        // Odd sizes exercise the promotion path, which is where Merkle implementations
        // usually go wrong.
        for n in 1..=17u128 {
            let map = entries(n);
            let root = commit(&map);
            for (inode, value) in &map {
                let proof = prove(&map, *inode).expect("entry is present");
                assert_eq!(proof.value, *value);
                assert!(proof.verify(&root), "inode {inode} of {n} failed to verify");
            }
        }
    }

    #[test]
    fn a_proof_for_a_missing_entry_is_not_produced() {
        let map = entries(5);
        assert!(prove(&map, 99).is_none());
    }

    #[test]
    fn a_tampered_value_fails() {
        let map = entries(6);
        let root = commit(&map);
        let mut proof = prove(&map, 3).unwrap();
        proof.value[0] ^= 0xff;
        assert!(!proof.verify(&root));
        assert!(check(&proof, &root).is_err());
    }

    #[test]
    fn a_tampered_sibling_fails() {
        let map = entries(6);
        let root = commit(&map);
        let mut proof = prove(&map, 3).unwrap();
        proof.steps[0].sibling[0] ^= 0xff;
        assert!(!proof.verify(&root));
    }

    #[test]
    fn flipping_a_sibling_side_fails() {
        // The side matters: hashing the same pair in the other order must not verify,
        // or an attacker could reorder a path to reach a different leaf.
        let map = entries(6);
        let root = commit(&map);
        let mut proof = prove(&map, 3).unwrap();
        proof.steps[0].sibling_is_left = !proof.steps[0].sibling_is_left;
        assert!(!proof.verify(&root));
    }

    #[test]
    fn a_proof_does_not_transfer_to_another_inode() {
        let map = entries(6);
        let root = commit(&map);
        let mut proof = prove(&map, 3).unwrap();
        proof.inode = 4;
        assert!(!proof.verify(&root));
    }

    #[test]
    fn an_interior_node_cannot_be_passed_off_as_a_leaf() {
        // The reason leaves and nodes carry different tags. Without them, an attacker
        // who controls an entry value could supply an interior node's preimage and prove
        // membership of a leaf that was never inserted.
        let a = leaf_hash(1, &[1u8; 32]);
        let b = leaf_hash(2, &[2u8; 32]);
        let interior = node_hash(&a, &b);

        // Any leaf hash colliding with the interior node would be the attack; the tag
        // byte makes the two preimage spaces disjoint.
        assert_ne!(interior, leaf_hash(1, &[1u8; 32]));
        assert_ne!(interior, leaf_hash(0, &interior));
    }

    #[test]
    fn a_trailing_node_is_promoted_not_doubled() {
        // Doubling the odd node would make these two maps share a root, which is the
        // classic Merkle malleability bug.
        let three = vec![(1u128, [1u8; 32]), (2, [2u8; 32]), (3, [3u8; 32])];
        let four_with_dup = vec![
            (1u128, [1u8; 32]),
            (2, [2u8; 32]),
            (3, [3u8; 32]),
            (3, [3u8; 32]),
        ];
        assert_ne!(commit(&three), commit(&four_with_dup));
    }

    #[test]
    fn an_absurdly_long_path_is_rejected_without_being_walked() {
        let map = entries(4);
        let root = commit(&map);
        let mut proof = prove(&map, 1).unwrap();
        proof.steps = vec![
            PathStep {
                sibling: [0u8; 32],
                sibling_is_left: false,
            };
            1000
        ];
        let err = check(&proof, &root).unwrap_err().to_string();
        assert!(err.contains("more than"), "{err}");
    }
}
