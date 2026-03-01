# Backlog

Last updated: March 1, 2026

This backlog organizes outstanding work by priority, dependency, and implementation area.

It is intentionally biased toward execution order, not just feature categories.

## Priority Bands

- `Now`: directly blocks the next meaningful milestone
- `Next`: unlocks the milestone after that or materially improves operator confidence
- `Later`: important, but should follow the practical baseline
- `Research`: should stay behind feature flags until the production baseline is solid

## Now

### Core State Machine

- Replace `apply_op_minimal` with real state transitions for directories and files.
- Persist inode-to-head mappings in KV.
- Persist directory state in a form that supports deterministic reconstruction.
- Build snapshots from actual current state instead of a placeholder root-only path.
- Implement operation-specific state behavior for:
  - `Mkdir`
  - `CreateFile`
  - `Write`
  - `Rename`
  - `Unlink`

### Correctness And Replay Safety

- Make local apply robust when operations arrive twice.
- Define and implement the behavior for operations whose prerequisites are missing.
- Add tests for deterministic conflict naming in live apply flows, not only helper functions.
- Add tests for rename vs unlink interactions.

### Admin Observability

- Add storage stats endpoints.
- Add applied-op counters and summary endpoints.
- Expose local repository metadata beyond the current head hash.
- Expand the embedded admin UI so it reflects actual local state, not only basic status.

## Next

### Replication Core

- Finish Hello and HelloAck handling.
- Implement clock summary comparison and missing-range requests.
- Add op batch transfer with backpressure-aware batching.
- Add blob request and transfer flows.
- Verify remote data before it touches local state.

### Shared Apply Pipeline

- Ensure local and remote operations use the same state transition logic.
- Add a pending-ops queue when dependencies or blobs are not yet available.
- Add recovery behavior when blobs are missing during apply.

### First Facade

- Choose the first external surface:
  - S3-like API
  - POSIX/FUSE
- Route the facade through the same local mutation semantics used by the daemon and replication layer.

## Later

### Encryption And Proof Integration

- Encrypt chunk bytes in the live write path.
- Store and retrieve key envelopes in real file flows.
- Attach transparent proof bundles automatically for newly created operations.
- Validate proof bundles on receipt and reject malformed ones.

### Energy And Resource Management

- Sample battery, temperature, CPU, and storage telemetry.
- Persist the most recent telemetry snapshot.
- Make replication respect the scheduler.
- Add compaction and cleanup policies.
- Add storage accounting and capacity reporting.

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

1. Finish persistent local namespace state.
2. Replace placeholder apply logic with real CRDT-backed operation handling.
3. Expand admin visibility so local correctness is easy to inspect.
4. Bring up oplog replication.
5. Add blob transfer and verified remote apply.
6. Implement the first user-facing facade.
7. Integrate encryption and transparent proofs.
8. Add energy-aware scheduling.
9. Harden operations and maintenance tooling.
10. Start ZK-specific work.

## Definition Of “Ready To Leave Backlog”

A backlog item should move into active implementation only when:

- its dependencies are already stable enough
- the resulting slice can be tested locally
- it does not bypass the core verifier-first design
- it improves the project’s runnable baseline instead of fragmenting it

## Companion Documents

- `./current-status.md`: current implementation snapshot
- `./roadmap.md`: staged milestone plan
