# NexusFS

> **NexusFS: A Single-Executable, Verifiable Distributed File System with Zero-Knowledge Proofs and Energy-Aware Replication for Edge and Offline-First Applications**

A Rust workspace building toward a verifiable, offline-first distributed filesystem in a
single binary.

> **Status — milestones M0–M8 of 8 complete.** Files round-trip through a signed
> operation log applied to CRDT-backed namespace state, an S3-compatible facade exposes
> that state over HTTP, **two nodes converge over QUIC** with every operation and chunk
> verified before it is accepted, **content can be encrypted at rest** while still
> replicating, and **replication now adapts to the device's power situation** — deferring
> content while still tracking the namespace when the battery is low, the machine is hot,
> or the link is metered. Operators get **garbage collection, format versioning and
> explicit peer enrolment**, and the state root is now a **Merkle commitment** whose
> entries can be proved individually — including proving that one is **absent**, which
> makes a deletion demonstrable. The POSIX/FUSE facade is **not implemented yet**, and
> the commitment layer is **not zero-knowledge** — see below. Full detail in
> [`documentation/current-status.md`](documentation/current-status.md).

## Try it in your browser

**[minh.systems/NexusFS/playground.html](https://minh.systems/NexusFS/playground.html)**

Two replicas in one tab: partition them, edit both, sync, and watch the state roots
converge with deterministic conflict naming. It runs the real core compiled to
WebAssembly — the same operations, CRDT state and conflict rules as the native binary,
differing only in storage backend. Nothing is installed and nothing leaves the page.

The repository also includes:
- Message schema + protocol spec (`docs/protocol.md`)
- Threat model (`docs/threat_model.md`)
- Architecture blueprint (`docs/architecture.md`)
- Research tracks beyond R01–R20 (`docs/research_tracks.md`)
- Implementation playbook with step-by-step tasks (`docs/coding_playbook_codex.md`)
- Public-facing docs hub (`documentation/`)
- Static project website for GitHub Pages (`site/`)
- Example config (`examples/nexusfs.toml`)
- Embedded admin UI (served from `crates/admin/assets/`)

---

## Quick start

```bash
cargo build -p nexusfs
```

Copy the example config and point `node.data_dir` somewhere outside any cloud-synced
folder (a leading `~` is expanded):

```bash
cp examples/nexusfs.toml ./nexusfs.toml
```

Then drive the filesystem:

```bash
cargo run -p nexusfs -- mkdir --config ./nexusfs.toml /docs
```

```bash
echo "hello nexus" > /tmp/a.txt && cargo run -p nexusfs -- put --config ./nexusfs.toml /tmp/a.txt /docs/a.txt
```

```bash
cargo run -p nexusfs -- ls --config ./nexusfs.toml /docs
```

```bash
cargo run -p nexusfs -- cat --config ./nexusfs.toml /docs/a.txt
```

```bash
cargo run -p nexusfs -- status --config ./nexusfs.toml
```

Commands: `mkdir [-p]`, `put`, `cat`, `ls`, `rm`, `mv`, `status`, `verify`, `gc`,
`migrate`, `peer`, `prove`, `check-proof`, `daemon`.

Run the daemon for the admin console on <http://127.0.0.1:7070> — it browses the
filesystem, shows replication and power state, and audits the repository on request:

```bash
cargo run -p nexusfs -- daemon --config ./nexusfs.toml
```

Note that the embedded database takes an exclusive lock, so `status` cannot run while
the daemon holds the store — query the running daemon through the admin API instead.

---

## Replication

Set `net.peers` to the addresses of other nodes and run the daemon with the `quic`
feature. Each node periodically pulls whatever it is missing from its peers.

```bash
./scripts/dev_run_two_nodes.sh
```

That seeds two nodes with different content while both are stopped, starts them as
peers, and waits until they report the same state root — including the deterministically
renamed copy of the directory each created independently.

How it works: a node sends its clock summary, the peer replies with the operations it
lacks, and only then does it request the chunks those operations reference. Operation
signatures and chunk hashes are both verified before anything is accepted, using the
same apply path local writes use.

Peer identity is an ed25519 key pinned on first use — TLS provides transport encryption
only, and certificates are not the trust anchor. A device presenting a different key
than the one pinned is refused. Set `net.tofu = false` to require explicit enrolment.

---

## Encryption and proofs

Turn both on in the config:

```toml
[security]
encrypt_at_rest = true
proof_mode = "transparent"   # or "required" to reject unproven operations
```

Chunk content is then encrypted with XChaCha20-Poly1305 before it reaches disk. Each
write mints a fresh file key, sealed with a repository key kept beside the device
identity (`repo.key`, written owner-only) and carried inside the file itself.

Chunks stay addressed by the hash of the bytes **as stored** — the ciphertext. That is
what keeps replication verifiable: a peer checks the hash before accepting content and
must be able to do so without any key. A peer with a different repository key still
converges on structure and still verifies transfers; it simply cannot read the content.

With proofs on, every local operation carries a bundle recording the state root before
it, the state root after it, and the objects it introduced — signed along with the
operation, so the author cannot later claim a different transition. Malformed bundles
are rejected deterministically on receipt.

Audit a repository at any time:

```bash
cargo run -p nexusfs -- verify --config ./nexusfs.toml
```

That checks every signature and proof, and reads every file back — so missing chunks,
wrong keys and tampered ciphertext all surface here. It exits non-zero on failure, so it
works as a cron or CI check. The same report is served at `/api/security`.

**Limits worth knowing.** Replicas share one repository key, so this protects the disk
and the wire, not one peer from another. File names, directory structure and file sizes
are not encrypted. `zk_commit` is implemented — see [Provable state](#provable-state) —
but is a commitment scheme rather than zero-knowledge. `zk_full` is accepted as a config
value and behaves as `none` rather than pretending to prove anything.

---

## Operating a repository

### Reclaiming storage

Nothing is overwritten in place, so a repository grows even when files do not. The
larger source is not overwrites but snapshots: every applied operation rebuilds the
snapshot, orphaning the previous one and a directory object for each level on the path
to the change. Storage therefore accrues per *operation*, and an append-only history
still has plenty to reclaim.

```bash
cargo run -p nexusfs -- gc --config ./nexusfs.toml
```

That surveys and prints what could be freed. Add `--apply` to actually delete. The
survey is the default because the failure modes are asymmetric — an unnecessary report
costs a walk of the namespace, an unnecessary delete costs data.

What survives: everything reachable from live state, and everything a parked operation
still refers to. What goes: superseded file versions, unlinked content, and old snapshot
objects. `/api/storage/gc` reports the same survey but never deletes, because the daemon
could write a blob between the mark and the sweep.

Collection refuses outright if the repository has no head or no root inode record —
those mean corruption or a half-finished restore, which is exactly when deleting is
worst.

### Upgrading across formats

The store records the on-disk format it was written with. A build that finds an older
one refuses to open it and tells you to migrate; a build that finds a *newer* one
refuses and cannot be forced, because it cannot know what the later format means.

```bash
cargo run -p nexusfs -- migrate --config ./nexusfs.toml
```

Opening never migrates on its own. A migration rewrites records in place, and you may
have no backup, or be running the wrong binary, or be mid-rollout across several nodes.
Refusing costs one command.

### Enrolling peers without trust-on-first-use

TOFU is convenient and wrong whenever first contact could be intercepted. Set
`net.tofu = false` and enrol keys directly. On each node:

```bash
cargo run -p nexusfs -- peer identity --config ./nexusfs.toml
```

That prints the device id, the public key, and the exact command to run on the other
node. Then:

```bash
cargo run -p nexusfs -- peer add --config ./nexusfs.toml <device-id> <pubkey>
```

`peer list` shows what is enrolled and `peer remove` forgets a key. Enrolling a
*different* key for a known device needs `--rotate`: overwriting silently would erase
the one signal that separates a planned rotation from an impersonation attempt.

---

## Provable state

The state root is a Merkle commitment over the inode map. Two replicas still compare a
single hash to decide whether they agree — but now any one entry can be proved on its
own, to someone holding no filesystem at all.

```bash
cargo run -p nexusfs -- prove --config ./nexusfs.toml /docs/note.txt --out note.json
```

That writes a self-contained proof: the inode, its object hash, and the sibling hashes
up to the root. Checking it opens no repository:

```bash
cargo run -p nexusfs -- check-proof note.json --root <state-root>
```

Pass `--root` with a root you obtained independently. Without it, the proof is checked
against the root recorded *inside itself*, which confirms internal consistency and
nothing about whether that state is one anyone else agrees with — the command says so
rather than printing a bare "valid". `/api/fs/proof?path=` serves the same structure.

Turning it on for operations:

```toml
[security]
proof_mode = "zk_commit"
```

Each local operation then carries an inclusion path for the entry it is about, and a
receiver checks that path against the root the operation claims — without holding the
author's prior state, which a transparent proof requires. Transparent proofs are still
accepted under this policy: they prove less, but they are not wrong, and refusing them
would make the mode unusable while a cluster is being upgraded.

**This is not zero-knowledge.** A verifier learns the inode and its object hash; what it
does not learn is the rest of the tree. The mode is named `zk_commit` because a sibling
path is exactly the witness a SNARK circuit would consume — the commitment half of the
work — not because a proving system is involved. `zk_full` remains unimplemented and
behaves as `none`.

### Proving something is gone

Absence needs a different construction, because a path that resolves to nothing has no
inode to name. Ask by inode instead:

```bash
cargo run -p nexusfs -- prove --config ./nexusfs.toml --inode <inode> --out gone.json
```

The leaves are sorted, so absence is the claim that two *adjacent* entries straddle the
inode — there is nowhere else it could be. Adjacency is the load-bearing part: two
entries that merely bracket it prove nothing, since the inode could be one of the
entries between them. Each neighbour therefore states its position, and that position is
checked against the shape its path must have in a map of that size.

Pairing the two directions is what makes a deletion demonstrable to someone holding
neither state:

```bash
# it was there
nexusfs check-proof existed.json --root <root-before>   # claim: present
# and it is not there now
nexusfs check-proof gone.json    --root <root-after>    # claim: absent
```

---

## Energy-aware replication

```toml
[energy]
enabled = true
battery_low_pct = 20
temp_high_c = 70
```

Before each sync pass the daemon samples the device — power source, charge, temperature,
CPU load, link cost — and decides how much replication may do.

The decision rests on one observation: **operations are tiny and content is large.** An
operation is a few hundred bytes describing an intent; the content it refers to can be
megabytes. So the interesting throttle is not "sync or don't" but *keep the namespace
converged and defer the bytes*. A device that has taken every operation but no content
still knows what exists, where, and at what version — it can list directories, answer
"has this changed", and fetch any particular file the moment someone wants it. That is a
far better degraded state than falling behind entirely, and it costs almost nothing.

| Condition | Operations | Content |
| --- | --- | --- |
| On mains, or power source unknown | yes | unlimited |
| Battery above ~2× the low threshold | yes | unlimited |
| Battery in the conserving band | yes | capped per pass |
| Battery at or below `battery_low_pct` | yes | deferred |
| Battery at or below the critical threshold | no | no |
| Temperature at or above `temp_high_c` | yes | deferred |
| Link is metered | yes | deferred |

Heat and metered links override the battery grade rather than folding into it: no amount
of remaining charge makes cooking the device acceptable, and a metered link costs money
per byte regardless of power.

**Unknown never means constrained.** A server with no battery sensor reports `unknown`
and runs unthrottled. Treating a missing sensor as an empty battery would make an
unconstrained machine throttle itself permanently — the obvious failure mode of a naive
implementation, and the reason every reading is a three-state enum rather than a `bool`.

Inspect the live decision, including why it was made:

```bash
curl -H "x-nexusfs-token: $TOKEN" http://127.0.0.1:7070/api/energy
```

```json
{"enabled":true,"power":"battery","battery_pct":14,"temp_c":null,
 "link":"unknown","sync":true,"content":false,"max_content_bytes":0,
 "interval_scale":2.0,"reason":"battery 14% is at or below the low threshold 20%"}
```

Set `enabled = false` to remove every limit while keeping the reading visible.

### Reading what was deferred

A node under a metadata-only budget knows a file exists and holds none of its bytes.
Rather than wait for the next unconstrained pass, a read fetches exactly what it needs
from a peer and then serves the file.

That matters more than it sounds, because of what the alternative looks like. A deferred
write is *parked*, not applied, so the inode has no content yet — and a bare read of such
a file returns **empty**. The facades therefore ask what the read is missing, try to
fetch it, and return `503` if it still cannot be had. A short file is a wrong answer
wearing the costume of a right one.

The fetch takes only what that read needs: reading one file does not drag the whole
deferred backlog across, or the budget would mean nothing. Content is hash-checked on
arrival exactly as in a sync pass — it is a second door into the same store.

---

## S3-compatible API

Set `s3.enabled = true` in the config and run the daemon with the `s3` feature. Objects
written over HTTP are ordinary files: the CLI can `cat` them and they appear in the
oplog as signed operations.

```bash
cargo run -p nexusfs --features s3 -- daemon --config ./nexusfs.toml
```

```bash
curl -X PUT --data "hello" http://127.0.0.1:9000/reports/2024/q1.txt
```

```bash
curl http://127.0.0.1:9000/reports/2024/q1.txt
```

```bash
curl "http://127.0.0.1:9000/reports?delimiter=/"
```

Supported: object PUT/GET/HEAD/DELETE, bucket create and list, ListObjectsV2 with
prefix, delimiter and pagination. A bucket is a top-level directory; the object key is
the path beneath it.

Not supported, by design in v0: SigV4 signing, multipart upload, versioning, ACLs and
lifecycle rules. Authentication is an optional shared secret (`s3.token`, sent as
`x-nexusfs-token`), so keep the facade on loopback unless you set one. ETags are BLAKE3
rather than MD5.

---

## Features / build flags

The project uses feature flags so the **same binary** can scale down to constrained devices.

Defaults for the `nexusfs` binary are `admin` only; the rest are opt-in.

- `admin` (default): embedded admin API + UI
- `quic`  (off by default): QUIC transport + peer replication
- `s3`    (off by default): S3-compatible HTTP API
- `posix` (off by default): FUSE mount stubs (OS-dependent)
- `zk`    (off by default): ZK scaffolding (proof traits + placeholder circuits)

The `nexusfs-storage` crate defaults to `sled`; `rocksdb` is an off-by-default
alternative and is currently a stub.

Example:
```bash
cargo build -p nexusfs --features "admin,quic,s3"
```

---

## Building outside a synced folder

If your checkout lives in iCloud Drive, Dropbox or similar, redirect Cargo's output so
the sync client is not fighting a build directory:

```bash
export CARGO_TARGET_DIR=~/Library/Caches/nexusfs-target
```

Or create a git-ignored `.cargo/config.toml` in the repository root with a
`[build] target-dir = "..."` entry.

---

## Building the playground locally

`site/nexusfs.wasm` is git-ignored and built by the Pages deploy workflow. To preview
the playground yourself, build it and serve `site/` over HTTP — a `file://` page cannot
fetch the module:

```bash
./scripts/build_wasm.sh
```

```bash
python3 -m http.server 8099 --directory site
```

No wasm-bindgen or wasm-pack needed — the module exposes a plain JSON-over-buffers
interface with no JS imports, so `cargo build --target wasm32-unknown-unknown` is the
whole toolchain. CI builds it on the wasm target, runs clippy against it, and asserts
the module still has zero JS imports.

The artefact is not committed because it is not reproducible across machines: the rustc
version and the build's absolute paths both end up inside the binary.

---

## Where to start coding

Read and follow:

- `docs/coding_playbook_codex.md`  ← step-by-step implementation tasks
- `docs/protocol.md`              ← replication protocol, message framing, auth
- `docs/threat_model.md`          ← attacker types & mitigations to preserve
- `docs/architecture.md`          ← module responsibilities and invariants

Additional specs:
- `docs/object_formats.md`
- `docs/ops_semantics.md`
- `docs/admin_api.md`
- `docs/glossary.md`

Public-facing project material:
- `documentation/`               ← curated markdown docs for collaborators and users
- `site/`                        ← static website for GitHub Pages / personal project pages


---

## Repository map

- `crates/nexusfs`    : single-binary entrypoint (CLI + daemon wiring)
- `crates/core`       : CAS objects, canonical encoding, chunking, namespace state, apply pipeline, snapshots
- `crates/storage`    : storage traits + backends (sled by default)
- `crates/crypto`     : identity keys, signing, AEAD encryption envelopes
- `crates/proto`      : shared types (ops + net messages)
- `crates/crdt`       : OR-Map + LWW registers + conflict handling
- `crates/net`        : QUIC transport, signed handshake, peer manager, sync sessions
- `crates/admin`      : embedded admin console backend + static UI assets
- `crates/energy`     : device telemetry + the rule-based replication scheduler
- `crates/privacy`    : padding + cover traffic (stubs)
- `crates/zk`         : proof traits, transparent proof bundles, ZK placeholders
- `crates/s3`         : S3-like API surface (stubs)
- `crates/fs_posix`   : FUSE mount surface (stubs)
- `crates/wasm`       : browser build of the core, powering the playground
- `documentation`     : public markdown documentation hub
- `site`              : static project website and GitHub Pages source

---

## License
Dual licensed: Apache-2.0 OR MIT.
