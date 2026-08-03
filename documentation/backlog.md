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

The entire former `Now` band — the local state machine, replay safety, conflict handling
and admin observability — landed with milestone M1. See `./current-status.md`.

## Now

### Replication Core

- Finish Hello and HelloAck handling.
- Implement clock summary comparison and missing-range requests.
- Add op batch transfer with backpressure-aware batching.
- Add blob request and transfer flows.
- Verify remote data before it touches local state.

### Shared Apply Pipeline

- Route remote operations through the existing `apply_op` — local and remote must not
  diverge. Verification, conflict resolution, and pending handling for both missing
  state dependencies and missing blobs already exist.
- Call `retry_pending` when a blob transfer completes, so writes parked on unfetched
  chunks apply as soon as their content lands.

### Second Facade

- The S3-like API is implemented and routed through the same mutation semantics as the
  CLI. POSIX/FUSE remains unimplemented.
- Add SigV4 request signing, or document loopback-only deployment as the supported model.
- Add multipart upload for objects too large to buffer in one request.

## Later

### Encryption And Proof Integration

- Encrypt chunk bytes in the live write path.
- Store and retrieve key envelopes in real file flows.
- Attach transparent proof bundles automatically for newly created operations.
- Validate proof bundles on receipt and reject malformed ones.

### Performance

- Batch storage writes. `SledStore` flushes on every put, costing an fsync per chunk and
  per state record.
- Cache directory maps. `resolve_path` re-reads and re-materializes each directory per
  path component.
- Rebuild snapshots incrementally instead of walking the whole tree on every apply.

### Energy And Resource Management

- Sample battery, temperature, CPU, and storage telemetry.
- Persist the most recent telemetry snapshot.
- Make replication respect the scheduler.
- Add compaction and cleanup policies, including garbage collection of unreferenced
  inodes and blobs.

### Operations And Maintenance

- Add `verify`-style commands for repository integrity.
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

1. Bring up oplog replication between two nodes.
2. Add blob transfer and verified remote apply.
3. Integrate encryption and transparent proofs.
4. Add energy-aware scheduling.
5. Harden operations and maintenance tooling.
6. Start ZK-specific work.

## Definition Of “Ready To Leave Backlog”

A backlog item should move into active implementation only when:

- its dependencies are already stable enough
- the resulting slice can be tested locally
- it does not bypass the core verifier-first design
- it improves the project’s runnable baseline instead of fragmenting it

## Companion Documents

- `./current-status.md`: current implementation snapshot
- `./roadmap.md`: staged milestone plan
