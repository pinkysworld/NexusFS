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

The admin service listens on `admin.bind` and authenticates with the `x-nexusfs-token`
header. It serves a small console plus a JSON API:

| Endpoint | Shows |
| --- | --- |
| `/api/status` | head, state root, device id, operation and pending counts |
| `/api/fs/head` | the current head hash |
| `/api/fs/ls?path=` | a directory listing from live namespace state |
| `/api/oplog/summary` | applied-operation clock summary |
| `/api/oplog/recent?limit=` | the most recent operations |
| `/api/storage/stats` | blob count and bytes |
| `/api/peers` | per-peer sync status, errors, and transfer counters |
| `/api/security` | the same report `nexusfs verify` prints |
| `/api/energy` | the current power reading, the replication budget, and why |

`/api/energy` is the one to reach for when replication looks slower than expected: its
`reason` field names the rule that fired, so "battery 14% is at or below the low
threshold 20%" distinguishes a deliberate throttle from a broken peer. Peers also report
`content_deferred`, which separates "up to date" from "namespace current, bytes pending".

Note that the embedded database takes an exclusive lock, so the `status` CLI command
cannot run while the daemon holds the store. Query the running daemon instead.

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
