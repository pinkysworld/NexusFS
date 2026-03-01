# Codex Implementation Playbook (Detailed)

This playbook is written as a **checklist of coding tasks** with explicit "Definition of Done"
to reduce ambiguity for an automated coding agent.

> Rule: implement the system in *vertical slices* so you always have a runnable binary.

---

## 0) Working conventions

### 0.1 Compile-first discipline
After each task group:
- run `cargo fmt` (if available)
- run `cargo test -p <crate>`
- run `cargo test` (workspace) at least once per milestone

### 0.2 Error handling and logging
- Use `anyhow::Result` for application-level errors.
- Use `thiserror` for library error enums when needed.
- Use `tracing` throughout.
- Never `unwrap()` on untrusted input from peers.

### 0.3 Determinism rule
Anything hashed must be:
- deterministic
- canonical
- versioned
- documented in `docs/architecture.md`

---

## 1) Milestone M1 — Local core (no networking)

### Task 1.1 Canonical encoding and hashing (crate: `core`)
**Files:**
- `crates/core/src/codec.rs`
- `crates/core/src/hash.rs`
- `crates/core/src/object.rs`

**Implement:**
- `encode_object(Object) -> Vec<u8>` using postcard
- stable ordering of directory entries
- `hash_bytes(data) -> Hash` using blake3
- `hash_object(obj) -> Hash` = hash(encode_object)

**DoD:**
- unit test: encoding is deterministic across repeated calls
- unit test: directory entry ordering is canonical

---

### Task 1.2 Storage backends (crate: `storage`)
**Files:**
- `crates/storage/src/lib.rs`
- `crates/storage/src/sled.rs`

**Implement:**
- `BlobStore` and `KvStore` traits fully
- `SledStore` backend:
  - one tree for kv (namespaced by cf)
  - one tree for blobs keyed by hash bytes
- add size stats methods (optional)

**DoD:**
- tests: put/get/has/delete for blobs
- tests: kv roundtrip and prefix scan

---

### Task 1.3 Chunking + CAS writes (crate: `core`)
**Files:**
- `crates/core/src/chunker.rs`
- `crates/core/src/state.rs`

**Implement:**
- fixed-size chunking:
  - default 1 MiB
  - streaming reader → chunks
- store chunks in `BlobStore`
- return list of `ChunkRef` with offsets

**DoD:**
- test: re-chunking same bytes yields same chunk hashes (before encryption)
- test: concatenating chunks reconstructs original bytes

---

### Task 1.4 Local oplog + apply operations (crate: `core` + `crdt` + `proto`)
**Files:**
- `crates/proto/src/op.rs`
- `crates/crdt/src/or_map.rs`
- `crates/crdt/src/lww.rs`
- `crates/core/src/state.rs`

**Implement:**
- `FsOp` kinds: Mkdir, CreateFile, Write, Rename, Unlink
- apply ops to CRDT state (directory OR-map + inode head register)
- persist oplog in KV:
  - key: `op/<device>/<counter>`
  - value: encoded op
- maintain applied set:
  - key: `applied/<device>/<counter> = 1`

**DoD:**
- tests: idempotent apply (apply same op twice has no effect)
- tests: rename/unlink conflict deterministic naming rule

---

### Task 1.5 Snapshot root + head pointer (crate: `core`)
**Files:**
- `crates/core/src/snapshot.rs`
- `crates/core/src/state.rs`

**Implement:**
- `create_snapshot()` stores SnapshotRoot object in CAS
- persist current head hash in KV (`heads/current`)
- provide `get_head()` API

**DoD:**
- test: after sequence of ops, snapshot head changes and is retrievable
- test: head survives process restart (store reopened)

---

## 2) Milestone M2 — Admin console + CLI

### Task 2.1 CLI wiring (crate: `nexusfs`)
**Files:**
- `crates/nexusfs/src/cli.rs`
- `crates/nexusfs/src/main.rs`

**Implement:**
- `daemon --config <path>`
- `status --config <path>` prints head and basic stats

**DoD:**
- `cargo run -p nexusfs -- status --config examples/nexusfs.toml` works

---

### Task 2.2 Admin HTTP API (crate: `admin`)
**Files:**
- `crates/admin/src/routes.rs`
- `crates/admin/src/assets.rs`

**Implement:**
- GET `/api/status`
- GET `/api/fs/head`
- GET `/api/storage/stats`
- serve embedded index.html for `/`

**DoD:**
- daemon starts and UI loads in browser
- endpoints return JSON

---

## 3) Milestone M3 — POSIX or S3 façade (choose one first)

### Option A: POSIX FUSE (crate: `fs_posix`)
**Implement minimal ops:**
- readdir/lookup/getattr
- create/open/read/write/release
- mkdir/unlink/rename

**DoD:**
- `mkdir`, `cp`, `cat`, `mv`, `rm` work in mounted dir

### Option B: S3-like API (crate: `s3`)
**Implement minimal ops:**
- PUT/GET/DELETE object
- list objects

**DoD:**
- curl can PUT and GET bytes successfully

---

## 4) Milestone M4 — Replication (crate: `net`)

### Task 4.1 QUIC framing + Hello handshake
**Files:**
- `crates/net/src/codec.rs`
- `crates/net/src/quic.rs`

**Implement:**
- framed send/recv
- Hello/HelloAck with ed25519 signature
- trust store stub (TOFU + allowlist)

**DoD:**
- two daemons connect and exchange Hello successfully

### Task 4.2 Have/WantOps/OpsBatch sync
**Files:**
- `crates/net/src/replication.rs`

**Implement:**
- clock summary computation from KV oplog index
- request missing ranges
- send ops in batches with backpressure

**DoD:**
- write op on node A is received and applied on node B

### Task 4.3 WantBlobs/BlobsBatch sync
**Implement:**
- compute referenced hashes from ops
- request missing blobs
- verify hash and store in CAS

**DoD:**
- file content written on A is readable on B after sync

---

## 5) Milestone M5 — Encryption at rest + transparent proofs

### Task 5.1 AEAD encrypt chunks (crate: `crypto` + `core`)
- encrypt chunk bytes before storing in CAS
- store per-file key envelopes

### Task 5.2 Transparent proof bundles (crate: `zk`)
- attach proof bundle to new ops (mode = transparent)
- verify structure on receipt

**DoD:**
- system still replicates correctly
- ops include proof bundle bytes
- verification rejects malformed proofs

---

## 6) Milestone M6 — Energy-aware scheduling (crate: `energy`)

### Task 6.1 Telemetry sampler
- implement Linux sampler (best effort)
- store latest telemetry in KV

### Task 6.2 Scheduler integration
- replication loop asks scheduler for tasks
- degrade blob transfers on low battery

**DoD:**
- toggling telemetry values changes behavior deterministically (test with mocked telemetry)

---

## 7) Milestone M7 — ZK MVP (crate: `zk`)

### Task 7.1 Implement ZkCommit circuit for ONE op
Suggested starting op: `Write`
- public inputs: old_commit, new_commit, inode_id, size
- witness: chunk commitments and offsets
- prove well-formed update

**DoD:**
- proof verifies on receiver
- failures are rejected and logged
- mode is feature-gated and optional

---

## 8) Cross-cutting quality gates

- fuzz tests for message decoding (size limits!)
- property-based tests for merge convergence
- benchmarks for chunking and replication
- docs updated per milestone
