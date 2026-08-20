# Roadmap

Last updated: August 2, 2026

This roadmap turns the current NexusFS blueprint into a staged execution plan.

The key rule is simple: every milestone must leave behind a repository that still builds, runs, and is easier to extend than the one before it.

## Delivery Principles

1. Ship vertical slices, not isolated subsystems.
2. Keep the single-binary story intact across milestones.
3. Favor verifiable local correctness before distributed complexity.
4. Avoid adding research-heavy features until the practical baseline is dependable.
5. Every milestone should improve observability, not just functionality.

## Current Position

- `M0` complete
- `M1` complete
- `M2` complete via the S3-like facade
- `M3` complete: two nodes converge over QUIC
- `M4` complete: encryption at rest and transparent proofs
- `M5` complete: energy-aware replication scheduling
- `M6` complete: operator tooling and failure-mode cover
- `M7` complete: a Merkle state commitment with checkable inclusion proofs
- `M8` complete for the tracks that were engineering; the rest is named as research

The workspace, daemon, storage baseline, docs, and the local filesystem core are real: a
signed operation log drives CRDT-backed namespace state, files round-trip through the CLI,
state survives restart, two nodes converge over QUIC, content can be encrypted at rest,
and replication adapts what it transfers to the device's power situation. What remains is
operational hardening, the POSIX facade, and the proof systems.

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

Status: Complete

Primary goal:

- complete a reliable single-node local filesystem core with persistent state

Delivered:

- canonical object encoding and deterministic hashing
- chunking and content-addressed blob writes
- CRDT-backed namespace state: OR-Map directories and LWW inode registers
- deterministic inode allocation derived from operation ids
- full apply logic for Mkdir, CreateFile, Write, Rename, Unlink and SetAttr
- signature verification enforced before any state change
- pending-operation queue for causally early operations
- deterministic conflict naming applied in live directory reads
- path resolution, directory listing and whole-file reads
- snapshots built from live state, committing to both structure and content
- CLI verbs (`mkdir`, `put`, `cat`, `ls`, `rm`, `mv`) and expanded admin API

Exit criteria — all met:

- a sequence of local file operations mutates persisted namespace state correctly
- restart preserves local filesystem state, oplog state, and current head
- idempotent operation replay is verified by tests
- the same operation set applied in different orders converges to one state root
- admin APIs report head, state root, oplog and storage state

### M2: First External Facade

Status: Complete (S3 facade; POSIX/FUSE not implemented)

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

Status: Complete

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

Status: Complete

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

Status: Complete

Primary goal:

- make background work adaptive to edge-device realities

Deliverables:

- telemetry sampling — done: power source, charge, temperature, load and link cost, via
  `pmset`/`sysctl` on macOS and sysfs on Linux, with explicit unknowns everywhere else
- scheduler decisions based on battery, temperature, and link cost — done: a rule-based
  scheduler grading battery and treating heat and metered links as overrides
- replication throttling modes — done: the session accepts a budget and can skip content
  entirely or stop at a byte ceiling, deferring the rest to a later pass
- admin visibility into current scheduling state — done: `/api/energy` reports the
  reading, the budget, and the rule that fired

Deliberately not done:

- *persisted telemetry snapshots.* A power reading from before a restart describes a
  machine that may since have been unplugged, moved, or cooled. Persisting it would
  produce a stored value that looks authoritative and is not, so the daemon samples on
  demand and caches only for the life of the process.

Exit criteria:

- replication behavior changes predictably under low-power or high-heat conditions —
  met, and covered by a decision table over the full input space
- scheduler decisions are testable and observable — met: the policy is a pure function
  under unit test, and replication honouring it is tested end to end over a real session

### M6: Operational Hardening

Status: Complete

Primary goal:

- turn the system from a research prototype into a maintainable operator-facing platform

Deliverables:

- storage accounting and cleanup — done: `nexusfs gc`, surveying by default, plus
  `/api/storage/gc` for a read-only view from the running daemon
- integrity verification commands — done in M4 as `nexusfs verify`, and extended here
  with tests proving it reports damage rather than absorbing it
- migration support — done: an on-disk format stamp, refusal in both directions, and
  `nexusfs migrate` with the step machinery in place
- improved trust onboarding — done: `nexusfs peer identity|list|add|remove`, making
  `net.tofu = false` usable
- broader integration and failure-mode tests — done: 30 new tests across collection,
  format, enrolment and damage

Exit criteria:

- operators can inspect, recover, and maintain state with built-in tooling — met:
  `status`, `verify`, `gc`, `migrate` and `peer`, each with an admin-API counterpart
  where safe to expose
- upgrade paths are documented and testable — met: the format stamp is enforced on every
  open path and covered by tests in both directions; the first real migration has a
  documented shape to follow

### M7: ZK Commitments

Status: Complete, with one deliverable deliberately scoped down — see below.

Primary goal:

- prove one real state transition under a commitment-friendly verification mode

Deliverables:

- first `ZkCommit`-mode proof **circuit** — delivered as a commitment scheme, not a
  circuit. The state root became a Merkle tree and operations carry inclusion paths.
  See "What was not built" below.
- one end-to-end proof generation path — done: `nexusfs prove` emits a self-contained
  proof, `nexusfs check-proof` verifies one without opening a repository, and
  `/api/fs/proof` serves the same structure
- receiver-side verification integration — done: `proof_mode = "zk_commit"` attaches an
  inclusion path per operation, checked on receipt against the root it claims
- a clear fallback path to transparent proofs — done: transparent bundles remain
  acceptable under commit policy, and an operation whose subject is not in the live tree
  emits a transparent bundle rather than proving the wrong entry

Exit criteria:

- one concrete operation type can be proved and verified end to end — met for every
  operation type, and verified by a party holding no filesystem state
- ZK mode remains optional — met: `proof_mode` selects it per node, and transparent
  interoperates. Not *feature-gated* at compile time, because the commitment is the
  state root: a build that computed a different root could never replicate with one
  that did not, so making it conditional would be a convergence hazard rather than an
  option

## What was not built

A proving system. `ZkCommit` names the commitment half of what a SNARK needs — an
inclusion path is exactly the witness a circuit would consume — and stops there.

Going further means adopting a proving backend, choosing a transparent or trusted setup,
and arithmetizing the hash. BLAKE3 is hostile to circuits, so a real implementation would
switch the commitment to something like Poseidon, which changes the state root again.
That is a research effort with its own milestone-sized risk, and a stub that looked like
a circuit would be worse than an honest commitment layer.

`zk_full` therefore remains unimplemented and behaves as `none`. Proving *absence* is
also unbuilt: the sorted leaf layout supports the usual approach of proving the two
entries that bracket a gap, but nothing needs it yet.

### M8: Research Expansion

Status: Complete for the tracks that could be built and tested. The remainder is named
below rather than left implied.

Primary goal:

- expand beyond the practical baseline into deeper research tracks once the core system is stable

Candidate tracks, and what happened to each:

- **proof batching** — done. `prove_many` builds the tree once and reads every requested
  path out of the same levels. Convenience rather than compression: the paths still
  travel in full, but the *prover* stops rebuilding the tree per entry, which is the
  cost that hurts when answering for a whole directory.
- **additional ZK coverage** — done as absence proofs, which complete the commitment
  story: an inclusion proof against an old root and an absence proof against a new one
  demonstrate a deletion to someone holding neither state.
- **stronger privacy layers** — not built. The obvious next step is per-recipient key
  envelopes so replicas need not share one repository key; `crypto::envelope` already
  seals and opens but is not wired into the write path. Encrypting names and directory
  structure is a larger change and would break the S3 facade's key mapping.
- **DTN / store-carry-forward** — not built. The replication session is already
  pull-based, resumable and budget-aware, which is most of what a delay-tolerant mode
  needs; what is missing is a transport that is not a live socket.
- **richer policy systems** — not built, and deliberately not sketched. Policy without a
  concrete requirement produces configuration nobody uses.

Also delivered under this milestone, from the performance backlog: durability moved from
the record to the operation boundary, which made applying an operation 6.8-9.5x faster.

## What is still open

- Incremental snapshots. The state-root walk is now the visible cost of an apply —
  3.6ms at 1000 entries, roughly 40% — and it grows with the tree. Making it incremental
  needs parent pointers and a persisted inode map.
- A mountable interface. POSIX/FUSE is *not* outstanding work: M2's goal was one
  user-facing facade and named POSIX and S3 as alternatives, and the S3 one shipped.
  Anything here is new scope, not a debt.
- A proving system, which stays behind its own milestone for the reasons in M7.

Exit criteria:

- research additions do not destabilize the practical baseline
- optional tracks stay clearly separated from core production flows

## Execution Sequence

The preferred delivery order is:

1. Finish M1 local state correctness
2. Ship one real facade in M2
3. Complete M3 replication
4. Add M4 encryption and transparent proofs (done)
5. Integrate M5 energy-aware scheduling (done)
6. Harden operations in M6 (done)
7. Introduce M7 ZK commitments (done)
8. Expand research work in M8 (done for the buildable tracks)

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
