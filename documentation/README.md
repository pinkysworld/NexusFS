# NexusFS Documentation

This folder is the public-facing documentation layer for NexusFS.

It complements the deeper engineering specifications in `/docs` with a cleaner entry point for collaborators, reviewers, and future users.

## What Is NexusFS?

NexusFS is a single-executable, verifiable distributed file system for edge and offline-first environments. It combines:

- local-first content-addressed storage
- signed operation logs
- deterministic replication
- optional proof modes, starting with transparent verification
- an embedded admin surface and optional API facades

## Where The Project Actually Is

**Milestone M1 is complete and M2 has not started.** The local filesystem works:
files can be created, written, listed, read back, renamed and removed through a
signed operation log applied to CRDT-backed namespace state, and that state survives
restart. Operations converge to the same result regardless of the order they arrive in.

**Nothing replicates over a network yet.** The QUIC transport and handshake exist, but
there is no peer manager and no Have/Want negotiation. Encryption at rest, proof
enforcement, the S3 and POSIX facades, energy-aware scheduling and ZK proofs are all
still unimplemented.

Read [Current Status](./current-status.md) for the precise breakdown before relying on
anything here.

## Try It Without Installing Anything

The [playground](https://pinkysworld.github.io/NexusFS/playground.html) runs the real
core compiled to WebAssembly — two replicas in one browser tab, with a partition and
deterministic convergence. It is the same Rust code the native binary runs; only the
storage backend differs.

## Documentation Map

- [Current Status](./current-status.md): what is implemented now and what remains in backlog
- [Backlog](./backlog.md): prioritized outstanding work grouped by execution order
- [Getting Started](./getting-started.md): build, run, and inspect the daemon
- [Architecture Overview](./architecture.md): the crate layout and system invariants
- [Protocol and Replication](./protocol-and-replication.md): operation flow, peer sync, and proof boundaries
- [Operations Guide](./operations.md): configuration, deployment, and maintenance
- [Security Model](./security.md): threat model summary and verification rules
- [Roadmap](./roadmap.md): milestone path from local core to full replication and ZK modes

## Source Specifications

These pages summarize the internal source specs. For implementation-level detail, use:

- `../docs/architecture.md`
- `../docs/protocol.md`
- `../docs/threat_model.md`
- `../docs/config.md`
- `../docs/ops_semantics.md`
- `../docs/roadmap.md`

## Project Principles

1. One binary should be enough to run the core system.
2. Data integrity must be verifiable before trust is granted.
3. Replication must tolerate partitions and converge deterministically.
4. Optional research features should never obscure the practical baseline.
