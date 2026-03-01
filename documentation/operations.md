# Operations Guide

NexusFS is designed to stay operational as a single local process, even before networking and optional facades are fully enabled.

## Configuration

The daemon loads a TOML configuration file with sections for:

- node identity and data directory
- network binding and peer list
- admin interface binding and token
- optional S3 and POSIX surfaces
- security mode
- energy thresholds

The example baseline is:

- `examples/nexusfs.toml`

The deep config reference is:

- `../docs/config.md`

## Runtime Layout

Typical local runtime state includes:

- a persistent database directory
- a generated identity file
- blob and KV trees inside the selected backend
- persisted head and administrative metadata

## Operating The Admin Interface

The admin service is intended to provide:

- local status and current head visibility
- storage and oplog inspection
- future peer and replication status
- future energy telemetry and proof statistics

## Development Workflow

Recommended day-to-day workflow:

1. Build with `cargo build -p nexusfs`
2. Run focused crate tests while developing
3. Run workspace tests after each meaningful slice
4. Exercise the daemon with the sample config
5. Keep documentation in sync with any new operational behavior

## Expected Evolution

As the project matures, this guide should expand to cover:

- backup and restore
- peer enrollment
- storage compaction
- migration flows
- proof verification tooling
