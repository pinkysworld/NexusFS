# NexusFS

> **NexusFS: A Single-Executable, Verifiable Distributed File System with Zero-Knowledge Proofs and Energy-Aware Replication for Edge and Offline-First Applications**

A Rust workspace building toward a verifiable, offline-first distributed filesystem in a
single binary.

> **Status — milestone M1 of 8 complete.** The local filesystem works: files can be
> created, written, listed, read back, renamed and removed through a signed operation
> log applied to CRDT-backed namespace state, and that state survives restart.
> Replication, encryption at rest, the S3/FUSE facades, energy-aware scheduling and ZK
> proofs are **not implemented yet**; those crates are still scaffolding. See
> [`documentation/current-status.md`](documentation/current-status.md).

## Try it in your browser

**[minh.systems/NexusFS/site/playground.html](https://minh.systems/NexusFS/site/playground.html)**

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

Commands: `mkdir [-p]`, `put`, `cat`, `ls`, `rm`, `mv`, `status`, `daemon`.

Run the daemon for the admin console on <http://127.0.0.1:7070>:

```bash
cargo run -p nexusfs -- daemon --config ./nexusfs.toml
```

Note that the embedded database takes an exclusive lock, so `status` cannot run while
the daemon holds the store — query the running daemon through the admin API instead.

---

## Features / build flags

The project uses feature flags so the **same binary** can scale down to constrained devices.

Defaults for the `nexusfs` binary are `admin` only; the rest are opt-in.

- `admin` (default): embedded admin API + UI
- `quic`  (off by default): QUIC transport + replication protocol scaffolding
- `s3`    (off by default): S3-like HTTP API stubs
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
the playground yourself:

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
- `crates/net`        : QUIC transport + handshake (replication state machine not implemented)
- `crates/admin`      : embedded admin console backend + static UI assets
- `crates/energy`     : telemetry + baseline scheduler interface
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
