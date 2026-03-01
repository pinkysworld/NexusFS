# Getting Started

NexusFS is organized as a Rust workspace. The current baseline includes a runnable daemon, a local storage core, and an embedded admin interface.

## Requirements

- Rust toolchain with Cargo
- A writable local data directory
- No external services are required for the default local-core flow

## Build

```bash
cargo build -p nexusfs
```

## Run The Daemon

```bash
cargo run -p nexusfs -- daemon --config examples/nexusfs.toml
```

By default, the daemon:

- opens the local sled-backed storage
- creates a persistent device identity if one does not exist
- bootstraps a repository head on first launch
- starts the admin interface on the configured bind address

## Check Local Status

```bash
cargo run -p nexusfs -- status --config examples/nexusfs.toml
```

This prints:

- the persistent device identifier
- the current head hash
- the generated admin token if one is in use

## Current Local-Core Capabilities

The current implementation supports:

- canonical object encoding for filesystem objects
- fixed-size chunking with deterministic chunk hashes
- chunk storage in the content-addressed blob store
- head snapshots stored in CAS and persisted in KV
- minimal idempotent oplog application scaffolding

## Next Practical Steps

1. Extend `apply_op_minimal` into full CRDT-backed directory and inode state.
2. Expose richer storage and head introspection in the admin API.
3. Enable peer-to-peer replication behind the QUIC transport.
