# NexusFS Roadmap (solo-friendly)

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
