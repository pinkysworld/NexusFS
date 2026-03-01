# Research Tracks (R01–R80)

NexusFS is organized as parallel research/engineering tracks. Each track MUST have:
- a route namespace: `/api/research/<track_id>/...`
- a UI tab/panel in the admin console
- at least one automated test
- a short doc section describing goals, metrics, and risks

---

## Track template (how to implement any track)

**Track ID:** Rxx  
**Name:** short title  
**Problem:** what limitation exists today?  
**Hypothesis / approach:** proposed technique  
**Deliverables:**
- Code modules (which crates/files)
- RPC endpoints
- Admin UI metrics + controls
- Tests (unit/integration/sim)
**Evaluation:**
- benchmarks and datasets
**Risks:**
- failure modes, costs, and fallback behavior

---

## A) Core system tracks

### R01 — Learned replication scheduling under energy constraints
- **Goal:** maximize freshness/availability per joule.
- **Approach:** start rule-based, then contextual bandit / RL with offline training.
- **Deliverables:** `energy::Scheduler` learned backend; telemetry dataset exporter; simulator.
- **Metrics:** head lag, bytes/J, convergence time, battery drain.

### R02 — Zero-knowledge file provenance and operation proofs
- **Goal:** prove operations are valid without revealing contents or sensitive metadata.
- **Approach:** ZkCommit mode with SNARK-friendly commitments + directory roots.
- **Metrics:** proof gen/verify time, proof size, acceptance rate.

### R03 — Per-file differential privacy for shared folders
- **Goal:** reduce leakage from size/listing/timing.
- **Approach:** padding + cover traffic + DP budgets in UI.
- **Metrics:** overhead %, distinguishability reduction.

### R04 — Verifiable conflict-free replicated data types (CRDTs)
- **Goal:** CRDT merges that are provably correct (transparent first, then ZK).
- **Approach:** prove OR-Map transitions against Poseidon roots.
- **Metrics:** proof overhead vs conflict rate.

### R05 — Energy-proportional chunking and deduplication
- **Goal:** adjust chunking to device energy and workload.
- **Approach:** switch chunk sizes/CDC based on telemetry and file type.
- **Metrics:** dedup ratio, CPU cost, read amp.

---

## B) Verifiability & security

### R06 — Merkle-based verifiable deletion guarantees
- **Goal:** generate evidence that a node *claims* deletion and can be audited.
- **Approach:** deletion receipts + challenge protocols; document limitations.
- **Metrics:** audit success rate, false-positive risk.

### R07 — Cross-device causal consistency with cryptographic proofs
- **Goal:** provide causal ordering proofs for ops.
- **Approach:** vector clocks and proof-carrying deps.
- **Metrics:** overhead of deps vs conflict reduction.

### R08 — Privacy-preserving neighbor discovery in mesh networks
- **Goal:** discover peers without leaking identity/social graph.
- **Approach:** rotating pseudonyms, PSI-style matching, or anonymous beacons.
- **Metrics:** discovery success, privacy leakage.

### R09 — Secure multi-party computation for file sharing
- **Goal:** enable sharing without revealing keys to any single party.
- **Approach:** threshold decryption, MPC key issuance.
- **Metrics:** latency, robustness.

### R10 — Verifiable audit logs for regulatory compliance
- **Goal:** export logs that auditors can verify.
- **Approach:** signed log segments + periodic checkpoints; optional anchoring.
- **Metrics:** export size, verification time.

---

## C) Performance & efficiency

### R11 — Energy-aware compaction for long-running nodes
- **Goal:** compaction only when energy/temp budget allows.
- **Approach:** compaction queue with scheduler gating.
- **Metrics:** write amp, DB size growth, tail latency.

### R12 — Adaptive topology learning for dynamic mesh networks
- **Goal:** choose best peers in churny networks.
- **Approach:** bandit peer selection; link-quality predictors.
- **Metrics:** convergence time vs churn.

### R13 — Learned caching policies for offline-first applications
- **Goal:** prefetch and retain content for offline windows.
- **Approach:** recency+frequency+predicted offline intervals.
- **Metrics:** cache hit rate, bytes wasted.

### R14 — Energy-aware compression pipeline
- **Goal:** choose codec based on CPU budget and file type.
- **Approach:** heuristic model + per-device calibration.
- **Metrics:** compression ratio vs CPU/J.

### R15 — Hybrid row/column storage for metadata-heavy workloads
- **Goal:** accelerate metadata queries and listings.
- **Approach:** maintain a columnar index for selected fields.
- **Metrics:** query latency, index overhead.

---

## D) Advanced features

### R16 — Wasm-based user-defined storage policies
- **Goal:** safe, sandboxed per-folder policies.
- **Approach:** deterministic Wasm runtime, resource limits.
- **Metrics:** policy overhead, safety.

### R17 — Verifiable hot-swapping of storage modules
- **Goal:** replace chunking/compression modules with verifiable compatibility.
- **Approach:** module manifests and signed compatibility statements.
- **Metrics:** upgrade success, rollback safety.

### R18 — On-device vector search over file metadata
- **Goal:** semantic search without cloud.
- **Approach:** local embeddings + encrypted indexes (later).
- **Metrics:** query latency, memory.

### R19 — Cross-platform binary optimization (x86/ARM/RISC-V)
- **Goal:** fast builds and small binary.
- **Approach:** feature gating, LTO, minimal deps.
- **Metrics:** binary size, cold start time.

### R20 — Energy-aware synchronization with partial connectivity
- **Goal:** synchronize "important parts" first.
- **Approach:** prioritized oplog + blob queues, partial sync modes.
- **Metrics:** time-to-first-useful-state.

---

## E) Additional tracks (R21–R50)

### R21 — Post-quantum identities & key exchange
### R22 — Capability-based access control with proof-carrying authorization
### R23 — Reproducible builds and binary attestations
### R24 — TPM/TEE integration for hardware-backed keys
### R25 — Threshold key management for shared folders
### R26 — SNARK-friendly Merkle overlay for directories & inode maps
### R27 — Recursive proofs for long operation logs (checkpoint compression)
### R28 — ZK “read correctness” proofs (content served matches commitments)
### R29 — Proof amortization and batching on low-power devices
### R30 — Proof-of-deletion with challenge protocols
### R31 — DTN store-carry-forward replication
### R32 — Multi-radio replication (Wi-Fi/BLE/LoRa metadata-only)
### R33 — Privacy-preserving peer reputation and Sybil resistance
### R34 — Congestion control tuned for mesh & intermittent links
### R35 — Federated topology learning with DP
### R36 — Erasure coding vs replication under energy constraints
### R37 — Tiered storage placement with endurance awareness
### R38 — CDC optimized for ARM/RISC-V (energy-proportional)
### R39 — Workload-adaptive metadata indexing
### R40 — Snapshot diff and fast cloning
### R41 — Wasm policy sandbox hardening + determinism proofs
### R42 — Private information retrieval for metadata queries
### R43 — DP budgeting UI + enforcement
### R44 — Oblivious directory listing (hide counts + patterns)
### R45 — Secure search over encrypted metadata
### R46 — Windows/macOS mounting parity
### R47 — Forensics and compliance export packages
### R48 — Self-healing repair protocols and blob recovery
### R49 — Multi-tenant isolation and quotas
### R50 — Rolling upgrades with verifiable state migration

(Short descriptions are provided in the appendix section below.)

---

## F) Extended tracks (R51–R80) — deeper “think harder” agenda

### R51 — File-content CRDTs (rope/sequence CRDT for collaborative editing)
- Make file bodies mergeable instead of “conflict copies.”

### R52 — Semantic merge assistants and conflict UX
- Use file type heuristics and merge tools; generate human-friendly resolution reports.

### R53 — Geo-aware replication and placement
- Place replicas based on geofences, jurisdiction, or operational zones.

### R54 — Bounded-staleness sync mode
- Guarantee reads are within T seconds or N ops behind a specified quorum (when available).

### R55 — Adaptive proof modes per peer
- Negotiate proof mode based on device power and trust, per connection.

### R56 — Secure bootstrapping via QR/out-of-band pairing
- Pair devices without TOFU risk in hostile networks.

### R57 — Data lifecycle policies (retention, TTL, legal hold)
- Verifiable retention rules and deletion schedules.

### R58 — Immutable audit snapshots and notarization
- Periodically produce signed, externally verifiable checkpoints.

### R59 — On-device anomaly detection for malicious ops
- Detect suspicious patterns (mass deletes, churn spikes) and alert.

### R60 — Removable media / cold storage offload
- Seamless attach/detach external drives with verified indexing.

### R61 — Verified compaction correctness
- Prove compaction does not change logical contents (transparent checks, then ZK).

### R62 — SNARK-friendly hash bridging (BLAKE3↔Poseidon commitments)
- Formalize how commitments bind to CAS hashes without full in-circuit BLAKE3.

### R63 — TEE-assisted proof generation
- Use TEEs to accelerate or protect witnesses (optional).

### R64 — Content previews and summarization pipeline
- Generate thumbnails/summaries offline with privacy controls.

### R65 — Local metadata query language (mini-SQL)
- Query file properties without scanning entire trees.

### R66 — NAT traversal improvements (ICE-like rendezvous)
- Better connectivity in real networks.

### R67 — Covert and resilient sync channels
- Optional steganographic or censorship-resistant transport (research only).

### R68 — Bandwidth shaping and fairness
- Ensure background sync doesn’t starve foreground app traffic.

### R69 — Differentially private telemetry sharing
- Share scheduler-relevant signals without exposing raw device state.

### R70 — Multi-hop replication with incentives/credits
- Encourage relays in community meshes.

### R71 — Remote attestation of node software version
- Verify peer is running an approved build.

### R72 — Social recovery for keys
- Recover access without central server.

### R73 — Directory sharding for massive namespaces
- Scale to millions of entries with partitioned metadata.

### R74 — Edge object store mode for ML datasets
- Efficient partial replication of large datasets.

### R75 — S3 byte-range GET and multipart upload
- Required for many S3 tools and big objects.

### R76 — rsync-like delta sync for large files
- Sync only changed ranges rather than whole chunks.

### R77 — Pinning and priority policies
- Explicit pin sets and replication priority levels.

### R78 — Real-time collaboration integrations
- WebRTC-style live sessions using the same oplog.

### R79 — Formal verification of core state machine
- TLA+/Ivy style modeling; ensure invariants and convergence.

### R80 — Interop with IPFS / other CAS networks
- Import/export DAGs; gateway mode.

---

## Appendix: Short descriptions for R21–R50

- **R21:** hybrid PQC + classical handshake, migration path
- **R22:** capabilities per operation, optional ZK proof of possession
- **R23:** reproducible builds, signed SBOM, in-admin verification
- **R24:** TPM sealing, TEE storage, stolen-device mitigation
- **R25:** M-of-N folder keys, offline recovery
- **R26:** maintain Poseidon Merkle roots for ZK-friendly proofs
- **R27:** recursive SNARKs to compress oplog proofs
- **R28:** prove served data matches commitments without revealing plaintext
- **R29:** batch proofs under energy constraints
- **R30:** deletion proofs with remote challenges and receipts
- **R31:** DTN bundles, store-carry-forward replication
- **R32:** radio-aware sync strategy (metadata over low-power links)
- **R33:** Sybil resistance without deanonymization
- **R34:** mesh-tuned congestion control and resumable transfers
- **R35:** federated learning of topology with DP
- **R36:** erasure coding tradeoffs for energy and reliability
- **R37:** tiering across RAM/SSD/flash with endurance signals
- **R38:** CDC optimized for low-power CPUs
- **R39:** adaptive indexes driven by query logs
- **R40:** snapshot diff export/import for fast cloning
- **R41:** Wasm determinism, sandbox escapes prevention
- **R42:** PIR for private metadata queries
- **R43:** track and enforce epsilon budgets with UI
- **R44:** hide listing patterns and counts
- **R45:** encrypted search or secure embeddings
- **R46:** parity across OS mounting stacks
- **R47:** audit packages and signed timelines
- **R48:** self-healing repair and alternative peer recovery
- **R49:** multi-tenant isolation, quotas, and governance
- **R50:** verifiable rolling upgrades and migrations
