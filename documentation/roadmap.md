# Roadmap

NexusFS is already bootstrapped as a compile-oriented workspace. The path forward is intentionally staged so every milestone leaves behind a runnable, inspectable system.

## M0: Bootstrapped Workspace

- Rust workspace and crate boundaries
- runnable daemon and admin stub
- design and protocol documentation

## M1: Local Filesystem Core

- storage backends
- chunking and CAS writes
- oplog persistence
- snapshots and persistent heads

## M2: External Facade

Choose one surface first:

- POSIX/FUSE facade
- S3-like object API

## M3: Replication

- peer transport
- operation synchronization
- blob transfer
- verified remote apply

## M4: Encryption and Transparent Proofs

- encrypted chunk storage
- key envelopes
- proof bundles and verification tooling

## M5: Energy-Aware Scheduling

- telemetry sampling
- policy-driven replication throttling
- admin insight into scheduler decisions

## M6+: ZK Commitments And Research Expansion

- first ZK circuit
- stronger commitment proofs
- broader privacy and systems research tracks

For the canonical internal milestone map, use `../docs/roadmap.md`.
