# Object Formats and Canonical Hashing

This document defines how NexusFS objects are encoded and hashed.

> Goal: if two nodes create the same logical object, they compute the same hash.

---

## 1) Canonical encoding

NexusFS v0 uses:
- `postcard` encoding (deterministic, compact)
- strictly versioned objects with `ObjectHeader { type_tag, version }`

Rules:
1. **No unordered maps** in hashed objects.
   - Use sorted `Vec` or `BTreeMap`.
2. Directory entries must be **sorted by name**.
3. All integers are fixed-width (u64/u32/etc).
4. No floats.

If you change a field list or semantics, bump `ObjectHeader.version`.

---

## 2) Hashing

- `Hash = BLAKE3( encode(object) )`
- For raw blobs (chunks):
  - `Hash = BLAKE3( blob_bytes )`

### Why BLAKE3?
- fast on CPUs found in edge devices
- good parallelism and SIMD support

---

## 3) Type tags

Reserved `type_tag` values (v0):

- `1` = FileNode
- `2` = DirNode
- `3` = SnapshotRoot
- `4` = PolicyBlob (future)
- `5` = ZkRootState (future)
- `6` = AuditCheckpoint (future)

---

## 4) Snapshot roots

A snapshot root is a commit of a filesystem view.
v0 skeleton stores:
- `root_dir_inode`
- `inode_map_root` (optional placeholder)

Future versions should include:
- inode map Merkle root (for efficient membership proofs)
- policy root (Wasm policy commitments)
- optional ZK Poseidon roots

---

## 5) Bridging BLAKE3 and SNARK-friendly commitments (ZK design note)

ZK circuits generally prefer Poseidon-based hashing.

Recommended approach:
- keep **BLAKE3** for CAS addressing and normal integrity verification
- also store **Poseidon commitments** for ZK:
  - `zk_commit = Poseidon(ciphertext || metadata)` (or plaintext, depending on design)
- ZK proofs operate over Poseidon roots
- The system binds `zk_commit` ↔ `cas_hash` by storing both in the op or metadata
  and requiring signatures (and eventually proofs) over both.

This avoids in-circuit BLAKE3 while still keeping a strong integrity chain.
