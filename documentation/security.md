# Security Model

NexusFS takes a verifier-first stance: data is not accepted merely because it was
received. Every boundary where bytes arrive from somewhere else — a peer, a facade, a
proof file — is a place where the receiving side re-derives the fact rather than
trusting the claim.

## Core Security Assumptions

- devices may be offline for long periods
- peers may be honest, buggy, stale, or malicious
- local disks may need at-rest encryption
- bandwidth and energy are constrained resources

## What Is Actually Enforced

**Operations are signed and verified before they change state.** The same `apply_op`
serves local writes and remote ones, so there is no path into the state machine that
skips verification. A tampered or unsigned operation is rejected rather than parked.

**Content is verified against the hash that was asked for.** A peer sending a chunk
cannot substitute bytes: the receiver recomputes the hash before storing. This holds for
encrypted repositories too, because chunks are addressed by the hash of the bytes *as
stored* — the ciphertext — so a peer with no key can still verify a transfer.

**A peer's answer is not permission to store.** Content that was never requested is
dropped even when it hashes correctly. Self-consistency is not the same as being asked
for; without this check a trusted-but-hostile peer could write arbitrary blobs into the
content-addressed store, and each one would count as progress against the guard that
ends a stalled session.

**Peer identity is an ed25519 key pinned on first use, independent of TLS.** Certificates
are transport encryption, not the trust anchor. A device presenting a different key than
the one pinned is refused whatever the policy says. Replacing a pinned key requires
`--rotate`, because a silent overwrite would erase the one signal separating a planned
rotation from an impersonation attempt. Setting `net.tofu = false` requires enrolment
before first contact.

**Content is sealed per recipient, not under a key every replica holds.** A write seals
its file key to each enrolled peer's X25519 key and to this device's, so a replica that
is not a recipient cannot read the content even holding every stored byte. Each device's
signing and sealing keys are independent secrets rather than one mapped into the other's
curve, which keeps a signing oracle and a Diffie-Hellman oracle off the same scalar.
Revoking a peer removes both its keys, so it stops being a recipient of anything written
afterwards.

**A file does not publish who can read it.** Envelopes carry no recipient identity; a
reader trials its own key against each. The recipient-set digest a file carries for
re-sealing is keyed by the file key, so only someone who can already read the file can
test a candidate set against it.

**Malformed evidence is refused, not ignored.** A proof bundle that does not parse or
does not match its operation is rejected deterministically. Evidence that can be
malformed and still accepted is worse than no evidence, because it looks like a check.

**A proof's subject comes from the proof.** `check-proof` reports the inode the proof
commits to, not the `path` and `inode` labels carried in the file — those are not covered
by the proof, and it warns when they disagree with it.

**Absence proofs rest on adjacency, and adjacency is checked.** Two entries that merely
bracket an inode prove nothing, because the inode could be one of the entries between
them. Each neighbour states its index, and that index is checked against the shape its
path must have in a map of the claimed size. The claimed size is prover-supplied and not
covered by the root, so the whole space of lies is swept by test for small maps: no
forged length lets a middle leaf pass as the first or the last, or makes two
non-adjacent leaves look adjacent.

**The on-disk format is stamped and enforced.** `postcard` carries no field names or
type tags, so a decoder handed bytes from another schema can succeed and produce
nonsense. The stamp turns that into a refusal in both directions, and a newer format
cannot be forced open.

## Threat Categories

The source threat model highlights concerns such as:

- forged or replayed operations
- tampered blobs
- namespace conflicts during concurrent mutation
- metadata leakage
- resource exhaustion and abuse of background work

## Limits Worth Stating Plainly

- **Access cannot be withdrawn.** `nexusfs share` seals a file key to more recipients;
  nothing takes it back from one. The ciphertext does not change, so a device that once
  held an envelope can still decrypt what it kept. Revocation needs re-encryption under a
  fresh key, which is not built.
- **`identity.toml` is the key to your content.** With per-recipient sealing, losing it
  loses everything sealed to this device. It is written owner-only, in the data
  directory.
- **File names, directory structure and file sizes are not encrypted.**
- **The commitment layer is not zero-knowledge.** A verifier learns the inode being
  proved and its object hash; what it does not learn is the rest of the tree.
  `zk_full` is accepted as a config value and behaves as `none`.
- **The S3 facade has no request signing.** Authentication is an optional shared secret,
  so it belongs on loopback or another trusted interface.
- **Identical plaintext does not deduplicate** across file keys. Convergent encryption
  would recover that at the price of letting anyone holding a candidate file confirm
  whether a node stores it.

## Security Priorities

1. Deterministic canonical encoding so hashes are stable.
2. Idempotent replay handling so repeated messages do not corrupt state.
3. Strong boundaries between trust establishment and data acceptance.
4. Explicit policy surfaces for privacy and future research modes.

## Auditing A Repository

`nexusfs verify` checks every signature, every proof's structure, and reads every file
back — which exercises chunk presence, ordering and, when encrypted, authentication. It
exits non-zero on failure, so it works as a cron or CI check. The same report is served
at `/api/security`.

## Deep-Dive Sources

- `../docs/threat_model.md`
- `../docs/object_formats.md`
- `../docs/protocol.md`
