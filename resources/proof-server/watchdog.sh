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

# Whether xrpld has been in sync since its database was last wiped; the
# wipe in startup.sh clears it too.
state=/var/lib/xrpld-watchdog
# Since when it has been out of sync. Under /run, so that a boot starts the
# count afresh: time the VM spent stopped is not time xrpld spent stuck.
run=/run/xrpld-watchdog
db=/var/lib/xrpld/db
# Longer than xrpld takes to catch up after a restart or a network outage,
# shorter than a node is willing to wait.
stuck_limit=1800

# Only the testnet resets. Whatever installs this, a mainnet server is
# never wiped.
network=$(curl -fsS -m 5 -H 'Metadata-Flavor: Google' \
  http://metadata.google.internal/computeMetadata/v1/instance/attributes/proof-network || true)
[ "$network" = testnet ] || exit 0

mkdir -p "$state" "$run"
now=$(date +%s)

info=$(curl -fsS -m 10 http://127.0.0.1:5005 -d '{"method":"server_info"}' || true)
# xrpld not answering is systemd's to restart, not a chain gone: a wipe
# would not help, so it neither starts nor ends a stretch out of sync.
[ -n "$info" ] || exit 0
server_state=$(printf '%s' "$info" | jq -r '.result.info.server_state // "none"' 2> /dev/null || echo none)
# Seconds since the newest validated ledger closed: in sync means a fresh one.
age=$(printf '%s' "$info" | jq -r '.result.info.validated_ledger.age // 999999' 2> /dev/null || echo 999999)
# Seconds this run of xrpld has been up.
uptime=$(printf '%s' "$info" | jq -r '.result.info.uptime // 0' 2> /dev/null || echo 0)

case "$server_state" in
  full | tracking | proposing | validating)
    if [ "$age" -lt 120 ]; then
      touch "$state/synced"
      rm -f "$run/stuck-since"
      exit 0
    fi
    ;;
esac

# Not synced. Before the first sync after a wipe, that is the sync itself.
[ -f "$state/synced" ] || exit 0

[ -f "$run/stuck-since" ] || echo "$now" > "$run/stuck-since"
since=$(cat "$run/stuck-since")
# A stretch counts within one run of xrpld only. One that restarts, from a
# crash loop or by hand, starts it afresh: a reset leaves xrpld running on
# the old chain, and a crashing xrpld is not helped by a wipe.
if [ $((now - since)) -gt "$uptime" ]; then
  since=$((now - uptime))
  echo "$since" > "$run/stuck-since"
fi
if [ $((now - since)) -lt "$stuck_limit" ]; then
  exit 0
fi

echo "proof-server: out of sync for $((now - since)) s (state $server_state, validated ledger $age s old), wiping the database" > /dev/console
systemctl stop xrpld.service
rm -rf "$db"
rm -f "$state/synced" "$run/stuck-since"
systemctl start xrpld.service
