# Contributing

This is a blueprint + skeleton repo intended to be built out iteratively.

## Ground rules
- Keep crate boundaries clean (see `docs/architecture.md`).
- Add tests for every feature.
- Avoid breaking the canonical encoding unless you bump object versions.
- Keep the binary single-executable friendly:
  - use feature flags
  - avoid runtime dependencies on external services

## Development workflow
- `cargo fmt`
- `cargo test`
- `cargo clippy` (optional)

## Security
- Do not accept unsigned ops.
- Hash-check all blobs before storing.
- Put size limits on all network messages.
