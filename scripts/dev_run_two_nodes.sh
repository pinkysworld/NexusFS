#!/usr/bin/env bash
set -euo pipefail

# Dev helper: run two NexusFS daemons locally on different ports/data dirs.
# Requires: cargo

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mkdir -p "$ROOT_DIR/.dev/nodeA" "$ROOT_DIR/.dev/nodeB"

cat > "$ROOT_DIR/.dev/nodeA/nexusfs.toml" <<'EOF'
[node]
data_dir = "./.dev/nodeA/data"
device_name = "nodeA"

[net]
listen = "127.0.0.1:4444"
peers = []
tofu = true

[admin]
bind = "127.0.0.1:7070"
token = ""

[s3]
enabled = false
bind = "127.0.0.1:9000"

[posix]
enabled = false
mountpoint = "/mnt/nexus"

[security]
encrypt_at_rest = false
proof_mode = "transparent"

[energy]
enabled = true
battery_low_pct = 20
temp_high_c = 70
EOF

cat > "$ROOT_DIR/.dev/nodeB/nexusfs.toml" <<'EOF'
[node]
data_dir = "./.dev/nodeB/data"
device_name = "nodeB"

[net]
listen = "127.0.0.1:5555"
peers = []
tofu = true

[admin]
bind = "127.0.0.1:8080"
token = ""

[s3]
enabled = false
bind = "127.0.0.1:9001"

[posix]
enabled = false
mountpoint = "/mnt/nexus"

[security]
encrypt_at_rest = false
proof_mode = "transparent"

[energy]
enabled = true
battery_low_pct = 20
temp_high_c = 70
EOF

echo "Starting nodeA admin: http://127.0.0.1:7070"
cargo run -p nexusfs -- daemon --config "$ROOT_DIR/.dev/nodeA/nexusfs.toml" &
PID_A=$!

echo "Starting nodeB admin: http://127.0.0.1:8080"
cargo run -p nexusfs -- daemon --config "$ROOT_DIR/.dev/nodeB/nexusfs.toml" &
PID_B=$!

trap 'kill $PID_A $PID_B 2>/dev/null || true' EXIT
wait
