#!/bin/sh
# The testnet proof server's watchdog (README.md#after-a-testnet-reset),
# which xrpld-watchdog.timer runs every few minutes.
#
# A testnet reset starts a new chain at a low ledger index, and xrpld never
# takes a validated ledger below the one it holds: it stays out of sync for
# good, and every node that uses it stalls. So once xrpld has been in sync,
# a stretch out of sync longer than any outage it rides out by itself means
# the chain it holds is gone: the watchdog wipes the database and xrpld
# syncs the new chain from nothing. Wiping costs only a resync, so a false
# alarm costs an hour of downtime, never a wrong answer.
set -eu

state=/var/lib/xrpld-watchdog
db=/var/lib/xrpld/db
# Longer than xrpld takes to catch up after a restart or a network outage,
# shorter than a node is willing to wait.
stuck_limit=1800

mkdir -p "$state"
now=$(date +%s)

info=$(curl -fsS -m 10 http://127.0.0.1:5005 -d '{"method":"server_info"}' || true)
server_state=$(printf '%s' "$info" | jq -r '.result.info.server_state // "none"' 2> /dev/null || echo none)
# Seconds since the newest validated ledger closed: in sync means a fresh one.
age=$(printf '%s' "$info" | jq -r '.result.info.validated_ledger.age // 999999' 2> /dev/null || echo 999999)

case "$server_state" in
  full | tracking | proposing | validating)
    if [ "$age" -lt 120 ]; then
      touch "$state/synced"
      rm -f "$state/stuck-since"
      exit 0
    fi
    ;;
esac

# Not synced. Before the first sync after a wipe, that is the sync itself.
[ -f "$state/synced" ] || exit 0

[ -f "$state/stuck-since" ] || echo "$now" > "$state/stuck-since"
since=$(cat "$state/stuck-since")
if [ $((now - since)) -lt "$stuck_limit" ]; then
  exit 0
fi

echo "proof-server: out of sync for $((now - since)) s (state $server_state, validated ledger $age s old), wiping the database" > /dev/console
systemctl stop xrpld.service
rm -rf "$db"
rm -f "$state/synced" "$state/stuck-since"
systemctl start xrpld.service
