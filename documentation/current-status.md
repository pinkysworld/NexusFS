# Current Status

Last updated: August 2, 2026

This page summarizes what NexusFS currently implements in the repository and what remains in the backlog.

## Overall State

**Milestones M0 through M3 are complete.** NexusFS is a working distributed filesystem:
files round-trip through a signed operation log applied to CRDT-backed namespace state,
an S3-compatible API exposes that state over HTTP, and two nodes converge over QUIC with
every operation and chunk verified before it is accepted.

Not yet implemented: encryption at rest, proof generation and enforcement, the
POSIX/FUSE facade, energy-aware scheduling, and ZK proofs.

## Implemented Now

### Repository and Build Baseline

- The Rust workspace builds as a cohesive multi-crate project on current stable Rust.
- CI runs `cargo fmt`, `cargo clippy -D warnings` and `cargo test --workspace` on every
  push and pull request.
- The repository includes public-facing docs in `documentation/` and a static project
  website in `site/`.

### Local Configuration and Identity

- TOML config loading is implemented for node, admin, network, security, and energy settings.
- `node.data_dir` expands a leading `~`, so the store can live outside a synced folder.
- The daemon creates and reuses a persistent device identity.
- The daemon creates and reuses an admin token when one is not supplied in config.

### Storage Layer

- `BlobStore` and `KvStore` traits are present, including blob count/size accounting.
- The sled backend is implemented and tested for blob put/get/has/delete and KV prefix scans.
- An in-memory backend is always available, which is what lets the identical core run in
  the browser.

### Filesystem Core

- Canonical object encoding using `postcard`, with directory entries normalized before hashing.
- BLAKE3 hashing for raw bytes and content-addressed object storage.
- Fixed-size chunking; chunk references carry hash, byte length and byte offset.
- **Persistent namespace state**: each directory is an observed-remove map of
  `name -> entry`, and each inode is a record with LWW registers for content and attributes.
- **Deterministic inode allocation**: an inode id is derived from the allocating operation's
  id, so every replica names the same inode without coordination.
- **Real operation apply** for `Mkdir`, `CreateFile`, `Write`, `Rename`, `Unlink` and
  `SetAttr`, replacing the former placeholder that only rotated the head.
- **Signature enforcement**: every operation is verified before it can change state.
  Unsigned and tampered operations are rejected.
- **Pending operations**: an operation whose preconditions are not yet satisfiable is
  parked and retried automatically when later operations arrive, rather than failing.
  This covers both missing state dependencies (a child arriving before its parent) and
  missing content — a write whose chunks have not been fetched parks rather than
  publishing a file that would read back as an error. `retry_pending` re-drives the
  queue when blobs arrive without an accompanying operation.
- **Deterministic conflict naming** applied in live directory reads, not only as a helper.
- **Read path**: path resolution, directory listing and whole-file reassembly from chunks.
- **Snapshots built from live state**: directories are materialized into canonical
  `DirNode` objects and committed alongside an inode-map root, so the state commitment
  moves whenever structure or content changes.

### Admin Surface

- The embedded admin server is wired into the daemon and serves a browsable UI.
- The API exposes:
  - `/api/status` (head, state root, op and pending counts)
  - `/api/fs/head`
  - `/api/fs/ls?path=`
  - `/api/oplog/summary`
  - `/api/oplog/recent?limit=`
  - `/api/storage/stats`

### CLI

`nexusfs` supports `daemon`, `status`, `mkdir [-p]`, `put`, `cat`, `ls`, `rm` and `mv`.
Every mutating verb builds a signed operation and applies it through the same pipeline
replication uses.

### S3-Compatible Facade

`crates/s3` implements object PUT/GET/HEAD/DELETE, bucket create and list, and
ListObjectsV2 with prefix, delimiter and continuation-token pagination. A bucket is a
top-level directory and an object key is the path beneath it, so S3's flat keyspace
maps onto the real tree without a separate index.

Writes go through the same `write_file` path the CLI uses, which means an object
written over HTTP is an ordinary file: the CLI can `cat` it, the admin API lists it,
and the oplog shows the signed `CreateFile`/`Write` operations that produced it. The
facade has no way to reach past the operation log.

Deliberately not implemented: SigV4 request signing, multipart upload, versioning,
ACLs, CORS and lifecycle rules. Authentication is an optional shared secret in
`x-nexusfs-token`, so the facade belongs on loopback or another trusted interface.
ETags are BLAKE3 rather than MD5, which clients that recompute them to verify uploads
will notice.

### Networked Replication

Two nodes converge over QUIC. The session is pull-based and one-directional — a node
asks a peer for what it lacks and nothing is pushed — so each session has one owner of
the loop and no negotiation about who sends next. Convergence comes from each node
pulling from the other.

Operations transfer before content: a `ClockSummary` diff selects the operations a peer
lacks, and only then does the puller ask for the chunks those operations turned out to
reference. Writes whose content has not arrived park and apply automatically once it
does.

Verification is not optional anywhere on this path. Operation signatures are checked by
the same `apply_op` local writes use, and chunk hashes are recomputed before content is
stored, so a peer cannot substitute bytes for a hash that was requested.

Peer identity is an ed25519 key pinned on first use, independent of TLS. A device
presenting a different key than the one pinned is refused whatever the policy says.
`/api/peers` reports each configured peer's last attempt, last success, error and
transfer counts.

Not implemented: push notification of new operations (peers poll on an interval),
delta-encoded operation ranges, and bandwidth or energy-aware scheduling.

### Browser Playground

`crates/wasm` compiles the core to `wasm32-unknown-unknown` against the in-memory
storage backend, which the project website loads to run two replicas in one page. It
exercises the real apply pipeline, so convergence and conflict naming shown there are
genuine rather than simulated. The module has no JS imports, so it builds with plain
cargo and needs no wasm-bindgen toolchain; the Pages workflow builds it on deploy
rather than serving a committed binary.

Note that the playground's "sync" hands one replica's oplog and blobs to the other
in-process. It exercises the same apply pipeline, but it is not the QUIC protocol the
daemon uses between real nodes.

### Test Coverage

55 tests, including order-independent convergence (the same operation set applied in
different orders yields an identical state root), idempotent re-apply, pending-op drain,
concurrent-create conflict naming, concurrent-write resolution, rename-vs-unlink,
subtree-cycle refusal, restart persistence, S3 key mapping and pagination, and
replication over both an in-memory pipe and real QUIC sockets — covering unknown-peer
refusal, key-rotation refusal, forged-operation rejection and corrupted-content
rejection.

## Partially Implemented Or Present As Scaffolding

- Transparent proof structures exist, but proofs are not generated or enforced.
- Crypto helpers exist for signing, AEAD, and envelopes. Operation signing is live;
  at-rest encryption is not integrated into the write path, and `Envelope::open` is
  unimplemented.
- POSIX/FUSE, privacy, energy and ZK crates are present but remain stubs.

## Backlog

### Highest-Priority Backlog

- Integrate chunk encryption into the live storage path (M4).
- Attach and enforce transparent proof bundles (M4).
- Replace polling with push notification of new operations.

### Security and Verification Backlog

- Integrate chunk encryption into the live storage path.
- Implement `Envelope::open` and add key-envelope handling to real read and write flows.
- Attach transparent proof bundles to new operations automatically.
- Enforce proof verification on receipt where enabled.
- Improve trust management beyond development-style bootstrap behavior.

### Product Surface Backlog

- Implement the S3-like facade on top of the existing state machine.
- Implement the POSIX/FUSE facade.
- Add operational tooling such as verification, migration, and maintenance commands.

### Systems Backlog

- Batch storage writes: the sled backend currently flushes on every put, costing an
  fsync per chunk.
- Cache directory maps rather than re-reading and re-materializing per path component.
- Integrate energy telemetry and the scheduler into real background work decisions.
- Add compaction, cleanup, and garbage collection for unreferenced inodes and blobs.
- Add broader integration tests between daemon instances.

## Practical Reading Of The Current Milestone

- M0 is complete.
- M1 is complete.
- M2 is complete via the S3 facade; the POSIX/FUSE alternative remains unimplemented.
- M3 is complete: two nodes converge over QUIC with verified remote apply.
- M4 and beyond are backlog, except for crate scaffolding and interface placeholders.

## Recommended Next Step

Bring up replication between two nodes. The local apply pipeline is now the single
source of truth for state changes, so a remote operation only has to be delivered and
handed to the same `apply_op` — verification, conflict resolution and pending handling
already work.
