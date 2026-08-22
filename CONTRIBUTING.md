# Contributing

NexusFS is a working system rather than a skeleton: all eight milestones are complete,
and the local core, the S3 facade, QUIC replication, encryption at rest, the energy
scheduler, operator tooling and the state commitment are all implemented and tested.
What remains is in [`documentation/backlog.md`](documentation/backlog.md), and none of
it blocks what shipped.

## Ground rules

- Keep crate boundaries clean (see [`docs/architecture.md`](docs/architecture.md)).
- Add tests for every feature. The suite is 205 tests and the interesting ones are
  properties, not smoke tests — order-independent convergence, idempotent re-apply,
  what a proof must refuse.
- Do not break the canonical encoding without bumping object versions. `postcard`
  carries no field names or type tags, so a decoder handed bytes from another schema can
  succeed and produce nonsense rather than failing.
- Anything that changes the state root is an on-disk **and** wire format change. Bump
  `CURRENT_FORMAT_VERSION` with a migration step and `PROTOCOL_VERSION` together.
- Keep the binary single-executable friendly: use feature flags, and avoid runtime
  dependencies on external services.

## Development workflow

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three run in CI, and clippy is **not** optional there — `RUSTFLAGS: -D warnings` is
set for the whole workflow. CI also builds the wasm target, asserts the module still has
no JS imports, and checks every feature combination of the binary.

If your checkout lives in a cloud-synced folder, redirect Cargo's output first:

```bash
export CARGO_TARGET_DIR=~/Library/Caches/nexusfs-target
```

## Measure before optimising

Several of the performance changes in this repository went the opposite way from the
obvious guess — the state-root walk looked like the cost of an apply and the fsync
turned out to be. Numbers in commit messages and in
[`documentation/current-status.md`](documentation/current-status.md) are measured, not
estimated. Keep it that way.

## Security invariants to preserve

- Do not accept unsigned operations.
- Hash-check every blob before storing it — and store only what was actually requested.
  A blob being self-consistent is not the same as this node having asked for it.
- Put size limits on all network messages, and bound anything a peer can make you
  iterate over.
- A peer's device key is pinned. Replacing one is an explicit action, never a silent
  overwrite.
- Do not present unverified data as verified. If a field is not covered by a signature
  or a proof, label it.

## Documentation

The public docs in [`documentation/`](documentation/) and the site in [`site/`](site/)
are part of the change, not a follow-up. A milestone that ships without them saying so
is a milestone nobody can find.
