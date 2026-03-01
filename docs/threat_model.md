# NexusFS Threat Model (v0)

This threat model guides implementation choices. It should be updated as features land.

---

## 1. Assets to protect

1) **Confidentiality of file content**
- plaintext must not be recoverable from stored chunks or network traffic

2) **Integrity of file content**
- peers must not accept corrupted or tampered data

3) **Integrity of filesystem state**
- directory structure, metadata, and operation history must not be forgeable

4) **Availability**
- the system should continue to function offline and under partial connectivity
- degrade gracefully under resource constraints

5) **Privacy of metadata (optional modes)**
- access patterns, file sizes, listing contents, and timing should be reducible via privacy modes
- note: strong metadata privacy is hard; document what each mode protects

6) **Auditability / non-repudiation**
- operations should be attributable to keys/devices (unless explicitly anonymized by policy)

---

## 2. Attacker models

### A1. Passive network observer
Capabilities:
- sees traffic metadata (source/dest IPs, timing, volumes)
- cannot break TLS

Goals:
- infer file content or sensitive metadata

Mitigations:
- QUIC/TLS encryption in transit
- optional padding/cover traffic (privacy layer)
Residual risks:
- timing and volume correlation remain unless cover traffic is strong (costly)

### A2. Active network attacker (MITM)
Capabilities:
- can inject/drop/reorder packets
- can attempt connection hijacking
- cannot break modern TLS primitives

Goals:
- tamper with data, desync peers, inject fake ops

Mitigations:
- QUIC/TLS channel integrity
- op signatures verified end-to-end
- blob hash verification
- idempotent replication with dedupe
Residual risks:
- DoS by flooding connections / CPU burn (rate limiting needed)

### A3. Malicious peer (Byzantine participant)
Capabilities:
- owns a valid key that is trusted by some nodes
- can send arbitrary signed ops and blobs

Goals:
- corrupt FS state, exfiltrate, cause conflicts, poison caches, inflate storage

Mitigations:
- policy engine (ACL/capabilities) for authorization
- proof verification (transparent then ZK) for operation validity
- resource quotas (per peer)
- CRDT merge rules that prevent divergence
Residual risks:
- authorized peers can still delete or overwrite if policy permits (this is not preventable cryptographically without additional governance)

### A4. Compromised device
Capabilities:
- attacker has full access to one node (keys, storage, memory)

Goals:
- impersonate device, decrypt local data, leak content, sign malicious ops

Mitigations:
- optional hardware-backed keys (TPM/TEE)
- ability to revoke a key/device and rotate folder keys
- minimize long-lived plaintext in memory
Residual risks:
- if device key compromised, attacker can act as that device until revoked

### A5. Physical theft of storage medium
Capabilities:
- attacker steals SD card/SSD

Goals:
- decrypt at-rest data

Mitigations:
- encrypt at rest by default
- encrypt key material at rest (passphrase, OS keyring, or TPM sealing)
Residual risks:
- weak passphrases, memory scraping while unlocked

### A6. Insider with admin console access
Capabilities:
- local admin token

Goals:
- change policies, exfiltrate metadata, force replication

Mitigations:
- admin auth token with rotation
- optional mTLS for admin
- audit logs
Residual risks:
- if admin access is compromised, many controls can be bypassed

---

## 3. Trust assumptions

- Cryptographic primitives are sound (ed25519, AEAD, BLAKE3, QUIC/TLS).
- OS provides basic process isolation (not a hostile kernel).
- When TOFU is enabled, the first connection is assumed not MITM (or you accept that risk).

---

## 4. Security requirements (implementation checklist)

### 4.1 Operations
- Every mutating op MUST be signed.
- Receiver MUST verify signature before storing/applying.
- Receiver MUST reject ops with invalid structure.
- Replays MUST be safe (dedupe by OpId).

### 4.2 Blobs
- Every blob MUST be verified by hash before acceptance.
- CAS MUST be immutable (never overwrite blob for same hash).
- Optional: store blob length to prevent memory bombs.

### 4.3 Key management
- Device keypair stored securely.
- File/folder keys encrypted for authorized recipients (envelopes).
- Implement key rotation hooks early (even if UI is later).

### 4.4 Network and DoS hardening
- Connection rate limiting (per IP / per pubkey)
- Message size limits
- Backpressure (`limit_ops`, `max_bytes`)
- CPU budget for proof verification (time boxed / queued)

### 4.5 Admin console
- Token auth required by default
- Bind to localhost by default
- Make remote binding explicit and warn loudly
- Audit admin actions

---

## 5. Privacy guarantees (be explicit)

### Baseline mode (default)
- Content confidentiality: strong (AEAD + TLS)
- Metadata confidentiality: weak (file sizes, access patterns leak)

### Padding mode
- Reduces file size leakage by bucketing/padding.
- Does not hide access frequency or timing.

### Cover traffic mode
- Reduces access pattern leakage at cost of bandwidth/energy.
- Must be energy-aware and rate-limited.

### Oblivious metadata mode (research)
- Attempts to hide directory access patterns and counts.
- Likely expensive; treat as opt-in.

---

## 6. Auditability and compliance

- All ops signed and stored in oplog.
- Provide export tool:
  - signed timeline of operations
  - snapshot hashes
- Provide verification tool:
  - verify signatures, hashes, and proof bundles

---

## 7. Residual risks and open problems

- Side channels (timing, power, RF) are largely out of scope.
- Metadata privacy remains difficult without high overhead.
- Key revocation in fully offline settings is challenging (needs policy and gossip).

This is acceptable if documented and configurable.
