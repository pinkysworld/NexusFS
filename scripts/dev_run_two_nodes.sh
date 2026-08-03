#!/usr/bin/env bash
# Run two NexusFS nodes locally and watch them converge.
#
# Seeds each node with a different file while both are stopped, then starts them as
# peers. Within a few seconds both should report the same state root and hold both
# files — including the deterministically renamed copy of the directory each created
# independently while apart.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEV="$ROOT_DIR/.dev"
BIN="${CARGO_TARGET_DIR:-$ROOT_DIR/target}/debug/nexusfs"

cleanup() {
  [[ -n "${A_PID:-}" ]] && kill "$A_PID" 2>/dev/null || true
  [[ -n "${B_PID:-}" ]] && kill "$B_PID" 2>/dev/null || true
}
trap cleanup EXIT

rm -rf "$DEV"
mkdir -p "$DEV"

write_config() {
  local name=$1 listen=$2 peer=$3 admin=$4 s3=$5
  cat > "$DEV/$name.toml" <<EOF
[node]
data_dir = "$DEV/$name"
device_name = "$name"

[net]
listen = "127.0.0.1:$listen"
peers = ["127.0.0.1:$peer"]
tofu = true
sync_interval_secs = 2

[admin]
bind = "127.0.0.1:$admin"
token = "$name-token"

[s3]
enabled = false
bind = "127.0.0.1:$s3"
token = ""

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
}

write_config alice 4444 4445 7070 9000
write_config bob   4445 4444 7071 9001

echo "==> building"
cargo build -q -p nexusfs --features "admin,quic"

echo "==> seeding both nodes while they are stopped"
echo "written on alice" > "$DEV/a.txt"
echo "written on bob"   > "$DEV/b.txt"
"$BIN" mkdir --config "$DEV/alice.toml" /shared
"$BIN" put   --config "$DEV/alice.toml" "$DEV/a.txt" /shared/from-alice.txt
"$BIN" mkdir --config "$DEV/bob.toml"   /shared
"$BIN" put   --config "$DEV/bob.toml"   "$DEV/b.txt" /shared/from-bob.txt

echo "    alice: $("$BIN" status --config "$DEV/alice.toml" | grep state_root)"
echo "    bob:   $("$BIN" status --config "$DEV/bob.toml"   | grep state_root)"

echo "==> starting both daemons"
"$BIN" daemon --config "$DEV/alice.toml" > "$DEV/alice.log" 2>&1 &
A_PID=$!
"$BIN" daemon --config "$DEV/bob.toml" > "$DEV/bob.log" 2>&1 &
B_PID=$!

root_of() {
  curl -s -H "x-nexusfs-token: $2-token" "http://127.0.0.1:$1/api/status" \
    | sed -E 's/.*"state_root":"([^"]*)".*/\1/'
}

echo "==> waiting for convergence"
for _ in $(seq 1 30); do
  A=$(root_of 7070 alice 2>/dev/null || true)
  B=$(root_of 7071 bob 2>/dev/null || true)
  if [[ -n "$A" && "$A" == "$B" ]]; then
    echo
    echo "converged on $A"
    echo
    echo "alice /:"; curl -s -H "x-nexusfs-token: alice-token" "http://127.0.0.1:7070/api/fs/ls?path=/"
    echo
    echo "bob   /:"; curl -s -H "x-nexusfs-token: bob-token"   "http://127.0.0.1:7071/api/fs/ls?path=/"
    echo
    echo "peers:";   curl -s -H "x-nexusfs-token: alice-token" "http://127.0.0.1:7070/api/peers"
    echo
    echo
    echo "Consoles: http://127.0.0.1:7070 and http://127.0.0.1:7071"
    echo "Ctrl+C to stop."
    wait
  fi
  sleep 1
done

echo "did not converge within 30s; see $DEV/alice.log and $DEV/bob.log" >&2
exit 1
