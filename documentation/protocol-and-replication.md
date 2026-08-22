# Protocol and Replication

NexusFS replicates intent before payload.

That means peers exchange signed operation knowledge first, then fetch the referenced blobs they do not yet have.

## Replication Shape

1. A peer handshake establishes identity, capabilities, and trust posture.
2. Peers exchange a clock summary describing the highest observed operation counters per
   device — plus the counters observed *above* a gap, so one operation a peer refused
   cannot stall every operation behind it.
3. Missing operations are requested and transferred in batches.
4. Referenced blobs are requested on demand.
5. Received data is verified, stored, and applied idempotently.

The session is pull-based and one-directional: a node asks a peer for what it lacks and
nothing is pushed, so each session has one owner of the loop and no negotiation about who
sends next. Convergence comes from each node pulling from the other.

Each pass is bounded by a budget the energy scheduler supplies, which can skip content
entirely or stop at a byte ceiling and defer the rest. A node that took every operation
and no content still knows what exists and where — and a read of a deferred file fetches
exactly what it needs from a peer rather than waiting for the next pass.

## Why Oplog First?

Oplog-first replication gives NexusFS better behavior under:

- unstable networks
- delayed blob availability
- resumable synchronization
- future proof verification and audit workflows

## Operation Types

The protocol currently models:

- `Mkdir`
- `CreateFile`
- `Write`
- `Rename`
- `Unlink`
- `SetAttr`

Each operation carries:

- a stable `OpId`
- causal context
- author metadata
- a signature
- an optional proof bundle

## Verification Rules

Before remote state becomes trusted:

- signatures must verify, through the same `apply_op` local writes use
- object or blob hashes must match what was requested
- **content that was not requested is dropped**, however well it hashes: a blob being
  self-consistent is not the same as this node having asked for it
- the peer's device key must match the one pinned for it
- policy checks must pass
- enabled proof modes must validate

## Proof Modes

- `None`: signatures and hashes only
- `Transparent`: structured non-ZK evidence — the state root before and after, and the
  object hashes introduced, signed along with the operation
- `ZkCommit`: implemented. Each operation carries a Merkle inclusion path for the entry
  it is about, so a receiver can check the claim against the root without holding the
  author's prior state. This is a commitment scheme, not zero-knowledge: the verifier
  learns the inode and its object hash, just not the rest of the tree
- `ZkFull`: unimplemented. Accepted as a config value and treated as `None` rather than
  pretending to prove anything

Transparent bundles remain acceptable under commit policy — they prove less but are not
wrong, and refusing them would make the mode unusable while a cluster is mid-upgrade.

Because the commitment *is* the state root, it is not compile-time optional: a build
computing a different root could never replicate with one that did not. Changing it came
with on-disk format v2 and `PROTOCOL_VERSION` 2. Per-recipient sealing brought v3, which
does change the wire format: a `Write` carrying encrypted content describes how its file
key is protected in a different shape. Either way a stale store is refused until migrated
and a mismatched peer refuses the handshake rather than syncing and then disagreeing
forever.

For message details, use `../docs/protocol.md`.
