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

Milestones M0 through M4 are done: the local state machine, the S3 facade, QUIC
replication with verified remote apply, and encryption at rest with transparent proofs.
See `./current-status.md`.

## Now

### Energy-Aware Scheduling (M5)

- Sample battery, temperature, CPU and storage telemetry.
- Persist the most recent telemetry snapshot.
- Make the replication loop consult the scheduler before transferring.
- Surface scheduling decisions in the admin console.

### Encryption Follow-Ups

- Per-recipient key envelopes, so replicas need not share one repository key.
- Encrypt file names and directory structure, which are currently in the clear.
- Key rotation and re-encryption of existing content.

### Replication Follow-Ups

- Push notification of new operations, so peers do not wait for the poll interval.
- Delta-encoded operation ranges rather than whole-op batches.
- Peer enrolment out of band, so trust-on-first-use is not the only option.
- Bandwidth and energy-aware scheduling of transfers.

### Second Facade

- The S3-like API is implemented and routed through the same mutation semantics as the
  CLI. POSIX/FUSE remains unimplemented.
- Add SigV4 request signing, or document loopback-only deployment as the supported model.
- Add multipart upload for objects too large to buffer in one request.

## Later

### Performance

- Batch storage writes. `SledStore` flushes on every put, costing an fsync per chunk and
  per state record.
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

1. Wire energy telemetry and the scheduler into background work (M5).
2. Harden operations: compaction, garbage collection, migration tooling (M6).
3. Start ZK-specific work (M7).

## Definition Of “Ready To Leave Backlog”

A backlog item should move into active implementation only when:

- its dependencies are already stable enough
- the resulting slice can be tested locally
- it does not bypass the core verifier-first design
- it improves the project’s runnable baseline instead of fragmenting it

## Companion Documents

- `./current-status.md`: current implementation snapshot
- `./roadmap.md`: staged milestone plan
