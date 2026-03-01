# NexusFS — Blueprint + Skeleton Workspace (Zip Deliverable)

This repository is a **Codex-ready, compile-oriented project skeleton** for:

> **NexusFS: A Single-Executable, Verifiable Distributed File System with Zero-Knowledge Proofs and Energy-Aware Replication for Edge and Offline-First Applications**

It includes:
- A Rust workspace with crate boundaries matching the intended architecture
- Message schema + protocol spec (`docs/protocol.md`)
- Threat model (`docs/threat_model.md`)
- Architecture blueprint (`docs/architecture.md`)
- Research tracks beyond R01–R20 (`docs/research_tracks.md`)
- Codex implementation playbook with step-by-step tasks (`docs/coding_playbook_codex.md`)
- Public-facing docs hub (`documentation/`)
- Static project website for GitHub Pages (`site/`)
- Example config (`examples/nexusfs.toml`)
- Minimal embedded admin UI (served from `crates/admin/assets/`)

> **Status**: This is a *skeleton* — it compiles and boots a minimal daemon + admin API,
> but most modules contain TODO stubs so Codex (or a human dev) can fill in implementation iteratively.

---

## Quick start

### 1) Build
```bash
cargo build -p nexusfs
```

### 2) Run the daemon (admin on 127.0.0.1:7070)
```bash
cargo run -p nexusfs -- daemon --config examples/nexusfs.toml
```

### 3) Open the admin UI
- http://127.0.0.1:7070

---

## Features / build flags

The project uses feature flags so the **same binary** can scale down to constrained devices.

- `admin` (default): embedded admin API + UI
- `quic`  (default): QUIC transport + replication protocol stubs
- `s3`    (off by default): S3-like HTTP API stubs
- `posix` (off by default): FUSE mount stubs (OS-dependent)
- `rocksdb` (off by default): RocksDB backend (native build)
- `sled`    (default): pure-Rust KV backend via `sled` (simpler builds)
- `zk`      (off by default): ZK scaffolding (proof traits + placeholder circuits)

Example:
```bash
cargo build -p nexusfs --features "admin,quic,s3"
```

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
- `crates/core`       : CAS objects, canonical encoding, chunking, snapshots, state
- `crates/storage`    : storage traits + backends (sled by default)
- `crates/crypto`     : identity keys, signing, AEAD encryption envelopes
- `crates/proto`      : shared types (ops + net messages)
- `crates/crdt`       : OR-Map + LWW registers + conflict handling
- `crates/net`        : QUIC transport + replication state machine
- `crates/admin`      : embedded admin console backend + static UI assets
- `crates/energy`     : telemetry + baseline scheduler interface
- `crates/privacy`    : padding + cover traffic (stubs)
- `crates/zk`         : proof traits, transparent proof bundles, ZK placeholders
- `crates/s3`         : S3-like API surface (stubs)
- `crates/fs_posix`   : FUSE mount surface (stubs)
- `documentation`     : public markdown documentation hub
- `site`              : static project website and GitHub Pages source

---

## License
Dual licensed: Apache-2.0 OR MIT.
