//! Operation application: the single place where filesystem state changes.
//!
//! Local writes and (later) replicated writes both land here, so there is exactly one
//! definition of what an operation means. Every op is signature-checked before it can
//! touch state, and operations whose preconditions are not yet satisfiable are parked
//! rather than rejected — an offline-first system routinely sees a child arrive
//! before its parent.

use anyhow::{bail, Context, Result};

use nexusfs_crdt::lww::LwwReg;
use nexusfs_crdt::or_map::Dot;
use nexusfs_crypto::{sign, verify, Identity};
use nexusfs_proto::{CausalCtx, ChunkRef, DeviceId, FsOp, FsOpKind, Hash, OpId};

use crate::codec::decode;
use crate::inode::{dot_for_op, inode_for_op, ROOT_INODE};
use crate::namespace::{
    validate_name, AttrState, ContentState, DirEntryValue, InodeRecord, CF_STATE,
};
use crate::object::EntryType;
use crate::state::{CoreState, CF_OPLOG};

const PENDING_PREFIX: &[u8] = b"pending\0";

/// Guard against a pathological rename chain walking forever.
const MAX_SUBTREE_NODES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    AlreadyApplied,
    /// Preconditions are not satisfiable yet; the op is stored and will be retried
    /// whenever later operations arrive.
    Pending(String),
}

/// Result of attempting the state mutation for one op.
enum Mutation {
    Done,
    Unmet(String),
}

fn pending_key(op_id: OpId) -> Vec<u8> {
    let mut k = Vec::with_capacity(PENDING_PREFIX.len() + 24);
    k.extend_from_slice(PENDING_PREFIX);
    k.extend_from_slice(&op_id.device_id.0.to_be_bytes());
    k.extend_from_slice(&op_id.counter.to_be_bytes());
    k
}

/// True when `chunks` cover the file's plaintext exactly once, start to end.
///
/// Measured in plaintext, not stored bytes: an encrypted chunk is longer than the
/// content it holds, so summing stored lengths would never reach `new_size` and every
/// encrypted write would be misrouted to the splice path.
fn chunks_form_whole_file(chunks: &[ChunkRef], new_size: u64) -> bool {
    let mut expected = 0u64;
    for c in chunks {
        if c.offset != expected {
            return false;
        }
        expected += c.plain_len as u64;
    }
    expected == new_size
}

/// Whether `op` could still win `current`, using the register's own ordering.
///
/// Mirrors `LwwReg::merge`, which keeps the incoming value only on a strictly greater
/// key. Anything at or below the current key is already superseded, and applying it
/// would leave state untouched.
fn write_can_win<T>(current: &LwwReg<T>, op: &FsOp) -> bool {
    (op.time_unix_ms, op.id.device_id.0, op.id.counter)
        > (current.ts, current.writer_id, current.seq)
}

impl CoreState {
    // ---- op construction and verification -------------------------------------

    /// Build a signed operation authored by this device.
    pub fn make_op(&self, identity: &Identity, kind: FsOpKind, now: u64) -> Result<FsOp> {
        let counter = self.next_op_counter()?;
        let mut op = FsOp {
            id: OpId {
                device_id: self.device_id,
                counter,
            },
            time_unix_ms: now,
            ctx: CausalCtx { deps: vec![] },
            kind,
            author_pubkey: identity.pubkey_bytes(),
            sig: Vec::new(),
            proof: None,
        };
        op.sig = sign(identity.signing_key(), &op.signing_bytes()?);
        Ok(op)
    }

    /// Enforce invariant I3/I5: nothing mutates state without a valid signature.
    pub fn verify_op(&self, op: &FsOp) -> Result<()> {
        if op.sig.is_empty() {
            bail!(
                "operation {:x}/{} is unsigned",
                op.id.device_id.0,
                op.id.counter
            );
        }
        let bytes = op.signing_bytes()?;
        verify(&op.author_pubkey, &bytes, &op.sig).with_context(|| {
            format!(
                "signature verification failed for op {:x}/{}",
                op.id.device_id.0, op.id.counter
            )
        })
    }

    // ---- the apply entry point --------------------------------------------------

    /// Verify, apply, and re-snapshot.
    ///
    /// Crash safety comes from the CRDT rather than from transactions: OR-Map adds are
    /// keyed by `(name, dot)` and LWW merges are idempotent, so replaying an op whose
    /// state write landed but whose applied-marker did not is a no-op.
    pub fn apply_op(&self, op: &FsOp) -> Result<ApplyOutcome> {
        self.apply_op_inner(op, true)
    }

    /// Apply without the proof-policy gate.
    ///
    /// Used only by `apply_local`, whose provisional operation cannot carry evidence
    /// yet: the proof records the state root the operation produces, which does not
    /// exist until it has been applied. Structure checks still run on the final,
    /// proven version that gets stored.
    pub(crate) fn apply_op_unproven(&self, op: &FsOp) -> Result<ApplyOutcome> {
        self.apply_op_inner(op, false)
    }

    fn apply_op_inner(&self, op: &FsOp, check_policy: bool) -> Result<ApplyOutcome> {
        self.verify_op(op)?;
        // Structure is checked before anything is stored, so malformed evidence is
        // rejected rather than retained alongside state it purports to describe.
        if check_policy {
            self.check_proof(op, self.proofs)?;
        }

        if self.is_op_applied(op.id)? {
            return Ok(ApplyOutcome::AlreadyApplied);
        }

        // Retain the op even if it cannot be applied yet: we have it, so it belongs in
        // our clock summary and should be offered to peers.
        self.append_op(op)?;

        let outcome = match self.mutate(op)? {
            Mutation::Done => {
                self.mark_op_applied(op.id)?;
                self.clear_pending(op.id)?;
                self.observe_time(op.time_unix_ms)?;
                ApplyOutcome::Applied
            }
            Mutation::Unmet(reason) => {
                self.store_pending(op)?;
                ApplyOutcome::Pending(reason)
            }
        };

        // A newly applied op may have unblocked ops parked earlier.
        let unblocked = self.drain_pending()?;

        if outcome == ApplyOutcome::Applied || unblocked > 0 {
            self.build_snapshot()?;
        }
        Ok(outcome)
    }

    // ---- pending queue ----------------------------------------------------------

    fn store_pending(&self, op: &FsOp) -> Result<()> {
        let bytes = crate::codec::encode(op)?;
        self.stores
            .kv
            .put_kv(CF_OPLOG, &pending_key(op.id), &bytes)?;
        Ok(())
    }

    fn clear_pending(&self, op_id: OpId) -> Result<()> {
        self.stores.kv.delete_kv(CF_OPLOG, &pending_key(op_id))?;
        Ok(())
    }

    pub fn pending_count(&self) -> Result<usize> {
        Ok(self.stores.kv.scan_prefix(CF_OPLOG, PENDING_PREFIX)?.len())
    }

    /// Operations parked awaiting dependencies.
    pub fn pending_ops(&self) -> Result<Vec<FsOp>> {
        self.stores
            .kv
            .scan_prefix(CF_OPLOG, PENDING_PREFIX)?
            .into_iter()
            .map(|(_k, v)| decode::<FsOp>(&v).context("decode pending op"))
            .collect()
    }

    /// Content hashes referenced by parked operations but absent from the local store.
    ///
    /// This is precisely the set a peer needs to send to unblock us, which is why
    /// replication asks for it rather than guessing from the whole oplog.
    pub fn missing_chunk_hashes(&self) -> Result<Vec<Hash>> {
        let mut wanted = std::collections::BTreeSet::new();

        for op in self.pending_ops()? {
            if let FsOpKind::Write { chunks, .. } = &op.kind {
                for chunk in chunks {
                    if !self.stores.blobs.has(&chunk.hash)? {
                        wanted.insert(chunk.hash);
                    }
                }
            }
        }
        Ok(wanted.into_iter().collect())
    }

    /// Retry parked operations and re-snapshot if any applied.
    ///
    /// `apply_op` drains automatically, which covers ops unblocked by other ops. This
    /// is the hook for the other trigger: blobs arriving without any new operation, as
    /// blob transfer will do. Returns how many operations became applicable.
    pub fn retry_pending(&self) -> Result<usize> {
        let applied = self.drain_pending()?;
        if applied > 0 {
            self.build_snapshot()?;
        }
        Ok(applied)
    }

    /// Retry parked operations until a full pass makes no progress.
    ///
    /// Returns how many became applicable.
    fn drain_pending(&self) -> Result<usize> {
        let mut total = 0usize;
        loop {
            let rows = self.stores.kv.scan_prefix(CF_OPLOG, PENDING_PREFIX)?;
            if rows.is_empty() {
                break;
            }

            let mut progressed = false;
            for (_key, value) in rows {
                let op: FsOp = decode(&value).context("decode pending op")?;

                if self.is_op_applied(op.id)? {
                    self.clear_pending(op.id)?;
                    progressed = true;
                    continue;
                }

                if let Mutation::Done = self.mutate(&op)? {
                    self.mark_op_applied(op.id)?;
                    self.clear_pending(op.id)?;
                    self.observe_time(op.time_unix_ms)?;
                    progressed = true;
                    total += 1;
                }
            }

            if !progressed {
                break;
            }
        }
        Ok(total)
    }

    // ---- mutation ---------------------------------------------------------------

    fn mutate(&self, op: &FsOp) -> Result<Mutation> {
        match &op.kind {
            FsOpKind::Mkdir { parent, name, mode } => {
                self.mutate_create(op, *parent, name, *mode, EntryType::Dir)
            }
            FsOpKind::CreateFile { parent, name, mode } => {
                self.mutate_create(op, *parent, name, *mode, EntryType::File)
            }
            FsOpKind::Write {
                inode,
                offset,
                chunks,
                new_size,
                encryption,
            } => self.mutate_write(op, *inode, *offset, chunks, *new_size, encryption.clone()),
            FsOpKind::Rename {
                old_parent,
                old_name,
                new_parent,
                new_name,
                observed,
            } => self.mutate_rename(op, *old_parent, old_name, *new_parent, new_name, observed),
            FsOpKind::Unlink {
                parent,
                name,
                observed,
            } => self.mutate_unlink(op, *parent, name, observed),
            FsOpKind::SetAttr {
                inode,
                mode,
                uid,
                gid,
            } => self.mutate_setattr(op, *inode, *mode, *uid, *gid),
        }
    }

    fn mutate_create(
        &self,
        op: &FsOp,
        parent: u128,
        name: &str,
        mode: u32,
        kind: EntryType,
    ) -> Result<Mutation> {
        validate_name(name)?;

        if !self.is_dir(parent)? {
            return Ok(Mutation::Unmet(format!(
                "parent inode {parent:x} does not exist or is not a directory"
            )));
        }

        let new_inode = inode_for_op(op.id);
        let record = InodeRecord::new(
            kind,
            mode,
            op.time_unix_ms,
            op.id.device_id.0,
            op.id.counter,
        );
        self.store_inode(new_inode, &record)?;

        if kind == EntryType::Dir {
            self.store_dir(new_inode, &Default::default())?;
        }

        let mut map = self.load_dir(parent)?.unwrap_or_default();
        map.add(
            name.to_string(),
            dot_for_op(op.id),
            DirEntryValue {
                inode_id: new_inode,
                entry_type: kind,
                created_unix_ms: op.time_unix_ms,
            },
        );
        self.store_dir(parent, &map)?;

        Ok(Mutation::Done)
    }

    fn mutate_write(
        &self,
        op: &FsOp,
        inode: u128,
        offset: u64,
        chunks: &[ChunkRef],
        new_size: u64,
        encryption: Option<Vec<u8>>,
    ) -> Result<Mutation> {
        let Some(mut record) = self.load_inode(inode)? else {
            return Ok(Mutation::Unmet(format!("inode {inode:x} does not exist")));
        };
        if record.kind != EntryType::File {
            bail!("inode {inode:x} is not a file");
        }

        // A write that cannot win the content register contributes nothing to state, so
        // there is no reason to demand its bytes. Checked before availability, because
        // otherwise an overwritten version parks forever waiting for content no reader
        // will ever see — and a peer that has since collected that content as garbage
        // could never satisfy the request, leaving the two nodes permanently mid-sync
        // despite agreeing on every byte of live state.
        //
        // The comparison is over the same `(ts, writer_id, seq)` key `merge` uses, so
        // every replica reaches the same verdict without coordinating.
        if !write_can_win(&record.content, op) {
            return Ok(Mutation::Done);
        }

        // Both paths below require the incoming chunks to be present locally. Applying
        // a write whose blobs have not arrived would publish a file that reads back as
        // an error — a torn state that no caller can distinguish from corruption. Park
        // instead, per docs/ops_semantics.md §2.3, and let `drain_pending` retry once
        // the transfer lands.
        if let Some(missing) = self.first_missing_chunk(chunks)? {
            return Ok(Mutation::Unmet(format!(
                "write references chunk {} which has not been fetched yet",
                hex::encode(missing)
            )));
        }

        // Fast path: the op carries the whole file, so the chunk list *is* the file and
        // no bytes have to be read back to build it. Note the chunks are stored exactly
        // as received — encrypted content is never decrypted just to re-record it.
        let node_hash = if offset == 0 && chunks_form_whole_file(chunks, new_size) {
            self.make_filenode(
                chunks.to_vec(),
                new_size,
                op.time_unix_ms,
                encryption.map(|sealed_key| crate::object::FileEncryption { sealed_key }),
            )?
        } else {
            // Partial write: we must splice, which needs the existing bytes and the
            // incoming bytes locally. If either is missing, park the op.
            let Some(existing) = self.try_read_file(inode)? else {
                return Ok(Mutation::Unmet(format!(
                    "inode {inode:x} has chunks that are not available locally yet"
                )));
            };
            let mut incoming = Vec::new();
            for chunk in chunks {
                let bytes = self
                    .stores
                    .blobs
                    .get(&chunk.hash)?
                    .context("chunk vanished after the availability check")?;
                incoming.extend_from_slice(&bytes);
            }

            // The buffer has to be justified by data actually in hand: the bytes the
            // file already had, or the bytes this write supplies at its offset. A
            // `new_size` beyond both is an assertion of zero padding no writer here
            // produces, and honouring it would allocate whatever a peer asked for —
            // `new_size` arrives inside a signed operation, but a signature only proves
            // authorship, not good faith.
            let justified = (existing.len() as u64).max(
                offset
                    .checked_add(incoming.len() as u64)
                    .context("write offset plus length overflows")?,
            );
            if new_size > justified {
                bail!(
                    "write claims size {new_size} but supplies only {justified} bytes \
                     of content"
                );
            }

            // usize is 32-bit on wasm32, where a silent truncation here would make the
            // browser build compute a different file than a native replica — and the
            // state root would disagree.
            let total = usize::try_from(new_size).context("file too large for this platform")?;

            let mut buffer = vec![0u8; total];
            let keep = (existing.len() as u64).min(new_size) as usize;
            buffer[..keep].copy_from_slice(&existing[..keep]);

            let start = usize::try_from(offset).context("offset too large for this platform")?;
            if start < buffer.len() {
                let end = start.saturating_add(incoming.len()).min(buffer.len());
                buffer[start..end].copy_from_slice(&incoming[..end - start]);
            }

            self.store_filenode_from_bytes(&buffer, op.time_unix_ms)?
        };

        let incoming = LwwReg::new(
            ContentState {
                node_hash: Some(node_hash),
                size: new_size,
                mtime_unix_ms: op.time_unix_ms,
            },
            op.time_unix_ms,
            op.id.device_id.0,
            op.id.counter,
        );

        // Merge rather than assign: a concurrent write that we already applied may
        // legitimately outrank this one, and both replicas must agree which.
        record.content.merge(&incoming);
        self.store_inode(inode, &record)?;
        Ok(Mutation::Done)
    }

    fn mutate_rename(
        &self,
        op: &FsOp,
        old_parent: u128,
        old_name: &str,
        new_parent: u128,
        new_name: &str,
        observed: &[OpId],
    ) -> Result<Mutation> {
        validate_name(new_name)?;

        if !self.is_dir(old_parent)? {
            return Ok(Mutation::Unmet(format!(
                "source parent {old_parent:x} does not exist or is not a directory"
            )));
        }
        if !self.is_dir(new_parent)? {
            return Ok(Mutation::Unmet(format!(
                "destination parent {new_parent:x} does not exist or is not a directory"
            )));
        }

        // Move the entry the author moved, identified by the dots it recorded — not
        // whatever currently ranks highest here. With a concurrent same-name creation
        // those differ, and picking locally would move a different inode on each
        // replica.
        let dots: Vec<Dot> = observed.iter().copied().map(dot_for_op).collect();
        let survivors = self
            .load_dir(old_parent)?
            .unwrap_or_default()
            .get_all(&old_name.to_string());

        let Some(entry) = survivors
            .iter()
            .filter(|(dot, _)| dots.contains(dot))
            .max_by_key(|(dot, _)| *dot)
            .map(|(_, value)| *value)
        else {
            if dots.is_empty() || survivors.iter().any(|(dot, _)| dots.contains(dot)) {
                // Nothing to move, and nothing outstanding that could change that.
                return Ok(Mutation::Done);
            }
            // The creation this rename observed has not arrived. Park rather than
            // dropping the rename, which would leave the entry under its old name here
            // and under the new one elsewhere.
            return Ok(Mutation::Unmet(format!(
                "rename source {old_name:?} has not been created here yet"
            )));
        };

        // Moving a directory beneath itself would detach the subtree from the root.
        if entry.entry_type == EntryType::Dir
            && self.subtree_contains(entry.inode_id, new_parent)?
        {
            bail!("cannot move a directory into its own subtree");
        }

        let value = DirEntryValue {
            inode_id: entry.inode_id,
            entry_type: entry.entry_type,
            created_unix_ms: op.time_unix_ms,
        };

        if old_parent == new_parent {
            let mut map = self.load_dir(old_parent)?.unwrap_or_default();
            map.remove_dots(&old_name.to_string(), dots.iter().copied());
            map.add(new_name.to_string(), dot_for_op(op.id), value);
            self.store_dir(old_parent, &map)?;
        } else {
            let mut source = self.load_dir(old_parent)?.unwrap_or_default();
            source.remove_dots(&old_name.to_string(), dots.iter().copied());
            self.store_dir(old_parent, &source)?;

            let mut dest = self.load_dir(new_parent)?.unwrap_or_default();
            dest.add(new_name.to_string(), dot_for_op(op.id), value);
            self.store_dir(new_parent, &dest)?;
        }

        Ok(Mutation::Done)
    }

    fn mutate_unlink(
        &self,
        _op: &FsOp,
        parent: u128,
        name: &str,
        observed: &[OpId],
    ) -> Result<Mutation> {
        if !self.is_dir(parent)? {
            return Ok(Mutation::Unmet(format!(
                "parent inode {parent:x} does not exist or is not a directory"
            )));
        }

        let mut map = self.load_dir(parent)?.unwrap_or_default();
        // Exactly what the author saw, not what happens to be here now. The dots may
        // not have arrived yet; recording them anyway is what makes the removal hold
        // when they do, and is why this needs no existence check.
        map.remove_dots(&name.to_string(), observed.iter().copied().map(dot_for_op));
        self.store_dir(parent, &map)?;
        Ok(Mutation::Done)
    }

    fn mutate_setattr(
        &self,
        op: &FsOp,
        inode: u128,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> Result<Mutation> {
        let Some(mut record) = self.load_inode(inode)? else {
            return Ok(Mutation::Unmet(format!("inode {inode:x} does not exist")));
        };

        let current = record.attrs.value;
        let incoming = LwwReg::new(
            AttrState {
                mode: mode.unwrap_or(current.mode),
                uid: uid.unwrap_or(current.uid),
                gid: gid.unwrap_or(current.gid),
            },
            op.time_unix_ms,
            op.id.device_id.0,
            op.id.counter,
        );
        record.attrs.merge(&incoming);
        self.store_inode(inode, &record)?;
        Ok(Mutation::Done)
    }

    // ---- helpers -----------------------------------------------------------------

    /// First referenced chunk that is not in the local CAS, if any.
    fn first_missing_chunk(&self, chunks: &[ChunkRef]) -> Result<Option<Hash>> {
        for chunk in chunks {
            if !self.stores.blobs.has(&chunk.hash)? {
                return Ok(Some(chunk.hash));
            }
        }
        Ok(None)
    }

    /// Read a file's bytes, or `None` if any chunk is not held locally.
    fn try_read_file(&self, inode: u128) -> Result<Option<Vec<u8>>> {
        let Some(record) = self.load_inode(inode)? else {
            return Ok(None);
        };
        let Some(node_hash) = record.content.value.node_hash else {
            return Ok(Some(Vec::new()));
        };
        let Some(crate::object::Object::FileNode(file)) = self.get_object(&node_hash)? else {
            return Ok(None);
        };
        for chunk in &file.chunks {
            if !self.stores.blobs.has(&chunk.hash)? {
                return Ok(None);
            }
        }
        Ok(Some(self.materialize_file(&file)?))
    }

    /// True when `target` lies within the subtree rooted at `root`.
    fn subtree_contains(&self, root: u128, target: u128) -> Result<bool> {
        if root == target {
            return Ok(true);
        }
        let mut stack = vec![root];
        let mut seen = std::collections::BTreeSet::new();
        let mut visited = 0usize;

        while let Some(inode) = stack.pop() {
            if !seen.insert(inode) {
                continue;
            }
            visited += 1;
            if visited > MAX_SUBTREE_NODES {
                bail!("directory tree walk exceeded {MAX_SUBTREE_NODES} nodes");
            }
            for entry in self.materialize_dir(inode)? {
                if entry.inode_id == target {
                    return Ok(true);
                }
                if entry.entry_type == EntryType::Dir {
                    stack.push(entry.inode_id);
                }
            }
        }
        Ok(false)
    }

    /// Allocate the next operation counter for this device.
    pub fn next_op_counter(&self) -> Result<u64> {
        const KEY: &[u8] = b"op/next_counter";
        let current = self
            .stores
            .kv
            .get_kv(crate::state::CF_META, KEY)?
            .and_then(|v| v.try_into().ok())
            .map(u64::from_be_bytes)
            .unwrap_or(0);
        let next = current + 1;
        self.stores
            .kv
            .put_kv(crate::state::CF_META, KEY, &next.to_be_bytes())?;
        Ok(next)
    }

    /// Root inode of the local repository.
    pub fn root_inode(&self) -> u128 {
        ROOT_INODE
    }

    /// Number of persisted namespace records, for admin reporting.
    pub fn state_entry_count(&self) -> Result<usize> {
        Ok(self.stores.kv.scan_prefix(CF_STATE, b"")?.len())
    }

    /// Device id as used for CRDT writer ordering.
    pub fn writer_id(&self) -> DeviceId {
        self.device_id
    }
}
