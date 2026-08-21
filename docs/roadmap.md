# NexusFS Roadmap (solo-friendly)

> **Superseded — kept for history.** This was the original capability sketch, written
> before any of it existed, and its milestone numbering does not match what was built.
> The delivered plan, with exit criteria and what was deliberately not built, is
> [`../documentation/roadmap.md`](../documentation/roadmap.md).
>
> The numbering diverged from M6 onward. What actually shipped:
>
> | Here | Delivered as |
> | --- | --- |
> | M0–M5 | the same, and complete |
> | M6 "ZK MVP" | **M7 State commitments** — a Merkle commitment with inclusion and absence proofs, not a circuit. A proving system stays behind its own milestone |
> | — | **M6 Operational hardening** — collection, format versioning, peer enrolment. Not in this sketch at all, and needed before any of the above was operable |
> | M7 "Expand research tracks" | **M8 Research expansion** — proof batching and absence proofs shipped; privacy layers, DTN and policy systems are named as open |
>
> All eight delivered milestones are complete. See
> [`../documentation/current-status.md`](../documentation/current-status.md).

This roadmap is organized by **capability** rather than by research tracks.

## Milestone M0 — Repo bootstrapped (this zip)
- Workspace compiles
- Daemon boots
- Admin UI served
- Clear module boundaries + docs

## Milestone M1 — Local filesystem core
- CAS + KV store working
- Chunking + file/dir objects
- Oplog + apply ops to local state
- Snapshots + head pointer
- Admin shows: head, CAS stats, oplog stats

## Milestone M2 — POSIX OR S3 façade (pick one first)
Option A: POSIX (FUSE)
- mkdir/create/read/write/rename/unlink work
Option B: S3-like API
- PUT/GET/DELETE/LIST work

## Milestone M3 — Replication (non-ZK)
- QUIC session manager
- Have/WantOps/OpsBatch sync
- WantBlobs/BlobsBatch sync
- Signed ops and verified blobs
- Admin shows peer health + sync progress

## Milestone M4 — Encryption at rest + transparent proofs
- Chunk AEAD encryption
- Key envelopes for authorized peers
- Transparent proof bundles attached to ops
- Verification tool (`nexusfs verify`)

## Milestone M5 — Energy-aware scheduling (baseline)
- Telemetry sampling (battery/temp)
- Rule-based scheduler
- Replication loop consults scheduler
- Admin energy tab: telemetry and current replication mode

## Milestone M6 — ZK MVP
- ProofMode::ZkCommit behind feature flag
- One circuit implemented (e.g., Write op commitment validity)
- Verification enforced for ZK-enabled peers

## Milestone M7 — Expand research tracks
- CRDT proofs
- DP policies
- DTN routing
- Proof batching
- Vector search
- Wasm policies
