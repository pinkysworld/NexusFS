# Configuration Reference

NexusFS uses a single TOML config file.

See: `examples/nexusfs.toml`

---

## `[node]`
- `data_dir`: directory for local DB + CAS
- `device_name`: purely cosmetic label for UI/logs

## `[net]`
- `listen`: QUIC listen address (e.g., `0.0.0.0:4444`)
- `peers`: list of peer URLs (`quic://host:port`)
- `tofu`: trust-on-first-use (default: true for dev, false for prod)

## `[admin]`
- `bind`: HTTP bind address (default `127.0.0.1:7070`)
- `token`: admin token (generated on first start if missing)

**Security note:** binding to `0.0.0.0` exposes admin to your network. Do this only with strong auth/mTLS.

## `[s3]`
- `enabled`: start the S3-like API server
- `bind`: bind address (e.g., `0.0.0.0:9000`)

## `[posix]`
- `enabled`: enable FUSE mount helper
- `mountpoint`: mount path

## `[security]`
- `encrypt_at_rest`: encrypt chunks and key material locally
- `proof_mode`: `none|transparent|zk_commit|zk_full`

## `[energy]`
- `enabled`: enable energy-aware scheduling
- `battery_low_pct`: threshold to restrict blob replication
- `temp_high_c`: threshold to restrict CPU-heavy jobs (proofs/compaction)
