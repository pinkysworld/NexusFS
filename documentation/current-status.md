# Current Status

Last updated: August 5, 2026

This page summarizes what NexusFS currently implements in the repository and what remains in the backlog.

## Overall State

**Milestones M0 through M5 are complete.** NexusFS is a working distributed filesystem:
files round-trip through a signed operation log applied to CRDT-backed namespace state,
an S3-compatible API exposes that state over HTTP, two nodes converge over QUIC with
every operation and chunk verified before it is accepted, content can be encrypted at
rest while still replicating, and replication adapts what it transfers to the device's
power, thermal and link situation.

Not yet implemented: the POSIX/FUSE facade and ZK proofs.

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
  - `/api/peers`
  - `/api/security` (encryption state, proof coverage, audit result)

### CLI

`nexusfs` supports `daemon`, `status`, `verify`, `mkdir [-p]`, `put`, `cat`, `ls`, `rm`
and `mv`.
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

Each pass is bounded by a budget the energy scheduler supplies (see below), which can
skip content entirely or stop at a byte ceiling and defer the rest to a later pass.

Not implemented: push notification of new operations (peers poll on an interval),
delta-encoded operation ranges, and prioritising *which* deferred content to fetch first
when the budget is capped.

### Encryption At Rest

Chunk content is encrypted with XChaCha20-Poly1305 before it is written, when
`security.encrypt_at_rest` is on. Each write mints a fresh file key; that key is sealed
with a repository key stored beside the device identity and travels inside the
`FileNode`, so content needs no side channel to be readable by a replica holding the
same repository key.

Chunks stay addressed by the hash of the bytes *as stored* — the ciphertext. That is
what keeps replication verifiable: a peer checks `hash(received) == requested` before
storing anything, and must be able to do so without holding any key. A peer with the
wrong repository key still converges on structure and still verifies transfers; it
simply cannot read the content.

The cost of this choice is that identical plaintext under different file keys does not
deduplicate. Convergent encryption would recover that at the price of letting anyone
holding a candidate file confirm whether a node stores it, so it is not used.

Whether a file is encrypted is recorded on the file, not on the node, so enabling
encryption does not strand content written before it was switched on.

Limitations worth stating plainly: replicas share one repository key, so this protects
the disk and the wire, not one peer from another. Per-recipient key distribution is what
`crypto::envelope` is for — now implemented, including `open`, but not yet wired into
the write path. File names, directory structure and file sizes are not encrypted.

### Transparent Proofs

With `security.proof_mode = "transparent"`, every locally created operation carries a
bundle recording the state root before it, the state root after it, and the object
hashes it introduced. The signature covers the bundle, so an author cannot later claim
a different transition.

On receipt, a malformed or mislabelled bundle is rejected deterministically — malformed
evidence is worse than none. A well-formed bundle whose `old_root` the receiver cannot
corroborate is accepted rather than refused, because operations legitimately arrive
before the state they build on. Setting `proof_mode = "required"` additionally refuses
operations that carry no proof at all.

`nexusfs verify` audits a repository: every signature, every proof's structure, and a
read of every file, which exercises chunk presence, ordering and — when encrypted —
authentication. It exits non-zero on failure, so it works as a cron or CI check. The
same report is available at `/api/security`.

These proofs are auditable evidence, not zero-knowledge and not a proof of correctness.
Establishing that a transition was correct means replaying it, which `verify` does
locally. `zk_commit` and `zk_full` remain unimplemented and are treated as `none`
rather than silently pretending to prove anything.

### Energy-Aware Scheduling

`crates/energy` samples the device before each sync pass — power source, battery charge,
temperature, one-minute load, link cost — and a rule-based scheduler turns that reading
into a budget: whether to contact peers at all, whether to transfer content or only
operations, a byte ceiling for the pass, and a multiplier on the poll interval. Sampling
shells out to `pmset` and `sysctl` on macOS and reads `/sys/class/power_supply` and
`/sys/class/thermal` on Linux; other platforms report unknown on every field.

The policy is built on the asymmetry between an operation and the content it names. An
operation is a few hundred bytes; the content can be megabytes. So the graded response is
to keep the namespace converged and defer the bytes — a device that has taken every
operation but no content still knows what exists, where, and at what version, and can
fetch any particular file on demand once power returns. The ladder runs full sync →
capped content → operations only → nothing, and only the last rung stops tracking the
filesystem at all.

Heat and metered links override the battery grade rather than being folded into it. No
amount of remaining charge makes sustained transfer on a hot device acceptable, and a
metered link costs money per byte regardless of power.

Every reading is a three-state enum rather than a boolean, and unknown always means
unconstrained. A server with no battery sensor is not a device at 0% charge; conflating
the two would make an unconstrained machine throttle itself permanently.

The decision is observable at `/api/energy`, which reports the reading, the resulting
budget, and a sentence explaining which rule fired. The daemon samples once per pass and
caches it, so the console explains the pass that actually ran rather than a fresh reading
that could contradict it.

Set `energy.enabled = false` to remove every limit while keeping the reading visible.

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

85 tests, including order-independent convergence (the same operation set applied in
different orders yields an identical state root), idempotent re-apply, pending-op drain,
concurrent-create conflict naming, concurrent-write resolution, rename-vs-unlink,
subtree-cycle refusal, restart persistence, S3 key mapping and pagination, and
replication over both an in-memory pipe and real QUIC sockets — covering unknown-peer
refusal, key-rotation refusal, forged-operation rejection and corrupted-content
rejection, encrypted round-trips, absence of plaintext on disk, wrong-key and
tampered-ciphertext rejection, replication of encrypted content to peers with and
without the repository key, the scheduler's decision table across power, charge, heat and
link cost, and replication actually honouring a metadata-only budget and a byte cap —
including that a deferred transfer completes on a later unconstrained pass.

## Partially Implemented Or Present As Scaffolding

- `crypto::envelope` now seals *and* opens, but is not yet used by the write path.
- Link cost is always reported as unknown: no platform metered-connection detection is
  implemented yet, so a metered link must currently be simulated in tests rather than
  detected in the field.
- POSIX/FUSE, privacy and ZK crates are present but remain stubs.

## Backlog

### Highest-Priority Backlog

- Detect metered links per platform, so the scheduler's link rule can fire in the field.
- Replace polling with push notification of new operations.
- Per-recipient key envelopes, so replicas need not share one repository key.
- Fetch deferred content on demand at read time, so a metadata-only node can serve a file
  the moment it is actually asked for.

### Security and Verification Backlog

- Add key-envelope handling to real read and write flows, so peers need not share a key.
- Improve trust management beyond development-style bootstrap behavior.
- Commitment-oriented proof systems (M7), replacing transparent bundles where useful.

### Product Surface Backlog

- Implement the POSIX/FUSE facade.
- Add operational tooling such as migration and maintenance commands.

### Systems Backlog

- Batch storage writes: the sled backend currently flushes on every put, costing an
  fsync per chunk.
- Cache directory maps rather than re-reading and re-materializing per path component.
- Add compaction, cleanup, and garbage collection for unreferenced inodes and blobs.
- Add broader integration tests between daemon instances.

## Practical Reading Of The Current Milestone

- M0 is complete.
- M1 is complete.
- M2 is complete via the S3 facade; the POSIX/FUSE alternative remains unimplemented.
- M3 is complete: two nodes converge over QUIC with verified remote apply.
- M4 is complete: encryption at rest and transparent proofs.
- M5 is complete: telemetry, a rule-based scheduler, and replication that honours it.
- M6 and beyond are backlog, except for crate scaffolding and interface placeholders.

## Recommended Next Step

Close the loop the energy scheduler opened: fetch deferred content on demand at read
time. A node running metadata-only already knows a file exists and which chunks it needs;
today a read of that file fails until the next unconstrained sync pass happens to bring
the bytes. Pulling the missing chunks when someone actually opens the file is what turns
"deferred" from a gap into a policy, and it reuses the blob phase of the existing
session protocol rather than needing new wire format.
