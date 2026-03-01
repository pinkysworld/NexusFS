# Filesystem Operation Semantics (v0 spec)

This doc defines the intended semantics for `FsOpKind` operations so that:
- POSIX façade and S3 façade behave consistently
- CRDT merges converge deterministically
- proofs can target clear pre/post conditions

---

## 1) General rules

- Every mutating operation is appended to oplog and is signed.
- Apply operations idempotently (by OpId).
- If preconditions fail locally (e.g., missing parent), store as pending until dependencies arrive.

---

## 2) Operation definitions

### 2.1 Mkdir { parent, name, mode }
**Preconditions**
- `parent` exists and is a directory
- `name` not empty and contains no path separators

**Postconditions**
- a new `inode_id` is allocated for the directory
- directory entry `name -> inode_id` is added to parent directory map

**Conflicts**
- if another inode concurrently claims same `name`:
  - deterministic rename using conflict suffix

---

### 2.2 CreateFile { parent, name, mode }
Same as Mkdir but creates a file inode.

---

### 2.3 Write { inode, offset, data_hashes, new_size }
**Preconditions**
- `inode` exists and is a file
- referenced blobs exist in CAS OR will be fetched (apply may be pending)

**Postconditions**
- file head register updated to point to new FileNode
- FileNode contains chunk refs reflecting new content

**Conflicts**
- concurrent writes produce MV versions:
  - surface as conflict copies OR keep multiple heads in MV-register

---

### 2.4 Rename { old_parent, old_name, new_parent, new_name }
**Preconditions**
- `old_parent/old_name` exists (or will exist)
- `new_parent` exists

**Postconditions**
- remove old entry, add new entry referencing same inode

**Conflicts**
- rename vs unlink:
  - if causally after unlink, rename is a no-op (or creates conflict artifact)
  - if concurrent, keep renamed entry with suffix

---

### 2.5 Unlink { parent, name }
**Postconditions**
- remove directory entry
- inode is GC-eligible if no references remain (future)

---

## 3) Deterministic conflict naming

See `crates/crdt/src/conflicts.rs`.

Invariant: every replica that sees the same conflict MUST derive the same name.
