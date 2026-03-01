# Roadmap

Last updated: March 1, 2026

This roadmap turns the current NexusFS blueprint into a staged execution plan.

The key rule is simple: every milestone must leave behind a repository that still builds, runs, and is easier to extend than the one before it.

## Delivery Principles

1. Ship vertical slices, not isolated subsystems.
2. Keep the single-binary story intact across milestones.
3. Favor verifiable local correctness before distributed complexity.
4. Avoid adding research-heavy features until the practical baseline is dependable.
5. Every milestone should improve observability, not just functionality.

## Current Position

NexusFS currently sits between:

- `M0` complete
- `M1` partially implemented

The workspace, daemon, storage baseline, docs, and initial local-core primitives are already real. The next milestones focus on completing the local state machine and then layering in replication, encryption, and higher-level facades.

## Milestone Roadmap

### M0: Bootstrapped Workspace

Status: Complete

Primary goal:

- establish the project shape and a runnable baseline

Delivered:

- multi-crate Rust workspace
- runnable daemon and CLI entrypoint
- embedded admin surface stub
- architectural, protocol, and threat-model documentation
- public docs and project website

Exit criteria:

- repository builds consistently
- daemon boots locally
- contributors can understand the intended system shape

### M1: Local Filesystem Core

Status: In progress

Primary goal:

- complete a reliable single-node local filesystem core with persistent state

Core deliverables:

- canonical object encoding and deterministic hashing
- chunking and content-addressed blob writes
- persistent snapshots and head management
- full oplog persistence and replay protection
- real directory and inode state mutation
- CRDT-backed namespace state

Still required to complete M1:

- replace placeholder `apply_op_minimal` behavior with full state application
- persist mutable directory and inode mappings, not just heads and ops
- build snapshots from actual namespace state
- represent rename, unlink, and conflicts deterministically in live state
- expose local storage and state stats through the admin API

Exit criteria:

- a sequence of local file operations mutates persisted namespace state correctly
- restart preserves local filesystem state, oplog state, and current head
- idempotent operation replay is verified by tests
- admin APIs can report head, oplog, and storage state clearly

### M2: First External Facade

Status: Not started

Primary goal:

- expose one practical user-facing interface on top of the local state machine

Decision point:

- implement either POSIX/FUSE first or the S3-like API first

Recommended order:

- S3-like facade first, because it is easier to validate in CI and less OS-specific than FUSE

Deliverables if S3-like facade goes first:

- PUT object
- GET object
- DELETE object
- LIST objects
- object-key translation into internal file operations

Deliverables if POSIX goes first:

- readdir, lookup, getattr
- create, read, write
- mkdir, rename, unlink

Exit criteria:

- one facade supports real read/write flows backed by the same local state machine
- the facade does not bypass oplog semantics
- a user can exercise the system without interacting with internal APIs directly

### M3: Verified Replication

Status: Not started

Primary goal:

- make two nodes converge over the network using the same verifier-first state machine

Deliverables:

- peer handshake and feature negotiation
- clock summary exchange
- Have/WantOps/OpsBatch synchronization
- WantBlobs/BlobsBatch transfer
- verified remote apply
- basic peer status visibility in the admin surface

Execution order:

1. stabilize the Hello/HelloAck handshake
2. add oplog synchronization
3. add blob transfer
4. unify remote apply with local apply logic
5. add observability for sync status and failures

Exit criteria:

- two nodes can exchange operations and referenced blobs
- receiving nodes verify signatures and hashes before applying state
- repeated sync attempts remain safe and idempotent
- sync progress and peer health are visible to operators

### M4: Encryption And Transparent Proofs

Status: Not started

Primary goal:

- make stored and replicated state more trustworthy and more private without changing the core workflow model

Deliverables:

- chunk encryption in the live write path
- real file-key or object-key envelope storage
- transparent proof bundle generation for new operations
- proof bundle verification on receipt
- verification tooling for local inspection

Exit criteria:

- encrypted content still supports local reads and replication
- malformed proof bundles are rejected deterministically
- proof data is visible and debuggable in tooling or admin output

### M5: Energy-Aware Scheduling

Status: Not started

Primary goal:

- make background work adaptive to edge-device realities

Deliverables:

- telemetry sampling
- persisted telemetry snapshots
- scheduler decisions based on battery, temperature, and link cost
- replication throttling modes
- admin visibility into current scheduling state

Exit criteria:

- replication behavior changes predictably under low-power or high-heat conditions
- scheduler decisions are testable and observable

### M6: Operational Hardening

Status: Not started

Primary goal:

- turn the system from a research prototype into a maintainable operator-facing platform

Deliverables:

- storage accounting and cleanup
- integrity verification commands
- migration support
- improved trust onboarding
- broader integration and failure-mode tests

Exit criteria:

- operators can inspect, recover, and maintain state with built-in tooling
- upgrade paths are documented and testable

### M7: ZK Commitments

Status: Not started

Primary goal:

- prove one real state transition under a commitment-friendly verification mode

Deliverables:

- first `ZkCommit`-mode proof circuit
- one end-to-end proof generation path
- receiver-side verification integration
- a clear fallback path to transparent proofs

Exit criteria:

- one concrete operation type can be proved and verified end to end
- ZK mode remains optional and feature-gated

### M8: Research Expansion

Status: Not started

Primary goal:

- expand beyond the practical baseline into deeper research tracks once the core system is stable

Candidate tracks:

- stronger privacy layers
- proof batching
- DTN/store-carry-forward replication modes
- richer policy systems
- additional ZK coverage

Exit criteria:

- research additions do not destabilize the practical baseline
- optional tracks stay clearly separated from core production flows

## Execution Sequence

The preferred delivery order is:

1. Finish M1 local state correctness
2. Ship one real facade in M2
3. Complete M3 replication
4. Add M4 encryption and transparent proofs
5. Integrate M5 energy-aware scheduling
6. Harden operations in M6
7. Introduce M7 ZK commitments
8. Expand research work in M8

## Critical Dependencies

- M2 depends on M1 being trustworthy enough to support user-facing reads and writes.
- M3 depends on M1 local apply logic being the single source of truth.
- M4 depends on M3 enough to test encrypted replication.
- M5 depends on M3 replication loops existing in a real form.
- M6 should run in parallel with late M3 and M4 hardening where practical.
- M7 should not begin before M4 transparent proof structure is dependable.

## Success Markers For The Next 90 Days

The highest-value short-term targets are:

1. complete the real local state machine
2. surface richer state in the admin API
3. bring up oplog replication between two nodes
4. follow with blob transfer and remote apply

If those land, NexusFS moves from “promising local core” into “real distributed system baseline.”

## Companion Documents

- `./current-status.md`: what is implemented right now
- `./backlog.md`: prioritized outstanding work
- `../docs/roadmap.md`: compact internal milestone map
