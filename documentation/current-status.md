# Current Status

Last updated: March 1, 2026

This page summarizes what NexusFS currently implements in the repository and what remains in the backlog.

## Overall State

NexusFS is beyond the pure skeleton stage, but it is still in an early milestone state.

The project currently delivers:

- a working Rust workspace
- a runnable `nexusfs` binary
- a persistent local storage baseline
- a minimal embedded admin surface
- the first practical slice of the local core

It does not yet deliver a full distributed filesystem experience end to end.

## Implemented Now

### Repository and Build Baseline

- The Rust workspace is bootstrapped and builds as a cohesive multi-crate project.
- The `nexusfs` binary supports `daemon` and `status` commands.
- The repository includes public-facing docs in `documentation/` and a static project website in `site/`.
- A GitHub Pages deployment workflow is included.

### Local Configuration and Identity

- TOML config loading is implemented for node, admin, network, security, and energy settings.
- The daemon creates and reuses a persistent device identity.
- The daemon creates and reuses an admin token when one is not supplied in config.

### Storage Layer

- `BlobStore` and `KvStore` traits are present.
- The sled backend is implemented and tested for blob put/get/has/delete and KV prefix scans.
- Objects and blobs are persisted locally through the sled-backed storage layer.

### Core Filesystem Foundations

- Canonical object encoding is implemented using `postcard`.
- Directory entries are normalized before object encoding and hashing.
- BLAKE3 hashing is used for raw bytes and content-addressed object storage.
- Fixed-size chunking is implemented.
- Chunk references now preserve hash, byte length, and byte offset.
- File content can be chunked and stored in the local blob store.
- Snapshot roots can be created and persisted.
- The current head pointer is stored and survives reopening the store.

### Oplog Baseline

- Filesystem operation types are defined in the shared protocol crate.
- Minimal oplog append and clock-summary logic are implemented.
- An applied-set marker exists for replay protection.
- `apply_op_minimal` is idempotent and updates the snapshot head in the current baseline flow.

### Admin Surface

- The embedded admin server is wired into the daemon.
- The project serves a local static admin UI.
- The API currently exposes:
  - `/api/status`
  - `/api/fs/head`
  - `/api/oplog/summary`

### Dependency and Compatibility Fixes Already Landed

- Current crate dependency versions compile across the workspace.
- QUIC dev endpoint scaffolding compiles against the current `quinn` and `rcgen` APIs.
- Crypto helper code compiles with the current AEAD trait imports.
- The full workspace test suite was brought back to a passing state during the current implementation pass.

## Partially Implemented Or Present As Scaffolding

- CRDT data structures exist, but the full local namespace state machine is not yet wired into actual filesystem apply logic.
- QUIC transport setup exists, but full replication behavior is not implemented.
- Transparent proof structures exist, but proof generation and enforcement are not yet end-to-end.
- Crypto helpers exist for signing, AEAD, and envelopes, but at-rest encryption is not fully integrated into the core write path.
- S3, POSIX/FUSE, privacy, energy, and ZK crates are present, but most remain stubs or placeholder layers.

## Backlog

### Highest-Priority Backlog

- Replace `apply_op_minimal` with full CRDT-backed directory and inode mutation logic.
- Persist real mutable namespace state instead of only appending ops and rotating heads.
- Build snapshots from actual inode and directory state, not only a reserved root placeholder.
- Add richer admin endpoints for storage stats, applied state, and local repository insight.

### Replication Backlog

- Implement the actual peer replication manager.
- Complete Hello and feature negotiation end to end.
- Implement Have/WantOps/OpsBatch synchronization.
- Implement WantBlobs/BlobsBatch transfer and remote blob fulfillment.
- Apply remote ops and blobs into the same verified local state machine.

### Security and Verification Backlog

- Integrate chunk encryption into the live storage path.
- Add key-envelope handling to real read and write flows.
- Attach transparent proof bundles to new operations automatically.
- Enforce proof verification on receipt where enabled.
- Improve trust management beyond development-style bootstrap behavior.

### Product Surface Backlog

- Expand the admin UI beyond the current minimal status panel.
- Implement the S3-like facade beyond placeholder routes.
- Implement the POSIX/FUSE facade beyond placeholder errors.
- Add operational tooling such as verification, migration, and maintenance commands.

### Systems Backlog

- Integrate energy telemetry and the scheduler into real background work decisions.
- Add compaction, cleanup, and storage accounting.
- Support pending operations when causal dependencies or blobs are missing.
- Add broader integration tests between daemon instances.

## Practical Reading Of The Current Milestone

NexusFS currently fits this description:

- M0 is complete.
- M1 is in progress, with meaningful local-core foundations implemented.
- M2 and beyond are still backlog, except for crate scaffolding and interface placeholders.

## Recommended Next Step

The most valuable next implementation step is to complete the real local filesystem state machine in `crates/core` so that operations mutate persistent directory and inode state, not only the oplog and head pointer.
