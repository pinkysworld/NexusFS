# Architecture Overview

NexusFS is built as a set of Rust crates that preserve clean boundaries between storage, protocol, verification, and optional product surfaces.

## Core Design Goals

- Single executable deployment
- Offline-first synchronization
- Verifier-first replication
- Energy-aware background behavior
- Feature flags for constrained environments

## Workspace Layout

- `crates/nexusfs`: binary entrypoint, CLI, daemon wiring
- `crates/core`: object model, canonical encoding, chunking, local state
- `crates/storage`: blob store and KV abstractions with backend implementations
- `crates/proto`: shared operation and message types
- `crates/crdt`: deterministic merge structures for mutable namespace state
- `crates/net`: peer transport and replication protocol
- `crates/crypto`: signing, encryption, and key envelope helpers
- `crates/admin`: embedded admin HTTP surface and static assets
- `crates/energy`: telemetry sampling and scheduling decisions
- `crates/privacy`: padding and cover-traffic scaffolding
- `crates/zk`: transparent proof bundles and future ZK modes
- `crates/s3`: S3-like API facade
- `crates/fs_posix`: POSIX/FUSE facade

## Architectural Invariants

1. Objects are encoded canonically before hashing.
2. Content-addressed objects are immutable.
3. Every mutation is represented as a signed filesystem operation.
4. Replication must be safe to replay.
5. Remote state is accepted only after verification.
6. Policy and privacy modes must be explicit and versioned.

## Data Model

NexusFS separates immutable content from mutable references:

- blobs live in the content-addressed store
- file and directory objects commit to chunk layout and namespace state
- snapshots commit to a filesystem view
- mutable heads and oplog indexes live in the KV layer

## Why This Shape?

The architecture is deliberately split so the local storage engine can mature independently from transport, while the transport can reuse the same canonical state machine and proof hooks.

For the full design rationale, use `../docs/architecture.md`.
