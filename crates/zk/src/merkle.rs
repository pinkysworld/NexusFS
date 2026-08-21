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

    let mut i = 0;
    while i + 1 < level.len() {
        next.push(node_hash(&level[i], &level[i + 1]));
        i += 2;
    }
    // A lone trailing node is carried up unchanged. Hashing it with a copy of itself
    // would let two different leaf sets share a root, which is the classic Merkle
    // malleability bug.
    if i < level.len() {
        next.push(level[i]);
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

    /// The root for a fixed map, pinned as a literal.
    ///
    /// Every other test here checks the tree against *itself* — that proofs verify, that
    /// changes propagate, that malleability is refused. All of those keep passing if the
    /// hashing changes, as long as it changes consistently. But this value is on disk and
    /// on the wire: it is what two replicas compare and what a proof is checked against,
    /// so a refactor that quietly moves it would break every existing repository while
    /// the suite stayed green.
    ///
    /// If this test fails, the on-disk format changed. That needs a version bump, not a
    /// new constant.
    #[test]
    fn the_commitment_is_pinned_to_known_values() {
        let cases: [(u128, &str); 6] = [
            (
                0,
                "6013fb03c7645b02a3ed2e0dab71f02d34fa85a3e9478c1c7d9b96626b6935e6",
            ),
            (
                1,
                "0f8135f428ecc2c196de66d137b1affff48790dcedae996a3e802b06ef5ba0e5",
            ),
            (
                2,
                "8734121094e931d3540bc4305fdb8ae7499849df7997bc64e1f734b0c5ab00ec",
            ),
            // Odd sizes exercise the promotion path, which is where the layout is most
            // easily changed by accident.
            (
                3,
                "6836d21102b9f66de0e737e784965964a6b985d7cb0e71881dec216045b820dc",
            ),
            (
                5,
                "7cc7a2faffbbe7963c497655c616160150b28de669f7ba5c34be3bad0aef69aa",
            ),
            (
                8,
                "ebf87a7f76ce8d69dcd45d8f6f3029265858853a820636e5c67de10e0e6bc0e1",
            ),
        ];

        for (n, expected) in cases {
            let map: Vec<(u128, Hash)> = (1..=n).map(|i| (i, [i as u8; 32])).collect();
            assert_eq!(
                hex_encode(&commit(&map)),
                expected,
                "the commitment for {n} entries moved; this is an on-disk format change"
            );
        }
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
    fn absence_is_provable_for_every_gap() {
        // Odd sizes again, and gaps at both ends as well as the middle.
        for n in 1..=17u128 {
            let map: Vec<(u128, Hash)> = (1..=n).map(|i| (i * 10, [i as u8; 32])).collect();
            let root = commit(&map);

            for probe in [5u128, 15, 25, n * 10 + 5] {
                if map.iter().any(|(i, _)| *i == probe) {
                    continue;
                }
                let proof = prove_absent(&map, probe).expect("probe is genuinely absent");
                check_absent(&proof, &root)
                    .unwrap_or_else(|e| panic!("absence of {probe} in {n} entries: {e}"));
            }
        }
    }

    /// A lied-about `len` must not buy the prover anything.
    ///
    /// `len` is the one field of an absence proof the root does not cover, so it is
    /// the natural thing to lie about. Three lies would each be fatal, and each is
    /// swept here against a *real* root with *real* neighbour paths — the only freedom
    /// the attacker has is the claimed size and the claimed positions:
    ///
    /// - pass a middle leaf off as the last one, to prove absence past it;
    /// - pass a middle leaf off as the first one, to prove absence before it;
    /// - pass two non-adjacent leaves off as adjacent, to swallow what lies between.
    ///
    /// Every one of them names an inode that is genuinely present, so a single
    /// acceptance is a forged proof of a deletion that never happened.
    #[test]
    fn a_forged_length_cannot_manufacture_a_gap() {
        for n in 2..=9usize {
            let map: Vec<(u128, Hash)> = (1..=n as u128).map(|i| (i * 10, [i as u8; 32])).collect();
            let root = commit(&map);
            let real: Vec<InclusionProof> = map
                .iter()
                .map(|(i, _)| prove(&map, *i).expect("entry is present"))
                .collect();

            for len in 1..=(2 * n + 4) {
                for j in 0..n {
                    // "past the end": target sits above leaf j, but j is not the last.
                    if j + 1 < n {
                        let forged = AbsenceProof {
                            inode: map[j + 1].0,
                            len,
                            left: Some(Neighbour {
                                index: len.saturating_sub(1),
                                proof: real[j].clone(),
                            }),
                            right: None,
                        };
                        assert!(
                            check_absent(&forged, &root).is_err(),
                            "n={n} len={len} j={j}: a middle leaf was accepted as the last"
                        );
                    }

                    // "before the start": target sits below leaf j, but j is not first.
                    if j > 0 {
                        let forged = AbsenceProof {
                            inode: map[j - 1].0,
                            len,
                            left: None,
                            right: Some(Neighbour {
                                index: 0,
                                proof: real[j].clone(),
                            }),
                        };
                        assert!(
                            check_absent(&forged, &root).is_err(),
                            "n={n} len={len} j={j}: a middle leaf was accepted as the first"
                        );
                    }

                    // "adjacent": two leaves with at least one entry between them.
                    for k in (j + 2)..n {
                        for i in 0..len.saturating_sub(1) {
                            let forged = AbsenceProof {
                                inode: map[j + 1].0,
                                len,
                                left: Some(Neighbour {
                                    index: i,
                                    proof: real[j].clone(),
                                }),
                                right: Some(Neighbour {
                                    index: i + 1,
                                    proof: real[k].clone(),
                                }),
                            };
                            assert!(
                                check_absent(&forged, &root).is_err(),
                                "n={n} len={len} j={j} k={k} i={i}: a gap was manufactured"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn absence_is_not_offered_for_something_present() {
        let map = entries(5);
        assert!(prove_absent(&map, 3).is_none());
    }

    #[test]
    fn absence_in_an_empty_map_is_provable() {
        let proof = prove_absent(&[], 42).unwrap();
        check_absent(&proof, &empty_root()).unwrap();
        // ...and only against the empty root.
        assert!(check_absent(&proof, &[7u8; 32]).is_err());
    }

    #[test]
    fn non_adjacent_neighbours_do_not_prove_a_gap() {
        // The attack this guards: hand over two entries that bracket the target but
        // have others between them, and claim everything in between is absent.
        let map: Vec<(u128, Hash)> = (1..=8u128).map(|i| (i * 10, [i as u8; 32])).collect();
        let root = commit(&map);

        let forged = AbsenceProof {
            inode: 40,
            len: map.len(),
            left: Some(Neighbour {
                index: 1,
                proof: prove(&map, 20).unwrap(),
            }),
            right: Some(Neighbour {
                index: 5,
                proof: prove(&map, 60).unwrap(),
            }),
        };
        let err = check_absent(&forged, &root).unwrap_err().to_string();
        assert!(err.contains("adjacent"), "{err}");
    }

    #[test]
    fn a_neighbour_that_does_not_bracket_is_refused() {
        let map: Vec<(u128, Hash)> = (1..=8u128).map(|i| (i * 10, [i as u8; 32])).collect();
        let root = commit(&map);

        let forged = AbsenceProof {
            inode: 15,
            len: map.len(),
            left: Some(Neighbour {
                index: 2,
                proof: prove(&map, 30).unwrap(),
            }),
            right: Some(Neighbour {
                index: 3,
                proof: prove(&map, 40).unwrap(),
            }),
        };
        assert!(check_absent(&forged, &root).is_err());
    }

    #[test]
    fn claiming_to_be_past_the_end_needs_the_actual_last_leaf() {
        let map: Vec<(u128, Hash)> = (1..=8u128).map(|i| (i * 10, [i as u8; 32])).collect();
        let root = commit(&map);

        // A middle entry dressed up as the final one.
        let forged = AbsenceProof {
            inode: 1_000,
            len: map.len(),
            left: Some(Neighbour {
                index: 2,
                proof: prove(&map, 30).unwrap(),
            }),
            right: None,
        };
        let err = check_absent(&forged, &root).unwrap_err().to_string();
        assert!(err.contains("last entry"), "{err}");
    }

    #[test]
    fn batched_proofs_match_the_ones_proved_singly() {
        for n in 1..=17u128 {
            let map = entries(n);
            let root = commit(&map);
            let wanted: Vec<u128> = map.iter().map(|(i, _)| *i).collect();

            let batch = prove_many(&map, &wanted);
            assert_eq!(batch.len(), wanted.len());
            for proof in &batch {
                assert_eq!(Some(proof.clone()), prove(&map, proof.inode));
                assert!(proof.verify(&root));
            }
        }
    }

    #[test]
    fn batching_skips_entries_that_are_not_there() {
        let map = entries(4);
        let batch = prove_many(&map, &[1, 99, 3]);
        assert_eq!(batch.len(), 2);
        assert_eq!(
            batch.iter().map(|p| p.inode).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn a_path_shape_identifies_exactly_one_position() {
        // What makes a claimed index checkable. If two positions in a map of the same
        // size produced identical path shapes, a prover could present a genuine path
        // for one leaf while claiming the index of another — and adjacency, which the
        // absence proof rests on, would mean nothing.
        for len in 1..=40usize {
            let mut seen = std::collections::HashMap::new();
            for index in 0..len {
                let shape = expected_shape(len, index).expect("index is in range");
                if let Some(other) = seen.insert(shape.clone(), index) {
                    panic!("len {len}: positions {other} and {index} share a shape {shape:?}");
                }
            }
        }
    }

    #[test]
    fn a_neighbour_claiming_the_wrong_index_is_refused() {
        let map: Vec<(u128, Hash)> = (1..=8u128).map(|i| (i * 10, [i as u8; 32])).collect();
        let root = commit(&map);

        let genuine = prove(&map, 30).unwrap();
        let lying = Neighbour {
            index: 5, // really index 2
            proof: genuine,
        };
        let err = check_neighbour(&lying, &root, map.len())
            .unwrap_err()
            .to_string();
        assert!(err.contains("shape"), "{err}");
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

// --- absence, and proofs about many entries at once -------------------------

/// The shape of a leaf's path: one entry per level that contributes a sibling, saying
/// which side that sibling is on.
///
/// A level contributes nothing when the node was promoted for having no partner, so a
/// path is shorter than the tree is deep. That is exactly why the *position* of a leaf
/// cannot be read off a path by counting steps — and why absence proofs have to state
/// the index and have it checked, rather than inferring adjacency from the paths alone.
fn expected_shape(len: usize, index: usize) -> Option<Vec<bool>> {
    if index >= len {
        return None;
    }
    let mut shape = Vec::new();
    let mut n = len;
    let mut pos = index;
    while n > 1 {
        if pos % 2 == 1 {
            shape.push(true); // sibling on the left
        } else if pos + 1 < n {
            shape.push(false); // sibling on the right
        }
        // `fold` pairs adjacent nodes and promotes a lone trailing one, so the next
        // level holds ceil(n/2) nodes and this node lands at pos/2 either way.
        pos /= 2;
        n = n.div_ceil(2);
    }
    Some(shape)
}

/// A neighbour in an absence proof: an inclusion proof plus where it sits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Neighbour {
    pub index: usize,
    pub proof: InclusionProof,
}

/// Evidence that an inode is *not* in a committed map.
///
/// Inclusion proves a positive; absence needs a different shape. The leaves are sorted
/// by inode, so absence is the claim that two *adjacent* entries straddle the inode —
/// there is nowhere else it could be. Adjacency is the load-bearing part: two entries
/// that merely bracket the inode prove nothing, because the inode could be one of the
/// entries between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbsenceProof {
    pub inode: u128,
    /// Number of leaves in the committed map.
    ///
    /// Supplied by the prover and *not* covered by the root — the commitment is over
    /// the leaves alone. What makes it non-forgeable is the shape check in
    /// [`check_neighbour`]: a path that reaches the root already fixes which leaf it
    /// belongs to, and no `(len, index)` pair other than the true one reproduces that
    /// path's shape. `a_forged_length_cannot_manufacture_a_gap` sweeps the whole space
    /// of lies for small maps, because that property is the load-bearing one and it is
    /// not self-evident from reading `expected_shape`.
    pub len: usize,
    /// The greatest entry below `inode`; absent only when the inode precedes them all.
    pub left: Option<Neighbour>,
    /// The least entry above `inode`; absent only when the inode follows them all.
    pub right: Option<Neighbour>,
}

/// Prove that `inode` is absent from `entries`.
///
/// `None` when it is actually present — a caller asking for the wrong kind of proof
/// should be told, not handed one that mysteriously fails to verify later.
pub fn prove_absent(entries: &[(u128, Hash)], inode: u128) -> Option<AbsenceProof> {
    if entries.iter().any(|(i, _)| *i == inode) {
        return None;
    }

    let below = entries.iter().rposition(|(i, _)| *i < inode);
    let above = entries.iter().position(|(i, _)| *i > inode);

    let neighbour = |index: usize| -> Option<Neighbour> {
        Some(Neighbour {
            index,
            proof: prove(entries, entries[index].0)?,
        })
    };

    Some(AbsenceProof {
        inode,
        len: entries.len(),
        left: below.and_then(neighbour),
        right: above.and_then(neighbour),
    })
}

/// Check one neighbour: its path must reach the root *and* match the position claimed.
///
/// Both halves matter. The root check alone would let a prover present a genuine path
/// for some other leaf while claiming whatever index suited them; the shape check binds
/// the path to the stated position, which is what makes the adjacency test meaningful.
fn check_neighbour(n: &Neighbour, root: &Hash, len: usize) -> Result<()> {
    check(&n.proof, root)?;
    let expected = expected_shape(len, n.index)
        .ok_or_else(|| anyhow::anyhow!("index {} is outside a map of {len}", n.index))?;
    let actual: Vec<bool> = n.proof.steps.iter().map(|s| s.sibling_is_left).collect();
    if expected != actual {
        bail!(
            "the path for index {} does not have the shape a map of {len} gives that \
             position",
            n.index
        );
    }
    Ok(())
}

/// Check that an absence proof holds against `root`.
pub fn check_absent(proof: &AbsenceProof, root: &Hash) -> Result<()> {
    if proof.len == 0 {
        // Nothing is in an empty map, and the root is what says it is empty.
        if *root != empty_root() {
            bail!("proof claims an empty map but the root is not the empty root");
        }
        return Ok(());
    }

    match (&proof.left, &proof.right) {
        (Some(left), Some(right)) => {
            check_neighbour(left, root, proof.len)?;
            check_neighbour(right, root, proof.len)?;
            if !(left.proof.inode < proof.inode && proof.inode < right.proof.inode) {
                bail!("the neighbours do not bracket {}", proof.inode);
            }
            if right.index != left.index + 1 {
                bail!("bracketing entries are not adjacent, so the gap is not closed");
            }
            Ok(())
        }
        // Past the last entry: the neighbour must be below the inode *and* be the final
        // leaf, or a middle entry could be dressed up as the end of the map.
        (Some(left), None) => {
            check_neighbour(left, root, proof.len)?;
            if left.proof.inode >= proof.inode {
                bail!("the lower neighbour is not below {}", proof.inode);
            }
            if left.index + 1 != proof.len {
                bail!("claimed to be past the end, but the neighbour is not the last entry");
            }
            Ok(())
        }
        (None, Some(right)) => {
            check_neighbour(right, root, proof.len)?;
            if right.proof.inode <= proof.inode {
                bail!("the upper neighbour is not above {}", proof.inode);
            }
            if right.index != 0 {
                bail!("claimed to be before the start, but the neighbour is not the first entry");
            }
            Ok(())
        }
        (None, None) => bail!("a non-empty map needs at least one neighbour"),
    }
}

/// Inclusion proofs for several entries against one root.
///
/// Convenience rather than compression: the paths still travel in full. What is shared
/// is the *traversal* — the tree is built once instead of once per entry, which is the
/// cost that hurts when answering for a whole directory.
pub fn prove_many(entries: &[(u128, Hash)], inodes: &[u128]) -> Vec<InclusionProof> {
    if entries.is_empty() || inodes.is_empty() {
        return Vec::new();
    }

    let mut levels: Vec<Vec<Hash>> = Vec::new();
    let mut level: Vec<Hash> = entries
        .iter()
        .map(|(inode, value)| leaf_hash(*inode, value))
        .collect();
    levels.push(level.clone());
    while level.len() > 1 {
        level = fold(&level);
        levels.push(level.clone());
    }

    let mut out = Vec::new();
    for inode in inodes {
        let Some(index) = entries.iter().position(|(i, _)| i == inode) else {
            continue;
        };
        let mut steps = Vec::new();
        let mut position = index;
        for level in &levels[..levels.len() - 1] {
            let step = if position % 2 == 0 {
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
            if let Some(step) = step {
                steps.push(step);
            }
            position /= 2;
        }
        out.push(InclusionProof {
            inode: *inode,
            value: entries[index].1,
            steps,
        });
    }
    out
}
