# Backlog

Last updated: August 2, 2026

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
- Peer enrolment out of band, so trust-on-first-use is not the only option.
- Prioritise which deferred content to fetch first when the budget is capped — currently
  the order is whatever `missing_chunk_hashes` returns, not what a user is likely to want.

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
- Rebuild snapshots incrementally instead of walking the whole tree on every apply.

### Storage Maintenance

- Add compaction and cleanup policies, including garbage collection of unreferenced
  inodes and blobs.

### Operations And Maintenance

- Add migration tooling for future schema changes.
- Improve trust and peer enrollment flows.
- Add better structured logs and operator diagnostics.

### Test Expansion

- Add cross-node integration tests.
- Add restart and crash-recovery tests.
- Add admin API coverage beyond the minimal routes.
- Add transport failure and retry tests.

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

1. A mountable interface, if one is wanted. See the note in `./current-status.md`:
   POSIX/FUSE is not owed — M2 asked for *one* facade and the S3 one shipped — and
   WebDAV reaches the same user-visible outcome without a kernel extension.
2. An incremental Merkle tree, so a one-entry change costs O(log n) rather than a
   rebuild. Structural, but an apply is fsync-bound today, so measure before starting.
3. Detect metered links per platform, so that scheduler rule can fire in the field.

## Definition Of “Ready To Leave Backlog”

A backlog item should move into active implementation only when:

- its dependencies are already stable enough
- the resulting slice can be tested locally
- it does not bypass the core verifier-first design
- it improves the project’s runnable baseline instead of fragmenting it

## Companion Documents

- `./current-status.md`: current implementation snapshot
- `./roadmap.md`: staged milestone plan
