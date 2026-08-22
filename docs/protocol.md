# NexusFS Replication Protocol (v0 Draft)

This protocol runs over **QUIC** (Quinn). It is designed to be:
- simple enough to implement first
- secure by default (auth + integrity)
- extensible for research features (ZK proofs, privacy-preserving discovery, DTN routing)

> This began as a v0 draft. The implementation now runs at `PROTOCOL_VERSION = 3`
> (`crates/net/src/replication.rs`). Version 2 came with the Merkle state commitment and
> version 3 with per-recipient key sealing, which changed the shape a `Write` uses to
> describe how its file key is protected. Either way a mismatched peer refuses the
> handshake rather than syncing and then disagreeing forever. Still to come: compression, streaming blob transfer, range requests, and
> compressed proof batches.
>
> Two properties the draft does not state and the implementation enforces: the clock
> summary carries the counters observed *above* a gap as well as the contiguous high
> water mark, so one refused operation cannot stall everything behind it; and content
> that was never requested is dropped on receipt, however well it hashes.

---

## 1. Transport

- QUIC over UDP
- TLS 1.3 is used by QUIC for confidentiality and integrity
- Application-level message signing is still required for **end-to-end verifiability**
  and to survive relay/DTN scenarios.

### 1.1 Framing

All application messages are framed on a QUIC bidirectional stream:

```
frame := len_u32_le || payload[len]
payload := postcard(MessageEnvelope)
```

- `len_u32_le`: little-endian u32
- `payload`: deterministic serialization (postcard)

### 1.2 Envelope

```rust
struct MessageEnvelope {
  protocol_version: u16,      // currently 3
  msg_id: u64,                // per-connection monotonic id
  reply_to: Option<u64>,      // for request/response matching
  payload: Msg,               // enum
}
```

---

## 2. Peer identity & trust

Each node has:
- `device_id: u128`
- `ed25519` signing keypair
- a local trust store:
  - **TOFU** (trust on first use) OR
  - explicit allowlist of pubkeys

### 2.1 Handshake messages

#### `Hello`
Sent immediately after connection establishment:

```rust
Hello {
  device_id: u128,
  pubkey_ed25519: [u8; 32],
  features: Vec<String>,
  build: BuildInfo,
  time_unix_ms: u64,
  nonce: [u8; 32],
  sig: Vec<u8>, // Sign(ed25519_sk, hash(Hello fields without sig))
}
```

`BuildInfo` includes semantic version and git hash (optional).

**Receiver actions**
- verify `sig` matches `pubkey_ed25519`
- consult trust store:
  - if unknown and TOFU enabled: record pubkey
  - else reject
- respond with `HelloAck`

#### `HelloAck`

```rust
HelloAck {
  accepted: bool,
  reason: Option<String>,
  features: Vec<String>,
  nonce: [u8; 32],
  sig: Vec<u8>,
}
```

---

## 3. State summaries and oplog sync

### 3.1 State summary structure

Nodes maintain an oplog indexed by `(device_id, counter)`.

Define a compact summary:

```rust
ClockSummary {
  // for each device_id we know about: highest contiguous counter applied
  entries: Vec<(u128 device_id, u64 max_counter)>,
}
```

### 3.2 Summary exchange

After Hello/HelloAck:
- both sides send `Have { summary: ClockSummary }`

```rust
Have {
  summary: ClockSummary,
  head: Option<SignedHead>, // optional snapshot head announcement
}
```

### 3.3 Determining missing operations

Given:
- my summary `S_me`
- peer summary `S_peer`

Missing from me:
- for each `(device_id, max_peer)` in `S_peer`:
  - my_max = S_me.get(device_id).unwrap_or(0)
  - if max_peer > my_max: I need `(my_max+1 ..= max_peer)`

### 3.4 Request missing ops

```rust
OpRange {
  device_id: u128,
  start: u64, // inclusive
  end: u64,   // inclusive
}

WantOps {
  ranges: Vec<OpRange>,
  limit_ops: u32, // backpressure
}
```

### 3.5 Sending ops

```rust
OpsBatch {
  ops: Vec<FsOp>,
  more: bool, // peer should request more if true
}
```

Receiver must:
- verify op signature
- verify op structure is well-formed
- store in oplog if not already present
- apply idempotently to CRDT state (after required blobs are present OR mark as pending)

---

## 4. Blob sync

Operations may reference content hashes. The receiver requests missing blobs:

```rust
WantBlobs {
  hashes: Vec<Hash>,
  max_bytes: u64, // sender should cap response
}
```

Response:

```rust
BlobsBatch {
  blobs: Vec<(Hash, Vec<u8>)>,
  more: bool,
}
```

Receiver must:
- verify `Hash == blake3(blob_bytes)`
- store in CAS
- unblock any pending ops that need these blobs

**Note:** v0 does not do range requests; later add:
- `WantBlobRanges { hash, ranges: ... }`
- resumable transfers

---

## 5. Snapshot head announcements

A **head** is a `SnapshotRoot` hash plus a signature over it:

```rust
SignedHead {
  head_hash: Hash,
  device_id: u128,
  time_unix_ms: u64,
  sig: Vec<u8>, // sign(sk_device, hash(head_hash || device_id || time))
}
```

`SnapshotAnnounce { signed_head: SignedHead }`

Nodes may send this:
- after applying a batch of ops
- periodically
- on request

Receiver verifies signature, stores peer head, and can use it for UI and future fast sync.

---

## 6. Telemetry exchange (energy-aware replication)

```rust
Telemetry {
  battery_pct: Option<u8>,
  charging: bool,
  temp_c: Option<i16>,
  cpu_load: f32,
  link_cost: f32,
  storage_free_bytes: u64,
  time_unix_ms: u64,
}
```

This message is *advisory*. It helps the scheduler decide:
- whether to push blobs now
- whether to only sync ops
- compaction timing

---

## 7. Errors

```rust
Error {
  code: u16,
  message: String,
  retry_after_ms: Option<u64>,
}
```

Recommended codes:
- 1000: protocol version mismatch
- 1001: auth failed / unknown peer
- 1002: invalid signature
- 1003: invalid blob hash
- 1004: busy / backpressure
- 1005: unsupported feature

---

## 8. Proof bundles (verifiability)

Every `FsOp` may contain:

```rust
ProofBundle {
  mode: ProofMode, // none|transparent|zk_commit|zk_full
  bytes: Vec<u8>,  // mode-specific encoding
}
```

### 8.1 Transparent proofs (v0 recommended)
`bytes` encodes:
- old head hash (optional)
- new head hash (optional)
- list of changed object hashes
- optional Merkle inclusion proofs

### 8.2 ZK commit proofs (v1 research)
`bytes` encodes:
- public inputs (old/new Poseidon roots, op type commitment)
- SNARK proof bytes

---

## 9. Versioning rules

- `protocol_version` increments on breaking changes.
- Additive message variants:
  - allowed if older peers ignore unknown variants (in practice: gated by feature negotiation)
- Feature negotiation:
  - peers include `features: Vec<String>` in Hello and HelloAck
  - nodes must only use features present in the intersection set

---

## 10. Security notes

- QUIC/TLS provides channel security, but end-to-end op signatures are still required.
- TOFU is acceptable for lab/mesh prototypes; production should use an allowlist or PKI.
- Reject all unsigned ops and all blobs with mismatching hashes.
