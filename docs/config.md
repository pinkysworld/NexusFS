# Configuration Reference

NexusFS uses a single TOML config file. The annotated baseline is
`examples/nexusfs.toml`; this page is the field-by-field reference.

---

## `[node]`
- `data_dir`: directory for the local database and content-addressed store. A leading
  `~` is expanded. **Keep this outside any cloud-synced folder** — a sync daemon
  rewriting files under a live embedded database can corrupt it.
- `device_name`: cosmetic label for UI and logs.

## `[net]`
- `listen`: QUIC listen address, e.g. `127.0.0.1:4444`.
- `peers`: addresses to pull from, as plain `host:port` strings — not URLs, and not
  scheme-prefixed. An unparseable entry is skipped with a warning.
- `tofu`: trust an unknown device's key the first time it connects and pin it. Leave
  this on only where first contact cannot be intercepted; otherwise set it false and
  enrol keys with `nexusfs peer add`.
- `sync_interval_secs`: seconds between pulls from each peer. Default 15. The energy
  scheduler multiplies this when it decides to back off.

Replication only runs when the binary is built with the `quic` feature. A malformed
`listen` address takes replication down and leaves the rest of the daemon running,
because a wrong config is when an operator most wants the console.

## `[admin]`
- `bind`: HTTP bind address. Default `127.0.0.1:7070`.
- `token`: admin token, required in the `x-nexusfs-token` header. Generated and
  persisted on first start when empty.

**Security note:** binding to `0.0.0.0` exposes the admin surface to the network. The
console is read-only, but the API reveals the whole namespace — keep it on loopback
unless you have another layer in front of it.

## `[s3]`
- `enabled`: start the S3-compatible API server. Needs the `s3` feature.
- `bind`: bind address, e.g. `127.0.0.1:9000`.
- `token`: shared secret required in `x-nexusfs-token`. Empty disables the check
  entirely, so leave the facade on loopback in that case. The daemon warns when it is
  unset.

## `[posix]`
- `enabled`: enable the FUSE mount helper. Stub — the facade is not implemented.
- `mountpoint`: mount path.

## `[security]`
- `encrypt_at_rest`: encrypt chunk content before it is written. The repository key is
  created at `data_dir/repo.key` on first use; **back it up**, because without it the
  content is unrecoverable. Replicas need the same key to read each other's files.
- `proof_mode`: one of
  - `none` — signatures and hashes only
  - `transparent` — attach signed evidence to local operations, and reject malformed
    evidence on receipt
  - `required` — as `transparent`, and additionally refuse operations carrying no proof
  - `zk_commit` — attach a Merkle inclusion path for the entry each operation is about,
    so a receiver can check the claim against the root without holding the author's
    prior state. Transparent bundles are still accepted, so a cluster can be upgraded a
    node at a time
  - `zk_full` — accepted and behaves as `none`. There is no proving system

  `zk_commit` is a commitment scheme, not zero-knowledge: a verifier learns the inode
  and its object hash, just not the rest of the tree.

## `[energy]`
- `enabled`: adapt replication to the device. Set false to remove every limit while
  still reporting the reading at `/api/energy`.
- `battery_low_pct`: at or below this charge, replication takes operations and defers
  content. At or below a quarter of it, replication stops entirely.
- `temp_high_c`: at or above this temperature, content is deferred regardless of charge.
- `link_cost`: `auto` (the default), `metered`, `unmetered`, or `unknown`. `auto`
  detects, and detection is partial by platform — NetworkManager answers properly on
  Linux, macOS recognises only a USB phone tether, and everything else reports unknown.
  A VPN defeats it everywhere, because the default route names the tunnel and the cost
  belongs to the link underneath. State the value explicitly where detection cannot see
  your situation; an operator knows their plan better than a probe does.
- `storage_reserve_mb`: megabytes of free space to leave alone on the filesystem holding
  `data_dir`. Default 1024. Not a throttle threshold but a floor — replication is a
  background job filling someone else's disk. Content is held to the room above it and
  stops entirely at it; operations keep flowing, so the node still tracks what exists.
  Free space is read for the store's filesystem, not for `/`, and an unreadable answer
  constrains nothing.

Heat and metered links override the battery grade rather than folding into it. Every
reading is three-state, and **unknown never constrains** — a server with no battery
sensor is not a device at 0%, and "we have no way to ask" is not "we asked and it is
free".

---

## On-disk format

The store records the format version it was written with, independently of this file. A
build that finds an older one refuses to open it and names `nexusfs migrate`; a build
that finds a newer one refuses and cannot be forced. The current version is 2.
