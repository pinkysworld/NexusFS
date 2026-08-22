# Admin API

The admin server is embedded in the daemon, enabled by the `admin` feature (on by
default), and bound to `admin.bind`. It serves:

- the static console at `/` and `/app.js`
- JSON endpoints under `/api/*`

Every endpoint requires `x-nexusfs-token: <token>`, from `admin.token` in the config or
generated on first start. `nexusfs status` prints the token.

**The API is read-only.** Every state-changing maintenance action needs the store's
exclusive lock, which the running daemon holds, so those live at the CLI — and trust
changes are deliberately not one click away in a browser.

---

## State

### GET `/api/status`
Head, state root, device id, operation and pending counts, wall clock, on-disk format
version.

### GET `/api/fs/head`
The current head hash on its own.

### GET `/api/identity`
This node's device id and public key — what to enrol on another node.

---

## Filesystem

### GET `/api/fs/ls?path=`
A directory listing from live namespace state: name, inode, kind, size.

### GET `/api/fs/cat?path=`
File contents, capped and binary-aware. If this node deferred the file's content, the
read fetches it from a peer first, and answers `503` if it still cannot be had — rather
than serving an empty file, which is what a parked write would otherwise produce.

`403` means the file is encrypted and this node is not a recipient: it holds the bytes
and cannot decrypt them. That is a refusal, not a failure, which is why it is not a
`500` — and it usually means the sealing key was never enrolled, since
trust-on-first-use pins only the signing key.

### GET `/api/fs/proof?path=`
A self-contained inclusion proof for one entry: the inode, its object hash, and the
sibling hashes up to the root. The same structure `nexusfs prove` writes, and
`nexusfs check-proof` verifies without opening a repository.

---

## Operation log

### GET `/api/oplog/summary`
Per-device replication progress — the highest observed counter for each device.

### GET `/api/oplog/recent?limit=`
The most recent operations.

---

## Storage

### GET `/api/storage/stats`
Blob count and total bytes.

### GET `/api/storage/gc`
A survey of unreachable storage. **Never deletes**, because the daemon can write a blob
between the mark and the sweep and that blob would look like garbage. Use
`nexusfs gc --apply` for the real thing.

---

## Replication

### GET `/api/peers`
Per-peer sync state: last attempt, last success, last error, operations and blobs
received, content bytes, whether the last pass deferred content, and the number of
completed syncs.

### GET `/api/peers/enrolled`
The pinned trust list.

These two answer different questions. The first lists sync *targets* and how they are
doing; the second lists *trusted keys*. A device can be trusted without being a target,
and a target may not be trusted yet — which is exactly the mismatch worth noticing when
replication is silently doing nothing.

---

## Energy

### GET `/api/energy`
The current reading (power source, charge, temperature, CPU load, link cost, free space
where the store lives, sample time), the budget it produced (sync, content, byte ceiling,
interval multiplier), and a `reason` naming the rule that fired.

`storage_free_bytes` is `null` when it could not be read, which is not the same as zero —
the scheduler treats unknown as unconstrained, so rendering it as `0 B` would suggest the
opposite. `nexusfs status` prints the same decision, which is the only route on a build
without the admin feature.

The daemon samples once per pass and caches it, so this explains the pass that actually
ran rather than a fresh reading that could contradict it.

```json
{"enabled":true,"power":"battery","battery_pct":14,"temp_c":null,
 "link":"unknown","storage_free_bytes":233692033024,"sync":true,"content":false,
 "max_content_bytes":0,"interval_scale":2.0,
 "reason":"battery 14% is at or below the low threshold 20%"}
```

---

## Security

### GET `/api/security`
Encryption state, proof coverage, and the same audit report `nexusfs verify` prints:
every signature, every proof's structure, and a read of every file. Runs on request
rather than on refresh, because it reads the whole repository.
