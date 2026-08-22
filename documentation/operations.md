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
| `/api/fs/proof?path=` | a self-contained inclusion proof for one entry |
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

A read of a file whose content this node deferred will fetch that content from a peer
first, and answer `503` if it cannot — rather than serving an empty file, which is what a
parked write would otherwise produce.

`/api/energy` is the one to reach for when replication looks slower than expected: its
`reason` field names the rule that fired, so "battery 14% is at or below the low
threshold 20%" distinguishes a deliberate throttle from a broken peer. Peers also report
`content_deferred`, which separates "up to date" from "namespace current, bytes pending".

`nexusfs status` prints the same reading and budget, which is the only route on a build
without the admin feature — and that is the build most likely to be running on a device
that throttles. It reports even when scheduling is switched off, because "nothing is
throttling this" and "throttling is disabled" are different answers to the same
question.

Note that the embedded database takes an exclusive lock, so the `status` CLI command
cannot run while the daemon holds the store. Query the running daemon instead.

## Maintenance Commands

| Command | Purpose |
| --- | --- |
| `nexusfs status` | head, state root, operation and pending counts, blob totals, and the current power reading with the replication budget it produced |
| `nexusfs verify` | every signature and proof, and a read of every file |
| `nexusfs gc` | survey unreachable storage; `--apply` to reclaim it |
| `nexusfs migrate` | upgrade the on-disk format |
| `nexusfs peer identity` | this node's device id and public key |
| `nexusfs peer list/add/remove` | manage which peers are trusted |
| `nexusfs share` | re-seal existing files to the peers enrolled now; `--apply` to write |
| `nexusfs prove <path>` | emit a proof that a path holds its current content |
| `nexusfs check-proof <file>` | verify one, without opening any repository |

All of them take the store's exclusive lock, so none can run while the daemon holds it.
That is also why `/api/storage/gc` only surveys: a running daemon can write a blob
between the mark and the sweep, and that blob would look like garbage.

`verify` exits non-zero on failure, so it works directly as a cron or CI check.

### Upgrading

The repository records the on-disk format it was written with, and a build that does not
match refuses to open it — older with a pointer to `nexusfs migrate`, newer with no way
forward at all. Opening never migrates by itself, because a migration rewrites records
in place. Back up the data directory first.

### Proving state to someone else

The state root is a Merkle commitment, so a single entry can be proved on its own:

```
nexusfs prove --config ./nexusfs.toml /docs/note.txt --out note.json
nexusfs check-proof note.json --root <root-you-obtained-independently>
```

`check-proof` opens no repository — if it needed one, the proof would not be establishing
anything the holder could not already see. Supply `--root` from a source you trust;
without it the command checks the proof against the root recorded inside itself and says
plainly that this proves only internal consistency.

Read its output carefully, because it distinguishes two things that look alike. The
`subject` line is the inode the *proof* commits to. The `path`, `inode` and `issuer`
lines beneath it are labels the proof file carries and the proof does not cover — anyone
can edit them — so they are printed as unverified, and a WARNING appears when a label
disagrees with the proof's actual subject. A genuine proof relabelled to name a different
file is exactly the attack this separation exists to defeat.

Proving something is *gone* works the same way but is asked by inode, since a path that
resolves to nothing has no inode to name:

```
nexusfs prove --config ./nexusfs.toml --inode <inode> --out gone.json
nexusfs check-proof gone.json --root <root-after-the-deletion>
```

An inclusion proof against the earlier root and an absence proof against the later one
together demonstrate the deletion to someone holding neither state.

### Enrolling peers

Leave `net.tofu = true` only where first contact cannot be intercepted. Otherwise set it
false and enrol keys ahead of time: run `nexusfs peer identity` on each node, and give
the printed command to the other. Changing a known device's key requires `--rotate`, so
an unexpected key is always noticed rather than silently accepted.

`peer identity` prints two keys. The ed25519 one decides whether a session is accepted;
the X25519 *sealing* key decides whether that peer can read encrypted content. `peer add`
takes both. A peer enrolled with only the first replicates and verifies normally and is
simply not a recipient — `peer list` and the console both say so rather than leaving a
blank.

### Sharing existing files with a peer enrolled later

Enrolment affects what is written afterwards. Files already on disk carry envelopes for
whoever was enrolled when they were written, so a new peer replicates them, verifies
them, and cannot read a byte.

```
nexusfs share --config ./nexusfs.toml            # survey
nexusfs share --config ./nexusfs.toml --apply    # re-seal
```

Each re-sealed file emits one signed `Write` carrying the same chunks and a new set of
envelopes, so it replicates and converges like any other operation. Files this node
cannot itself read are counted and skipped: without the file key there is nothing to
re-seal with.

**This grants access and never withdraws it.** The ciphertext does not change, so a
device that once held an envelope — or the repository key a file was sealed with — can
still decrypt what it kept. Running `share` after *removing* a peer does nothing at all.
Withdrawing access means re-encrypting the content under a fresh key, which is not built.

### What to back up

`identity.toml` is the file that opens your content when per-recipient sealing is in use,
and it is also what signs this device's operations. `repo.key` still matters for files
written before sealing existed. Both are in the data directory and written owner-only.

## Development Workflow

Recommended day-to-day workflow:

1. Build with `cargo build -p nexusfs`
2. Run focused crate tests while developing
3. Run workspace tests after each meaningful slice
4. Exercise the daemon with the sample config
5. Keep documentation in sync with any new operational behavior

## Expected Evolution

Peer enrolment, migration flows and proof verification tooling are covered above. What
this guide still cannot tell you how to do:

- **Backup and restore.** There is no supported procedure beyond copying the data
  directory while the daemon is stopped, and no tested restore path.
- **Storage compaction.** `gc` reclaims unreachable blobs; nothing compacts the
  underlying database, and orphaned inode and directory records are left behind.
- **Closing the two gaps `energy.link_cost` works around.** Detection cannot see a Wi-Fi
  hotspot on macOS, or through a VPN to the link beneath it.
