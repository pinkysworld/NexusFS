# Current Status

Last updated: August 21, 2026

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

A node that skipped content to save power fetches it on demand when someone reads,
rather than waiting for the next unconstrained pass.

Not yet implemented: the POSIX/FUSE facade, and zero-knowledge proving — the commitment
layer is deliberately not that, for reasons recorded below. 231 tests pass, clippy is
clean, and every feature combination of the binary is built in CI.

## Implemented Now

### Repository and Build Baseline

- The Rust workspace builds as a cohesive multi-crate project on current stable Rust.
- CI runs `cargo fmt`, `cargo clippy -D warnings` and `cargo test --workspace` on every
  push and pull request, builds the wasm target and asserts the module still has no JS
  imports, and checks every feature combination of the binary rather than the two that
  happen to be used — a `cfg` on a type and a `cfg` on its construction site can drift
  apart, and only a build of that exact combination notices.
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
  - `/api/status` (head, state root, op and pending counts, format version)
  - `/api/fs/head`, `/api/fs/ls?path=`, `/api/fs/cat?path=`
  - `/api/fs/proof?path=` (inclusion proof for one entry)
  - `/api/oplog/summary`, `/api/oplog/recent?limit=`
  - `/api/storage/stats`, `/api/storage/gc` (survey only, never deletes)
  - `/api/peers` (sync state per configured peer), `/api/peers/enrolled` (pinned keys)
  - `/api/identity` (this node's device id and public key)
  - `/api/energy` (the reading, the budget, and the rule that fired)
  - `/api/security` (encryption state, proof coverage, audit result)

### CLI

`nexusfs` supports `daemon` and `status`; the filesystem verbs `mkdir [-p]`, `put`,
`cat`, `ls`, `rm` and `mv`; the operator verbs `verify`, `gc`, `migrate` and
`peer identity|list|add|remove`; and the proof verbs `prove` and `check-proof`.
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

Replicas no longer share one key. A write seals its file key to each enrolled peer and
to this device, using X25519 to an ephemeral key and XChaCha20-Poly1305 under the shared
secret — so a replica holding the ciphertext, every operation, and the old repository key
still cannot read a file it is not a recipient of. That is the difference between
"encrypted at rest" and "one peer protected from another".

Each device therefore has two keys: an ed25519 signing key and an X25519 sealing key.
They are independent secrets rather than one mapped into the other's curve — the
birational map is standard and would mean one key to enrol instead of two, but it puts a
signing oracle and a Diffie-Hellman oracle over the same secret scalar, and this is not
the codebase where that should first be got right. Both are enrolled together:
`nexusfs peer identity` prints both, `peer add` takes both, and a peer enrolled without a
sealing key replicates and verifies normally while simply not being a recipient.

Envelopes carry no recipient identity. A reader trials its own key against each, which
costs one exchange and one AEAD open per entry, and in exchange a file does not publish
the list of devices able to read it. What a file *does* carry is a digest of its
recipient set keyed by the file key — needed so re-sealing can tell whether a file is
already addressed to the current peers, and keyed so that only someone who can read the
file can test a candidate set against it.

`nexusfs share` re-seals existing files to the peers enrolled now, since enrolment only
affects what is written afterwards. It grants access and never withdraws it: the
ciphertext does not change, so anyone who already held a key still holds one.

`nexusfs rotate` is the other half, and what removing a peer actually needs. It mints a
fresh file key per file, re-encrypts the content under it, and seals that to the
recipients enrolled now — so every chunk changes, every hash naming it changes, and the
old ciphertext becomes garbage `gc` reclaims. `--path` limits it to one file, for a
suspect key rather than a departing peer.

What rotation withdraws is access to the content *from here on*. A device that copied the
old ciphertext and kept an envelope for it can still read that version for as long as it
keeps the bytes; nothing can withdraw what somebody already took. That sentence is
printed on every run, survey included, because an operator who believes otherwise will
make a decision they would not otherwise make.

The two are separate commands because the costs are not comparable: re-sealing rewrites a
few hundred bytes of envelope per file, rotation reads and rewrites every byte.

The repository key is still read, so files written before this keep working, and it is
still written by a node that has no sealing key at all. Nothing else produces one.

**What to back up changed.** With per-recipient sealing, `identity.toml` is what opens
your content — not `repo.key`. Both live in the data directory; the identity file is now
the one that matters, and it is written owner-only.

Still not encrypted: file names, directory structure and file sizes.

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
locally. `zk_commit` is implemented and described below; it is a commitment scheme
rather than a proving system. `zk_full` remains unimplemented and is treated as `none`
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

Link cost is now reported rather than permanently unknown, so that override can fire in
the field. Detection is partial by platform and says so: Linux asks NetworkManager for
the default-route interface and gets a real answer both ways, including NM's cellular
guess; macOS recognises a phone tethered over USB and nothing else, because a Wi-Fi
hotspot is indistinguishable from home broadband and inferring it from the network's
name would be a heuristic dressed up as a reading; other platforms report unknown. A VPN
defeats detection everywhere, since the default route names the tunnel rather than the
link underneath it.

Because that leaves real deployments uncovered, `energy.link_cost` states the answer
outright — `auto`, `metered`, `unmetered` or `unknown` — and skips detection when set.
An operator on a satellite uplink should not wait for a probe to be written for their
platform. An unreadable source always yields unknown, never unmetered: the scheduler
treats the two identically, but only one is a fact.

Heat and metered links override the battery grade rather than being folded into it. No
amount of remaining charge makes sustained transfer on a hot device acceptable, and a
metered link costs money per byte regardless of power.

Free space is a different kind of reading again, and is applied differently. The others
are tradeoffs the ladder weighs; disk is a wall, because bytes cannot be stored where
there is no room. It therefore narrows whatever the ladder decided and never widens it.
`energy.storage_reserve_mb` (default 1024) is a floor rather than a threshold —
replication is a background job filling someone else's disk, and the last gigabyte
belongs to whatever the machine is actually for. Content is held to the room above the
reserve and stops entirely at it, while operations keep flowing.

It is also the only reading about a *path* rather than a machine: a node with its store
on an external volume cares about that volume, and `df` is asked about the data
directory rather than the root. An unreadable answer is unknown and constrains nothing.

The cap is always explained, even when it is generous. An earlier draft stayed silent
when the room was ample, on the grounds that a 216GB ceiling binds no real pass — which
produced a budget carrying a 216GB cap while reporting "no constraints apply". A number
disagreeing with its own explanation is how a console stops being trusted.

Every reading is a three-state enum rather than a boolean, and unknown always means
unconstrained. A server with no battery sensor is not a device at 0% charge; conflating
the two would make an unconstrained machine throttle itself permanently.

The decision is observable at `/api/energy`, which reports the reading, the resulting
budget, and a sentence explaining which rule fired. The daemon samples once per pass and
caches it, so the console explains the pass that actually ran rather than a fresh reading
that could contradict it.

Set `energy.enabled = false` to remove every limit while keeping the reading visible.

`nexusfs status` prints the same reading and budget. That matters most on the build
least likely to have the admin feature compiled in — a constrained device, which is also
the one most likely to be throttling — and it reports even when scheduling is switched
off, because "nothing is throttling this" and "throttling is disabled" are different
answers.

### Operator Tooling

**Garbage collection.** Mark-and-sweep from two roots: live namespace state, and the
references held by operations still parked waiting for content. Superseded file
versions, unlinked content and historical snapshot objects are collectable; anything a
pending operation needs is not.

The dominant source of garbage is not overwrites. Every applied operation rebuilds the
snapshot, which orphans the previous `SnapshotRoot` and one `DirNode` per directory on
the path to the change — so a repository accrues garbage per *operation*, and even an
append-only history has objects to reclaim.

Records are collected as well as blobs. Unlinking a file removes the entry from its
parent's map and used to leave the file's own records — its inode record, its parent
pointer, and for a directory its entry map — behind forever, unreferenced and invisible.
They are swept from the same reachability walk but counted separately in the report,
because the failure modes are not symmetric: a wrongly deleted blob can be fetched from
a peer again, and a wrongly deleted record cannot, since the operation that produced it
is already marked applied.

Reachability for records is its own walk rather than a reuse of the object walk, and the
difference is the case that matters: a file created but not yet written has no
`FileNode`, so it appears in no inode map — and its record must survive, or the write
still in flight would apply into nothing. A parked operation's inodes are roots for the
same reason.

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
records in place, and the operator may have no backup or be mid-rollout. The current
format is v3, and both steps are real. v1 to v2 moved the state root to a Merkle
commitment, so that migration re-snapshots from live state rather than reinterpreting
bytes. v2 to v3 changed the shape of a file's encryption record, which cannot be
rewritten at all — a `FileNode` is named by its own hash and a `Write` is signed — so it
checks and either carries the repository forward or refuses with the reason. It can carry
most repositories forward because postcard writes `None` as a single zero byte whatever
it wraps, which makes every *plaintext* record byte-identical across the two formats.

A brand-new store is stamped current rather than migrated, which is why "no head" and
"pre-versioning" are distinguished instead of guessed.

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
`none`. Proving *absence* is built and described below.

Because the commitment *is* the state root, it is not compile-time optional: a build
computing a different root could never replicate with one that did not. Changing it
therefore came with on-disk format v2 and PROTOCOL_VERSION 2. Per-recipient sealing
brought v3 for the same kind of reason, this time a genuine wire change. Either way a
stale store is refused until migrated and a mismatched peer refuses the handshake rather
than syncing and then disagreeing forever.

### Reading Deferred Content

The energy scheduler can decide to take operations and skip the bytes. That leaves a node
that knows a file exists, where it lives and what it is made of — and cannot read it.

A read now closes that gap itself. The facade asks `core` what the read is missing, hands
that list to whatever transport the daemon has, and reads once it arrives. `core` never
learns about peers: it answers the question, the daemon answers the network.

Three things can be missing and all of them count. Chunks a *pending* write is waiting on
— the usual case, because under a metadata-only budget the write never applied. The
`FileNode` object itself, which is a stored object like any other and so is skipped along
with the content; the inode records its hash, so it can still be asked for by name. And
the chunks that object names. Because the second must arrive before the third can be
named, fetching runs in rounds, bounded so that a round which learns nothing new stops
rather than repeating itself.

The honesty problem this exposed is worth stating plainly: a parked write means the inode
has no content, so a bare read of a deferred file returns *empty*. The facades therefore
refuse — `503` from both the admin API and the S3 facade — rather than serving a short
file, which is a wrong answer that looks like a right one.

A fetch takes only what the read needs. Reading one file does not pull the whole deferred
backlog across, or the budget the scheduler set would mean nothing. Content is
hash-checked on arrival exactly as in a sync pass, and the same no-progress guard
applies: a peer that keeps promising more while handing back nothing usable ends the
exchange rather than looping.

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
apply improved about 6%, because an apply is dominated by its fsync rather than by
computation — a single fsync, since the duplicate described below was removed. The
remaining cost is still linear: the Merkle tree is rebuilt from the map each time, so an
incremental tree updating one leaf in O(log n) is the next layer rather than something
this change delivered.

Correctness is pinned by an invariant suite that re-derives the map with a full walk
after every kind of operation and demands equality, since a wrong entry is not a stale
cache but two replicas disagreeing about what the filesystem is.

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

That boundary syncs once, which it did not always do. `Stores` holds a blob store and a
key-value store, and every deployment builds both from one sled database — so flushing
each in turn fsynced the same shared log twice for a single operation. `Stores::shared`
now records that the two roles are one backend and `flush` syncs accordingly; the
general case has `Stores::split`, which still syncs both. Measured over 300 `mkdir`s on
a warm database, the redundant sync cost 7ms of a 16.7ms operation boundary.

### Hardening

A review pass over the replication and proof paths produced four changes worth recording,
because each of them is a class of mistake rather than a one-off.

**A peer's answer is not permission to store.** `fetch_chunks` and the blob phase of
`pull_from_peer` accepted any blob whose bytes matched its own hash. Self-consistency is
not the same as being asked for: a trusted-but-hostile peer could write arbitrary content
into the content-addressed store, and because every stored blob counted as progress, the
no-progress guard that ends a session never fired. Both now drop anything outside the
set this node actually requested.

**A label is not a claim.** `nexusfs check-proof` printed the proof file's `path` and
`inode` header fields as the proof's subject. Those fields are not covered by the proof,
so editing them relabelled a genuine proof as being about another file while the tool
still reported that it held. It now prints the inode the proof commits to, marks the
file's own labels as unverified, and warns when the two disagree — the one tool whose job
is to not be fooled should not be the one repeating an attacker's text.

**A wrong config is when you most want the console.** Replication starts before the
facades, because a read borrows its transport to fetch deferred content. That ordering
made a typo in `net.listen` abort the daemon before the admin server was up. The error is
now reported and replication alone stays down.

**Feature combinations rot when nothing builds them.** A `cfg` on a type and a `cfg` on
its construction site had drifted apart, so a replication-only build did not compile —
invisible because CI built two combinations out of seven. It now builds all of them with
warnings denied.

**A daemon that starts in silence cannot be operated.** `EnvFilter::from_default_env()`
resolves to ERROR only when `RUST_LOG` is unset, so every `info!` and `warn!` in the
workspace was dead by default: no "admin listening on", no "replication enabled", and no
warning that trust-on-first-use would pin whichever key connected first. That last one is
what made it a bug rather than a preference — a security notice nobody can read is not a
notice. The filter now falls back to `info`, and `RUST_LOG` still wins in both
directions.

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

231 tests, including order-independent convergence (the same operation set applied in
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

The commitment values themselves are pinned as literals. Every other Merkle test checks
the tree against *itself*, and all of them keep passing if the hashing changes as long
as it changes consistently — but the root is on disk and on the wire, so a refactor that
quietly moved it would break every existing repository while the suite stayed green.

Link-cost detection is tested by splitting each probe into a subprocess call and a pure
parser, so the parsers are exercised on any host against captured output: NetworkManager
answering in both directions and when guessing, a default route chosen by lowest metric
where two exist, macOS hardware ports where a port name contains an interface name, and
the VPN case where no port matches. Also that an absent or unparseable source reads as
unknown rather than unmetered, and that a stated `link_cost` is used verbatim.

## Partially Implemented Or Present As Scaffolding

- Key rotation and re-encryption of content already written. `share` re-seals a file
  key to more recipients; nothing withdraws access from one, which needs the content
  itself re-encrypted.
- Link-cost detection covers Linux via NetworkManager and, on macOS, only a phone
  tethered over USB. A Wi-Fi hotspot reads as unknown, and a VPN hides the link on every
  platform. `energy.link_cost` is the way to state what a probe cannot see.
- `crates/fs_posix` and `crates/privacy` are stubs. `crates/zk` is not — it holds the
  Merkle commitment, the proofs and the transparent bundles; what it does not hold is a
  proving system.

## Backlog

Nothing here blocks anything already shipped. These are the things that are genuinely
not built, grouped by what they would buy.

### Replication And Scheduling

- **The two cases link-cost detection cannot see.** A Wi-Fi hotspot on macOS looks
  identical to home broadband, and a VPN hides the link beneath it on every platform.
  `energy.link_cost` exists to work around both; closing them properly needs more than a
  default-route lookup.
- **Push notification of new operations**, so peers do not wait out the poll interval.
- **Delta-encoded operation ranges** rather than whole-operation batches.
- **Prioritising which deferred content to fetch first** when the budget is capped.
  Today the order is whatever `missing_chunk_hashes` returns, not what a user is likely
  to want next.

### Encryption

- **Encrypted names and directory structure**, which are in the clear today. Larger than
  it sounds, because it breaks the S3 facade's key mapping.
- **Whether the handshake should carry the sealing key**, so trust-on-first-use pins both
  and encrypted replication works without a second enrolment step. The cost is granting
  read access to whoever connects first, which is a larger grant than accepting their
  operations — a question rather than an obvious yes.
- **Rotation on a schedule**, or triggered by revocation. Today it is a command an
  operator runs.

### Proofs

- **Compressed proof batches.** `prove_many` shares one traversal but still sends every
  path in full; overlapping paths could share their upper steps.
- **A proving system**, which needs a circuit-friendly hash and is research rather than
  engineering. See "Deliberately Not Built".

### Performance

- **An incremental Merkle tree.** The inode map is maintained rather than walked, but
  the commitment over it is still rebuilt per apply — linear work for a one-entry
  change. Weigh it against the fact that an apply is fsync-bound.
- **Cached directory maps.** `resolve_path` re-reads and re-materializes each directory
  once per path component.

### Storage Maintenance

- **Compaction policy**, which does not exist in any form. `gc` reclaims what nothing
  refers to; nothing compacts the database underneath it.

### Product Surface

- **A mountable interface.** POSIX/FUSE is not a debt — M2 asked for one facade and the
  S3 one shipped — so this is new scope. WebDAV reaches the same user-visible outcome
  without a kernel extension and is testable in CI.
- **SigV4 request signing** for the S3 facade, or documenting loopback-only as the
  supported deployment.
- **Multipart upload**, for objects too large to buffer in one request.

### Tests

- Cross-node integration tests beyond the two-node script.
- Restart and crash-recovery tests.
- Transport failure and retry tests.

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
- **Persisted telemetry snapshots** (M5). A power reading from before a restart
  describes a machine that may since have been unplugged, moved or cooled. Persisting it
  would produce a stored value that looks authoritative and is not.
- **A compile-time switch for the commitment.** The commitment *is* the state root, so a
  build that computed a different one could never replicate with one that did not.
  Making it optional would be a convergence hazard dressed as a feature flag.
- **Convergent encryption.** It would restore deduplication across identical plaintext,
  at the price of letting anyone holding a candidate file confirm whether a node stores
  it.

## Where To Pick This Up

No milestone is unfinished and nothing above is blocked. The backlog is real, though —
see it above — so this section is about what is worth doing next, not what is owed.

Two candidates are large enough to weigh rather than pick by order.

**A mountable interface — if one is wanted.** Note first that POSIX/FUSE is not a debt:
M2 asked for *one* user-facing facade, named POSIX and S3 as alternatives, and the S3 one
shipped. What a mount buys is that unmodified programs — an editor, `git`, `rsync`,
Finder — can use the filesystem, which is the difference between an object store you
script against and a folder you use.

Two things to settle before writing any of it:

- *Write granularity.* An applied operation costs about 6ms including its fsync. POSIX
  writes arrive in small pieces, so mapping one `write()` to one operation makes copying
  a 10MB file roughly 2,500 operations — and partial writes take the splice path, which
  reads the whole file back each time, so the cost is quadratic rather than linear. The
  fix is ordinary — buffer per file handle, emit one operation on release or fsync — but
  it is design work, not binding work, and it belongs first.
- *Which interface.* FUSE needs a kernel extension. WebDAV does not, mounts natively in
  Finder, Explorer and GIO, reuses the existing HTTP stack and path model, and unlike
  FUSE is testable in CI. It does not give true POSIX semantics — but neither does a
  mount too slow to use.

**An incremental Merkle tree.** The map is maintained rather than walked now, but
the commitment over it is still rebuilt per apply — linear in the filesystem for a change
that touched one entry. Weigh it against the fact that an apply is currently fsync-bound,
so the end-to-end gain would be small.

**Also open, smaller.** Compressed proof batches, where overlapping paths share their
upper steps. Push notification, so peers do not wait out the poll interval. Prioritising
which deferred content to fetch first under a capped budget. Cached directory maps, since
`resolve_path` re-materializes each directory once per path component.
