# Current Status

Last updated: August 11, 2026

This page summarizes what NexusFS currently implements in the repository and what remains in the backlog.

## Overall State

**Milestones M0 through M8 are complete**, with M8's research tracks split between what could be built and what is named as open. NexusFS is a working distributed filesystem:
files round-trip through a signed operation log applied to CRDT-backed namespace state,
an S3-compatible API exposes that state over HTTP, two nodes converge over QUIC with
every operation and chunk verified before it is accepted, content can be encrypted at
rest while still replicating, and replication adapts what it transfers to the device's
power, thermal and link situation. Operators can reclaim storage, upgrade across on-disk
formats, and enrol peers without relying on trust-on-first-use. The state root is a
Merkle commitment, so any single entry can be proved to a party holding no filesystem —
including proving an entry is *absent*, which makes a deletion demonstrable.

Not yet implemented: the POSIX/FUSE facade, and zero-knowledge proving — the commitment
layer is deliberately not that, for reasons recorded below.

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

### Operator Tooling

**Garbage collection.** Mark-and-sweep from two roots: live namespace state, and the
references held by operations still parked waiting for content. Superseded file
versions, unlinked content and historical snapshot objects are collectable; anything a
pending operation needs is not.

The dominant source of garbage is not overwrites. Every applied operation rebuilds the
snapshot, which orphans the previous `SnapshotRoot` and one `DirNode` per directory on
the path to the change — so a repository accrues garbage per *operation*, and even an
append-only history has objects to reclaim.

`nexusfs gc` surveys by default and deletes only with `--apply`. It refuses outright
when the repository has no head or no root inode record: those indicate corruption or a
half-finished restore, and marking cannot distinguish "everything is garbage" from
"marking failed". `/api/storage/gc` reports the same survey and never deletes, because
the daemon may write between the mark and the sweep.

This is only safe because a superseded write no longer waits for its content. Previously
an overwritten version parked forever, which meant a node kept asking peers for bytes no
reader would see — and a peer that had collected them could never answer.

**Format versioning.** The store records the on-disk format it was written with.
`postcard` carries no field names or type tags, so a decoder handed bytes from a
different schema does not reliably fail; it can succeed and produce nonsense. The stamp
converts that into a refusal.

An older format refuses to open and names `nexusfs migrate` as the fix. A newer one
refuses and cannot be forced. Opening never migrates by itself: a migration rewrites
records in place, and the operator may have no backup or be mid-rollout. The migration
machinery is in place; v1 is the first format, so no step is implemented yet.

**Peer enrolment.** The pinned-key store lives in `core`, not in the networking crate,
so keys can be managed on a build without QUIC and — more to the point — before any
connection is attempted. `nexusfs peer identity` prints what to enrol elsewhere;
`peer add`, `peer list` and `peer remove` manage the pinned set. Replacing an existing,
different key requires `--rotate`, because a silent overwrite would erase the one signal
distinguishing a planned rotation from an impersonation attempt. Two nodes with
`net.tofu = false` converge on pre-enrolled keys alone.

### State Commitments And Inclusion Proofs

The state root used to be a single BLAKE3 over the sorted inode map. It said whether two
replicas agreed and nothing else: convincing anyone of one fact meant handing them the
whole state. It is now a Merkle root over the same leaves, so a single entry can be
proved with the root, the inode, its object hash and O(log n) sibling hashes.

`nexusfs prove <path>` emits a self-contained proof; `nexusfs check-proof` verifies one
without opening a repository at all. Without an explicit `--root` it reports that it
only established internal consistency — a proof checked against the root recorded inside
itself says nothing about whether that root is one anyone else agrees with.
`/api/fs/proof?path=` serves the same structure.

Setting `security.proof_mode = "zk_commit"` attaches an inclusion path for the entry each
operation is about, and a receiver checks it against the root the operation claims. That
is strictly more than a transparent proof offers: a transparent proof can only be judged
by someone who already holds the author's prior state. Transparent bundles are still
accepted under commit policy — they prove less but are not wrong, and refusing them would
make the mode unusable mid-rollout. An operation whose subject is not in the live tree
falls back to a transparent bundle rather than proving some other entry.

Three Merkle details that are easy to get wrong, and are covered by tests: leaves and
interior nodes carry distinct tag bytes, so an interior node's preimage cannot be passed
off as a leaf; a lone trailing node is promoted rather than hashed with a copy of itself,
which would let two different leaf sets share a root; and path length is bounded so a
hostile proof cannot cost unbounded work to reject.

**This is a commitment scheme, not zero-knowledge.** A verifier learns the inode being
proved and its object hash; what it does not learn is the rest of the tree. The mode is
named `ZkCommit` because an inclusion path is exactly the witness a SNARK circuit would
consume — the commitment half of the job. `zk_full` remains unimplemented and behaves as
`none`. Proving *absence* is also unbuilt.

Because the commitment *is* the state root, it is not compile-time optional: a build
computing a different root could never replicate with one that did not. Changing it
therefore came with on-disk format v2 and PROTOCOL_VERSION 2, so a stale store is
refused until migrated and a mismatched peer refuses the handshake rather than syncing
and then disagreeing forever.

### Absence Proofs And Batching

Absence needs a different construction from inclusion, because a path that resolves to
nothing has no inode to name — so it is asked by inode. The leaves are sorted, which
makes absence the claim that two *adjacent* entries straddle the inode: there is nowhere
else it could be.

Adjacency is what the proof rests on. Two entries that merely bracket the inode prove
nothing, because the inode could be one of the entries between them. Each neighbour
therefore states its index, and that index is checked against the shape its path must
have in a map of that size — a property that holds because a path shape identifies
exactly one position, which is itself covered by a test.

Pairing directions is the point: an inclusion proof against an old root plus an absence
proof against a new one demonstrates a deletion to someone holding neither state.

`prove_many` answers for several entries from one traversal. This is convenience rather
than compression — the paths still travel in full — but the prover stops rebuilding the
tree per entry, which is the cost that hurts when answering for a whole directory.

### Storage Durability

Writes are no longer individually durable. An operation touches about a dozen keys, and
syncing each one bought a guarantee nobody wants: that *half* an operation survives a
crash. Durability now sits at the operation boundary, where losing a whole operation is
the failure — and it is the one the design already tolerates, since applying is
idempotent and a peer still holds it.

Measured on this machine: 58.7ms to 6.2ms per operation at 250 entries, 62.8ms to 9.3ms
at 1000. Paths that do not end in an applied operation — peer enrolment, the format
stamp, collection, bootstrap — carry their own flush rather than relying on the database
being dropped cleanly, which a killed process never does.

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

161 tests, including order-independent convergence (the same operation set applied in
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

M6 adds collection safety (what survives, what the sweep refuses to do, that collecting
twice is stable and that it does not hide pre-existing damage), format refusals in both
directions, enrolment and rotation, and failure modes: missing chunks, content that no
longer matches its hash, forged operations, an unspliceable partial write, and a lost
head rebuilt from live state.

M7 adds the Merkle commitment (determinism, every-entry inclusion at every tree size,
tag confusion, trailing-node malleability, tampered values, siblings and sides, and path
length bounds) and the commitment proof mode end to end, including verification by a
party holding no repository. M8 adds absence proofs — every gap at every tree size,
plus the forgeries they must refuse: non-adjacent neighbours, a neighbour that does not
bracket, a middle entry dressed up as the last, and a neighbour claiming an index its
path shape does not support.

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
- Collect orphaned inode and directory records, not only blobs: collection reclaims
  content but leaves the KV entries of unlinked inodes behind.
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
- M6 is complete: collection, format versioning, peer enrolment and failure-mode cover.
- M7 is complete as a commitment layer; the proving-system half is deliberately not
  built, and the roadmap records why.
- M8 is complete for the tracks that were engineering — proof batching, absence proofs,
  and the storage durability work. Privacy layers, delay-tolerant replication and policy
  systems are named as open rather than left implied.

## Deliberately Not Built

- **A proving system.** Adopting one means a proving backend, a setup ceremony, and
  arithmetizing the hash — BLAKE3 is hostile to circuits, so a real implementation would
  move the commitment to something like Poseidon and change the state root again. That
  is its own milestone-sized risk, and a stub resembling a circuit would be worse than
  an honest commitment layer.
- **Absence proofs.** The sorted leaf layout supports proving the two entries that
  bracket a gap; nothing needs it yet.
- **Persisted telemetry snapshots** (M5). A power reading from before a restart
  describes a machine that may since have been unplugged.

### The Maintained Inode Map

The state root commits to a flat map of inode to object hash. That map used to be
rebuilt by walking the whole tree on every applied operation — materializing, encoding
and hashing every directory to enumerate what was reachable, work proportional to the
filesystem rather than to the change.

It is now maintained. A directory's hash depends only on its own entries and attributes,
not on its children, so changing one inode changes exactly one map entry. Creates and
writes patch the entries they touch; removals and renames fall back to a full walk,
because they can change *which* inodes are reachable and there is no cheap bound on how
many.

Reachability is the part that has to be exact, and it is answered two ways: a reachable
directory always carries a map entry, so membership settles it; a file may legitimately
have none — it has no content yet — so it is answered through the parent recorded when
it was created, confirming the parent still lists it. An operation applied inside a
directory that has since been unlinked therefore adds nothing.

Measured at a thousand entries: the state root fell from 3.66ms to 1.04ms. End-to-end an
apply improved about 6%, because an apply is now dominated by its single fsync rather
than by computation. The remaining cost is still linear — the Merkle tree is rebuilt from
the map each time — so an incremental tree, updating one leaf in O(log n), is the next
layer rather than something this change delivered.

Correctness is pinned by an invariant suite that re-derives the map with a full walk
after every kind of operation and demands equality, since a wrong entry is not a stale
cache but two replicas disagreeing about what the filesystem is.

## Recommended Next Step (updated)

An incremental Merkle tree. The map is now maintained rather than walked, but the
commitment over it is still rebuilt per apply — linear in the filesystem for a change
that touched one entry. Updating a single leaf in O(log n) is the remaining structural
win.

Worth weighing against it: an apply is currently fsync-bound, so the end-to-end gain
would be small until something else changes. Closing the loop the energy scheduler
opened — fetching deferred content on demand at read time — buys more for users today. A node running metadata-only already knows a file exists and which chunks it needs;
today a read of that file fails until the next unconstrained sync pass happens to bring
the bytes. Pulling the missing chunks when someone actually opens the file is what turns
"deferred" from a gap into a policy, and it reuses the blob phase of the existing
session protocol rather than needing new wire format.

After that, M7 is the first commitment-oriented proof. It should stay feature-gated and
fall back to transparent proofs, so the practical baseline never depends on it.
