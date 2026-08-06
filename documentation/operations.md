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
| `/api/status` | head, state root, device id, counts, on-disk format version |
| `/api/identity` | this node's device id and public key, for enrolling elsewhere |
| `/api/fs/head` | the current head hash |
| `/api/fs/ls?path=` | a directory listing from live namespace state |
| `/api/fs/cat?path=` | file contents, capped and binary-aware |
| `/api/oplog/summary` | per-device replication progress |
| `/api/oplog/recent?limit=` | the most recent operations |
| `/api/storage/stats` | blob count and bytes |
| `/api/peers` | per-peer sync status, errors, and transfer counters |
| `/api/peers/enrolled` | the pinned trust list |
| `/api/security` | the same report `nexusfs verify` prints |
| `/api/energy` | the current power reading, the replication budget, and why |
| `/api/storage/gc` | a survey of unreachable storage (never deletes) |

The console groups these into five tabs — Overview, Files, Replication, Operations and
Maintenance. It is **read-only by design**. Every state-changing maintenance action
needs the store's exclusive lock, which the running daemon holds, so those live at the
CLI; and trust changes are deliberately not one click away in a browser.

Two panels run on a button rather than on refresh: the integrity audit reads every file,
and the collection survey walks the whole namespace. Everything else is cheap enough for
the optional five-second auto-refresh.

Note that `/api/peers` and `/api/peers/enrolled` answer different questions. The first
lists sync *targets* and how they are doing; the second lists *trusted keys*. A device
can be trusted without being a target, and a target may not be trusted yet — which is
exactly the mismatch worth noticing when replication is silently doing nothing.

`/api/energy` is the one to reach for when replication looks slower than expected: its
`reason` field names the rule that fired, so "battery 14% is at or below the low
threshold 20%" distinguishes a deliberate throttle from a broken peer. Peers also report
`content_deferred`, which separates "up to date" from "namespace current, bytes pending".

Note that the embedded database takes an exclusive lock, so the `status` CLI command
cannot run while the daemon holds the store. Query the running daemon instead.

## Maintenance Commands

| Command | Purpose |
| --- | --- |
| `nexusfs status` | head, state root, operation and pending counts, blob totals |
| `nexusfs verify` | every signature and proof, and a read of every file |
| `nexusfs gc` | survey unreachable storage; `--apply` to reclaim it |
| `nexusfs migrate` | upgrade the on-disk format |
| `nexusfs peer identity` | this node's device id and public key |
| `nexusfs peer list/add/remove` | manage which peers are trusted |

All of them take the store's exclusive lock, so none can run while the daemon holds it.
That is also why `/api/storage/gc` only surveys: a running daemon can write a blob
between the mark and the sweep, and that blob would look like garbage.

`verify` exits non-zero on failure, so it works directly as a cron or CI check.

### Upgrading

The repository records the on-disk format it was written with, and a build that does not
match refuses to open it — older with a pointer to `nexusfs migrate`, newer with no way
forward at all. Opening never migrates by itself, because a migration rewrites records
in place. Back up the data directory first.

### Enrolling peers

Leave `net.tofu = true` only where first contact cannot be intercepted. Otherwise set it
false and enrol keys ahead of time: run `nexusfs peer identity` on each node, and give
the printed command to the other. Changing a known device's key requires `--rotate`, so
an unexpected key is always noticed rather than silently accepted.

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
