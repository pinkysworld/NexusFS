# Backlog

Last updated: August 21, 2026

This backlog organizes outstanding work by priority, dependency, and implementation area.

It is intentionally biased toward execution order, not just feature categories.

## Priority Bands

- `Now`: directly blocks the next meaningful milestone
- `Next`: unlocks the milestone after that or materially improves operator confidence
- `Later`: important, but should follow the practical baseline
- `Research`: should stay behind feature flags until the production baseline is solid

## Recently Completed

Milestones M0 through M8 are done: the local state machine, the S3 facade, QUIC
replication with verified remote apply, encryption at rest with transparent proofs,
energy-aware replication scheduling, operator tooling — collection, format versioning and
explicit peer enrolment — a Merkle state commitment with checkable inclusion and absence
proofs, and the storage durability work that made an apply 6.8-9.5x faster. M8's
remaining research tracks are named in `./roadmap.md` rather than left implied. See
`./current-status.md`.

Since then, and previously listed here as outstanding:

- **Incremental snapshots.** The inode map is maintained with parent pointers instead of
  rebuilt by a full walk on every apply. The state root fell from 3.66ms to 1.04ms at a
  thousand entries, and correctness is pinned by an invariant suite that re-derives the
  map with a full walk after every kind of operation and demands equality.
- **On-demand fetch of deferred content.** A read asks the daemon's transport for exactly
  what it is missing, so a node under a metadata-only budget can serve a file the moment
  it is asked for. The facades return `503` rather than a short file when it still cannot
  be had.
- **One fsync per operation.** `CoreState::flush` was syncing the same sled log twice,
  because the blob store and the key-value store are the same database in every
  deployment. That was 7ms of a 16.7ms operation boundary.
- **A review round** over the replication and proof paths: a peer's unsolicited content
  is refused rather than stored, `check-proof` no longer repeats the proof file's own
  unverified labels as if they were established, a malformed `net.listen` no longer takes
  the admin console down with it, and CI builds every feature combination rather than
  two of seven.

## Now

### Energy Follow-Ups

- Detect metered links per platform. The scheduler already treats a metered link as an
  override, but nothing ever reports one, so that rule cannot fire in the field.
- Read storage headroom, which `Telemetry` carries but nothing populates.
- Consider surfacing the budget in the CLI (`status`), not only over the admin API.

### Proof Follow-Ups

- Compressed batches: `prove_many` shares the traversal but still sends every path in
  full. Overlapping paths could share their common upper steps.
- A proving system, if and when the commitment moves to a circuit-friendly hash. This is
  research, not engineering, and should stay behind its own milestone.

### Encryption Follow-Ups

- Per-recipient key envelopes, so replicas need not share one repository key.
- Encrypt file names and directory structure, which are currently in the clear.
- Key rotation and re-encryption of existing content.

### Replication Follow-Ups

- Push notification of new operations, so peers do not wait for the poll interval.
- Delta-encoded operation ranges rather than whole-op batches.
- Prioritise which deferred content to fetch first when the budget is capped — currently
  the order is whatever `missing_chunk_hashes` returns, not what a user is likely to want.

Peer enrolment out of band is **done**: `nexusfs peer identity|list|add|remove` makes
`net.tofu = false` usable, and two nodes converge on pre-enrolled keys alone.

### Second Facade

- The S3-like API is implemented and routed through the same mutation semantics as the
  CLI. POSIX/FUSE remains unimplemented.
- Add SigV4 request signing, or document loopback-only deployment as the supported model.
- Add multipart upload for objects too large to buffer in one request.

## Later

### Performance

- Update the Merkle tree incrementally. The inode map is maintained now, but the
  commitment over it is rebuilt each apply — linear work for a one-entry change.
- Cache directory maps. `resolve_path` re-reads and re-materializes each directory per
  path component.

### Storage Maintenance

- Add compaction and cleanup policies, including garbage collection of unreferenced
  inodes and blobs.

### Operations And Maintenance

- Add better structured logs and operator diagnostics.
- Surface the energy budget in `nexusfs status`, so a node built without the admin
  feature can still explain its own throttling.

Migration tooling and peer enrolment are **done** — `nexusfs migrate` with an enforced
on-disk format stamp, and `nexusfs peer` with explicit key enrolment and `--rotate`.

### Test Expansion

- Add cross-node integration tests beyond the two-node script.
- Add restart and crash-recovery tests.
- Add admin API coverage beyond the minimal routes.
- Add transport failure and retry tests.

The suite is 167 tests today, covering convergence, conflict naming, encryption,
replication over both an in-memory pipe and real QUIC sockets, the scheduler's decision
table, collection safety, format refusals in both directions, and the Merkle commitment
including the forgeries an absence proof must refuse.

## Research

### Proof And ZK Work

- Implement the first `ZkCommit` circuit for a real operation type.
- Define the data model that binds transparent and ZK proof paths cleanly.
- Add feature-gated end-to-end proof verification for ZK-capable peers.

### Privacy Work

- Expand padding policy integration into real file flows.
- Add cover-traffic scheduling that cooperates with the energy scheduler.
- Explore metadata leakage reduction strategies.

### Advanced Systems Work

- Add store-carry-forward and delay-tolerant sync ideas.
- Explore proof batching and checkpointing.
- Add more expressive policy and trust layers.

## Suggested Work Queue

The most effective execution order right now is:

1. Detect metered links per platform, so that scheduler rule can fire in the field. The
   cheapest item here, and the only one that turns a tested rule into a working one.
2. Per-recipient key envelopes, so replicas need not share one repository key.
   `crypto::envelope` already seals and opens; the write path does not use it.
3. A mountable interface, if one is wanted. See the note in `./current-status.md`:
   POSIX/FUSE is not owed — M2 asked for *one* facade and the S3 one shipped — and
   WebDAV reaches the same user-visible outcome without a kernel extension.
4. An incremental Merkle tree, so a one-entry change costs O(log n) rather than a
   rebuild. Structural, but an apply is fsync-bound today, so measure before starting.

## Definition Of “Ready To Leave Backlog”

A backlog item should move into active implementation only when:

- its dependencies are already stable enough
- the resulting slice can be tested locally
- it does not bypass the core verifier-first design
- it improves the project’s runnable baseline instead of fragmenting it

## Companion Documents

- `./current-status.md`: current implementation snapshot
- `./roadmap.md`: staged milestone plan
