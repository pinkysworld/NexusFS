# NexusFS Architecture Blueprint

This document defines **design decisions, invariants, and module boundaries** so implementation can proceed with minimal ambiguity.

## 1. Primary goals

1) **Single executable**
- One binary runs everything: local storage, replication, admin UI, and optional façades.
- No external services required at runtime.

2) **Offline-first**
- Works under intermittent connectivity and partitions.
- Supports eventual convergence with deterministic conflict rules.

3) **Verifiable**
- Every operation is **signed** and can carry a **proof bundle**.
- Start with transparent proofs (signatures + hashes) and evolve to ZK proofs.

4) **Private**
- Encryption at rest and in transit.
- Optional privacy modes: size padding, cover traffic, oblivious metadata access (research).

5) **Edge-friendly**
- Resource-aware background jobs (replication, compaction, proof generation).
- Energy-aware scheduler that can degrade gracefully.

## 2. Non-goals (for initial versions)

- Strict global serializability across devices (too heavy for offline-first).
- Full AWS S3 API parity in v0 (start with a minimal subset).
- Full ORAM across all data (too expensive); start with metadata-only or padding strategies.
- In-circuit BLAKE3 preimage proofs (ZK-full mode is research-grade).

## 3. Architectural invariants (must remain true)

**I1 — Canonical, deterministic object encoding**
- Hashes must be stable across platforms and versions (within a given object version).

**I2 — Immutable objects**
- Content-addressed objects never mutate. Mutable state is represented by new objects + updated heads.

**I3 — Signed operation log**
- Every filesystem mutation is represented as a signed operation in the oplog.

**I4 — Idempotent replication**
- Replaying received operations must be safe (dedupe by op id).

**I5 — Verifier-first**
- Remote data is accepted only if:
  - signatures verify
  - hashes match content
  - policy checks pass
  - and (if enabled) proofs verify

**I6 — Policy and privacy are explicit**
- Policies (ACL, padding, sync rules) must be serialized, versioned, and auditably applied.

## 4. Layers and crate boundaries

### 4.1 `core`
Responsibilities:
- Object model (Chunk/FileNode/DirNode/SnapshotRoot)
- Canonical encoding and hashing
- Chunking strategies (fixed-size v0; CDC later)
- Snapshot creation and head management
- Local state mutation logic (apply ops, build snapshots)

Hard rule:
- `core` must not depend on networking frameworks.

### 4.2 `storage`
Responsibilities:
- Generic CAS + KV traits
- Backend implementations (sled default; rocksdb optional)
- Compaction hooks and statistics

### 4.3 `crypto`
Responsibilities:
- Device identity (ed25519 signing keys)
- AEAD encryption for chunks
- Key envelopes for file/folder keys (X25519/HPKE-like patterns)
- Message authentication helpers

### 4.4 `proto`
Responsibilities:
- Shared message types:
  - `FsOp` (operation log entries)
  - replication protocol messages
  - common types (`Hash`, `OpId`, etc.)

### 4.5 `crdt`
Responsibilities:
- OR-Map for directories
- LWW or MV registers for inode heads
- Deterministic conflict resolution rules

### 4.6 `net`
Responsibilities:
- QUIC session management
- Framing and message exchange
- Replication state machine (ops → blobs)
- Peer auth handshake and feature negotiation

### 4.7 `admin`
Responsibilities:
- Embedded HTTP server
- Websocket updates
- Track registry and per-track route namespaces
- Serve embedded static UI assets

### 4.8 `energy`
Responsibilities:
- Telemetry sampling (battery/temp/cpu/link cost)
- Scheduler interface and baseline rule-based scheduler

### 4.9 `privacy`
Responsibilities:
- Size padding policies
- Cover traffic scheduling (rate-limited and energy-aware)
- Future: oblivious access layers

### 4.10 `zk`
Responsibilities:
- Proof mode selection and interfaces
- Transparent proof bundles (non-ZK) for immediate verifiability
- ZK commit mode scaffolding (Poseidon-based commitments, roots, circuits)

### 4.11 `fs_posix` and `s3`
Responsibilities:
- Translate API calls/syscalls into oplog operations and reads
- Keep consistent semantics across façades

## 5. Data model

### 5.1 Content Addressed Store (CAS)
Keyed by `Hash = [u8; 32]` (BLAKE3 digest).
Stores opaque blobs (usually ciphertext), with optional indexing.

### 5.2 Primary objects

- `Chunk`: encrypted/compressed data fragment
- `FileNode`: ordered list of chunk references + file metadata
- `DirNode`: directory entries + metadata
- `SnapshotRoot`: commits to a consistent filesystem view; used as a sync "head"

### 5.3 Mutable state
Mutable references are stored in KV:
- `inode_id -> current_object_hash`
- `dir_inode_id -> OR-Map state`
- `head -> current SnapshotRoot hash`
- `oplog`: appended operations + index by device/counter

## 6. Operation log design

All mutations append an `FsOp`:
- globally unique `OpId { device_id, counter }`
- causal context (vector-clock-like deps; starts simple)
- op kind (mkdir, create, write, rename, unlink, setattrs, ...)
- author pubkey + signature
- optional proof bundle

The oplog is replicated first; data blobs are fetched on-demand.

## 7. Replication design

Baseline:
- peer handshake and feature negotiation
- exchange "what ops I have" summary
- fetch missing ops
- fetch referenced blobs
- verify, apply, and update head

Advanced:
- partial connectivity & store-carry-forward
- energy-aware scheduling
- proof batching and checkpointing

## 8. Energy-aware scheduling

The replication loop must consult a scheduler:
- avoid heavy transfers on low battery/high temperature
- prefer oplog sync over blob sync under constraints
- prioritize recently accessed or pinned content

## 9. Proof modes

- **None**: signatures + hashes only
- **Transparent**: structured evidence of state transition (old head/new head, changed hashes)
- **ZkCommit**: ZK proofs over Poseidon commitments and SNARK-friendly Merkle roots
- **ZkFull**: research-grade, in-circuit crypto proofs (optional)

## 10. Versioning & upgrades

- Every object has `(type_tag, version)` header.
- Protocol messages include `protocol_version` and `features`.
- Backward compatible upgrades must be additive where possible.
- When incompatible:
  - run a migration tool inside the same executable (`nexusfs migrate`).

## 11. Observability

Expose:
- structured logs (tracing)
- metrics endpoints (optional)
- admin UI:
  - head hash, peer health, oplog size, CAS size
  - energy telemetry
  - proof stats (size, verification time)
