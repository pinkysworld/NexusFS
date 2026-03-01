# Protocol and Replication

NexusFS replicates intent before payload.

That means peers exchange signed operation knowledge first, then fetch the referenced blobs they do not yet have.

## Replication Shape

1. A peer handshake establishes identity, capabilities, and trust posture.
2. Peers exchange a clock summary describing the highest observed operation counters per device.
3. Missing operations are requested and transferred in batches.
4. Referenced blobs are requested on demand.
5. Received data is verified, stored, and applied idempotently.

## Why Oplog First?

Oplog-first replication gives NexusFS better behavior under:

- unstable networks
- delayed blob availability
- resumable synchronization
- future proof verification and audit workflows

## Operation Types

The protocol currently models:

- `Mkdir`
- `CreateFile`
- `Write`
- `Rename`
- `Unlink`
- `SetAttr`

Each operation carries:

- a stable `OpId`
- causal context
- author metadata
- a signature
- an optional proof bundle

## Verification Rules

Before remote state becomes trusted:

- signatures must verify
- object or blob hashes must match
- policy checks must pass
- enabled proof modes must validate

## Proof Modes

- `None`: signatures and hashes only
- `Transparent`: structured non-ZK evidence
- `ZkCommit`: future commitment-based ZK verification
- `ZkFull`: research-grade full ZK mode

For message details, use `../docs/protocol.md`.
