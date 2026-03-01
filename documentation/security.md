# Security Model

NexusFS takes a verifier-first stance: data is not accepted merely because it was received.

## Core Security Assumptions

- devices may be offline for long periods
- peers may be honest, buggy, stale, or malicious
- local disks may need at-rest encryption
- bandwidth and energy are constrained resources

## Immediate Defenses

The practical baseline focuses on:

- signed operations
- content-addressed integrity for blobs and objects
- explicit proof mode selection
- optional encryption at rest
- authenticated local admin access

## Threat Categories

The source threat model highlights concerns such as:

- forged or replayed operations
- tampered blobs
- namespace conflicts during concurrent mutation
- metadata leakage
- resource exhaustion and abuse of background work

## Security Priorities

1. Deterministic canonical encoding so hashes are stable.
2. Idempotent replay handling so repeated messages do not corrupt state.
3. Strong boundaries between trust establishment and data acceptance.
4. Explicit policy surfaces for privacy and future research modes.

## Deep-Dive Sources

- `../docs/threat_model.md`
- `../docs/object_formats.md`
- `../docs/protocol.md`
