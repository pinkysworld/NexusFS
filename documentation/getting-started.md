# Getting Started

NexusFS is a Rust workspace that builds one binary. The daemon, the CLI, the admin
console and the optional facades are all the same executable behind feature flags.

## Requirements

- Rust toolchain with Cargo
- A writable local data directory
- No external services are required

If your checkout lives in iCloud Drive, Dropbox or similar, redirect Cargo's output
first — a sync client fighting a build directory is slow at best, and an embedded
database under one can corrupt:

```bash
export CARGO_TARGET_DIR=~/Library/Caches/nexusfs-target
```

## Build

```bash
cargo build -p nexusfs
```

## Configure

```bash
cp examples/nexusfs.toml ./nexusfs.toml
```

Point `node.data_dir` somewhere outside any synced folder. A leading `~` is expanded.

## Use The Filesystem

Every mutating verb builds a signed operation and applies it through the same pipeline
replication uses — there is no back door into the state machine.

```bash
cargo run -p nexusfs -- mkdir --config ./nexusfs.toml /docs
```

```bash
echo "hello nexus" > /tmp/a.txt
cargo run -p nexusfs -- put --config ./nexusfs.toml /tmp/a.txt /docs/a.txt
```

```bash
cargo run -p nexusfs -- ls --config ./nexusfs.toml /docs
cargo run -p nexusfs -- cat --config ./nexusfs.toml /docs/a.txt
```

`rm` and `mv` complete the set. Run `status` in a fresh process to confirm the state
survived:

```bash
cargo run -p nexusfs -- status --config ./nexusfs.toml
```

That prints the device id, the data directory, the head hash, the state root, operation
and pending counts, blob totals, and the admin token.

## Run The Daemon

```bash
cargo run -p nexusfs -- daemon --config ./nexusfs.toml
```

The daemon opens the store, creates a device identity if there is none, refuses to run
against an on-disk format it does not understand, bootstraps a head on first launch, and
serves the admin console on `admin.bind` — <http://127.0.0.1:7070> by default.

The embedded database takes an exclusive lock, so `status` cannot run while the daemon
holds the store. Query the running daemon through the admin API instead.

## Turn On More Of It

Replication, the S3 facade and the console are feature flags on one binary:

```bash
cargo build -p nexusfs --features "admin,quic,s3"
```

- `admin` (default): the embedded admin API and console
- `quic`: QUIC transport and peer replication
- `s3`: the S3-compatible HTTP facade
- `posix`, `zk`: stubs, off by default

Two nodes replicating locally:

```bash
./scripts/dev_run_two_nodes.sh
```

That seeds two nodes with different content while both are stopped, starts them as
peers, and waits until they report the same state root — including the deterministically
renamed copy of the directory each created independently.

## Try It Without Installing Anything

The [playground](https://minh.systems/NexusFS/playground.html) runs this exact core
compiled to WebAssembly: two replicas in one browser tab, a partition, and deterministic
convergence. Same operations, same CRDT state, same conflict rules — only the storage
backend differs.

## Where To Go Next

- [Operations Guide](./operations.md) — the admin API, maintenance commands, upgrading,
  enrolling peers, and proving state to someone else
- [Current Status](./current-status.md) — what is implemented, precisely
- [Architecture Overview](./architecture.md) — the crate layout and the invariants
