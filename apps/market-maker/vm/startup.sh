#!/bin/sh
# The boot script of the market maker's VM
# (spec/areas/market-maker-vm.md), which GCE runs as root at every boot
# from the instance metadata (provision.sh puts it there). It is
# idempotent.
#
# 1. Installs curl, Git, jq and minisign, and makes the system user
#    dassie-mm, which the nodes and the market maker run as.
# 2. Clones the repository, or brings it to the newest `main`: the units
#    and the scripts they run come from there.
# 3. Installs the newest release, if there is no install yet, with the
#    checkout's install.sh and `--no-service`
#    (spec/areas/release.md#installsh): the install layout `dassie update`
#    keeps current, without dassie.service on ports 80 and 443, which this
#    VM does not run.
# 4. Writes the settings from the instance metadata, and the targets' node
#    IDs, which pin both nodes to every target.
# 5. Installs the units, and (re)starts both nodes, the market maker and
#    the daily update.
#
# A change to this script reaches the VM only through its metadata
# (`gcloud compute instances add-metadata dassie-mm --zone=us-east1-b
# --metadata-from-file=startup-script=apps/market-maker/vm/startup.sh`); a
# change to the units, the other scripts or the settings, at its next boot
# (`gcloud compute instances reset dassie-mm --zone=us-east1-b`).
set -eu
# GCE runs this without a home, which Git needs.
export HOME=/root

repository=https://github.com/justmoon/dassie.git
checkout=/opt/dassie-mm/repo
vm="$checkout/apps/market-maker/vm"
base=/opt/dassie
mirror=https://get.dassie.land
config=/etc/dassie-mm
metadata=http://metadata.google.internal/computeMetadata/v1

fail() {
  echo "Error: $*" >&2
  exit 1
}

export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -qy --no-install-recommends ca-certificates curl git jq minisign

id -u dassie-mm > /dev/null 2>&1 ||
  useradd --system --user-group --no-create-home --home-dir /nonexistent \
    --shell /usr/sbin/nologin dassie-mm

# --- The repository -----------------------------------------------------------

umask 022
mkdir -p "$(dirname "$checkout")"
if [ ! -d "$checkout/.git" ]; then
  rm -rf "$checkout"
  git -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 \
    clone --quiet "$repository" "$checkout"
fi
# A transfer slower than 1 kB/s for a minute stops, rather than holding
# the boot.
git -C "$checkout" -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 \
  fetch --quiet origin main
git -C "$checkout" checkout --quiet --force --detach origin/main

# --- The release ----------------------------------------------------------------

# The newest release, verified as install.sh verifies it
# (spec/decisions/release-integrity.md), by install.sh itself: the
# checkout's, with the checkout's release key written in, as a release
# writes it. It also links /usr/local/bin/dassie, for the owner's
# `dassie status` with DASSIE_IPC_SOCKET_PATH. An install whose binary
# names no version is taken for none, as install.sh takes it.
if ! "$base/current/bin/dassie" --version > /dev/null 2>&1; then
  key=$(sed -n 2p "$checkout/resources/minisign.pub")
  installer=$(mktemp -t dassie-mm-install-XXXXXXXXXX)
  sed "s|@RELEASE_KEY@|$key|" "$checkout/resources/installer/install.sh" > "$installer"
  sh "$installer" --mirror "$mirror" --no-service || fail "the installer failed"
  rm -f "$installer"
fi

# --- The settings ---------------------------------------------------------------

attributes=$(curl -fsS --max-time 10 -H "Metadata-Flavor: Google" \
  "$metadata/instance/attributes/?recursive=true&alt=json") ||
  fail "cannot read the instance metadata"

# setting <name> <default>: the instance metadata's attribute, or the
# default where there is none.
setting() {
  printf '%s' "$attributes" |
    jq -r --arg name "$1" --arg default "$2" '.[$name] // $default'
}

# number <name> <default>: a setting that is a whole number.
number() {
  value=$(setting "$1" "$2")
  case "$value" in
    "" | *[!0-9]*) fail "the metadata's $1 is not a whole number" ;;
  esac
  [ "${#value}" -le 19 ] || fail "the metadata's $1 is too large"
  printf '%s' "$value"
}

# The defaults are starting points. Amounts are in the nodes' ledger units,
# a billionth of an XRP or a dollar. The XRP band's top, 80 XRP, is below
# the faucet's grant of 100 XRP (about 99 after the reserve), so the first
# rebalance buys DSUSD into the DSUSD node: up to the DSUSD band's top of
# 40, which costs about 32 XRP at the testnet pool's 0.76 XRP a DSUSD.
# That leaves about 67 XRP, below the top with room for several trades
# (and still below it at a DSUSD of half an XRP, 20 XRP for 40). So the
# two bands together hold the whole grant, and neither node is left above
# its band with the other full, where a rebalance has nowhere to go.
#
# The targets, the core nodes to trade through: the metadata's mm-targets,
# URLs separated by commas, or else its mm-target-url, the one target of
# the release before, or else boot1–boot3 (boot4 runs no DSUSD scheme). At most as many
# as a node pins (MAX_TARGETS in crates/dassie-cli/src/market_maker/
# config.rs), each once.
max_targets=128
listed=$(setting mm-targets "")
source=mm-targets
if [ -z "$listed" ]; then
  listed=$(setting mm-target-url "https://boot1.dassie.xyz,https://boot2.dassie.xyz,https://boot3.dassie.xyz")
  source=mm-target-url
fi
targets=""
count=0
old_ifs=$IFS
IFS=,
set -f
for target in $listed; do
  IFS=$old_ifs
  target=$(printf '%s' "$target" | tr -d ' \t\r\n')
  target=${target%/}
  [ -n "$target" ] || continue
  case "$target" in
    https://?*) ;;
    *) fail "the metadata's $source names $target, not an https:// URL" ;;
  esac
  case "$target" in
    *[!A-Za-z0-9.:/-]*) fail "the metadata's $source names $target, not a plain URL" ;;
  esac
  case ",$targets," in
    *",$target,"*) fail "the metadata's $source names $target twice" ;;
  esac
  count=$((count + 1))
  [ "$count" -le "$max_targets" ] || fail "the metadata's $source names more than $max_targets targets"
  targets=${targets:+$targets,}$target
done
IFS=$old_ifs
set +f
[ "$count" -ge 1 ] || fail "the metadata's $source names no target"
# Each on its own, so that one that is not a number stops the script.
xrp_trade_size=$(number mm-xrp-trade-size 2000000000)
xrp_band_low=$(number mm-xrp-band-low 20000000000)
xrp_band_high=$(number mm-xrp-band-high 80000000000)
dsusd_trade_size=$(number mm-dsusd-trade-size 2000000000)
dsusd_band_low=$(number mm-dsusd-band-low 5000000000)
dsusd_band_high=$(number mm-dsusd-band-high 40000000000)
margin_ppm=$(number mm-margin-ppm 10000)
slippage_ppm=$(number mm-slippage-ppm 10000)
umask 022
mkdir -p "$config"
cat > "$config/settings.env.new" <<EOF
MM_TARGETS=$targets
MM_XRP_TRADE_SIZE=$xrp_trade_size
MM_XRP_BAND_LOW=$xrp_band_low
MM_XRP_BAND_HIGH=$xrp_band_high
MM_DSUSD_TRADE_SIZE=$dsusd_trade_size
MM_DSUSD_BAND_LOW=$dsusd_band_low
MM_DSUSD_BAND_HIGH=$dsusd_band_high
MM_MARGIN_PPM=$margin_ppm
MM_SLIPPAGE_PPM=$slippage_ppm
EOF
mv -f "$config/settings.env.new" "$config/settings.env"

# The targets' node IDs, which both nodes pin as their uplinks: what each
# target's /stats names, which is where the market maker reads its ID and
# key too; for a single target, the metadata's mm-uplink if it is set. A
# target that does not answer keeps the ID read for it before
# ($config/target-ids, a URL and its ID per line); one never read is left
# out until a boot reads it, since the nodes would not start without
# their IDs. Two URLs of one node pin it once.
override=""
if [ "$count" = 1 ]; then override=$(setting mm-uplink ""); fi
# A VM set up for one target kept its ID in uplink.env instead: seed the
# list from it once, so a target that does not answer at this boot keeps
# the ID read before, as it did then.
if [ ! -f "$config/target-ids" ] && [ -f "$config/uplink.env" ]; then
  old_url=$(sed -n 's/^DASSIE_MM_UPLINK_OF=//p' "$config/uplink.env")
  old_id=$(sed -n 's/^DASSIE_UPLINKS=//p' "$config/uplink.env")
  case "$old_id" in
    "" | *[!A-Za-z0-9_-]*) ;;
    *) if [ -n "$old_url" ]; then printf '%s %s\n' "$old_url" "$old_id" > "$config/target-ids"; fi ;;
  esac
fi
uplinks=""
: > "$config/target-ids.new"
IFS=,
set -f
for target in $targets; do
  IFS=$old_ifs
  uplink=$override
  if [ -z "$uplink" ]; then
    uplink=$(curl --proto =https -fsS --max-time 20 "$target/stats" | jq -er .node.id) || uplink=""
  fi
  if [ -z "$uplink" ] && [ -f "$config/target-ids" ]; then
    uplink=$(awk -v url="$target" '$1 == url { print $2; exit }' "$config/target-ids")
    if [ -n "$uplink" ]; then
      echo "Note: $target/stats does not answer; its node ID stays as it was read before"
    fi
  fi
  if [ -z "$uplink" ]; then
    echo "Note: no node ID for $target: neither node pins it until a boot reads it" >&2
    continue
  fi
  case "$uplink" in
    *[!A-Za-z0-9_-]*) fail "the node ID for $target is not one a node takes" ;;
  esac
  # As long as a node ID may be (crates/dassie-identity/src/node_id.rs).
  [ "${#uplink}" -ge 2 ] && [ "${#uplink}" -le 45 ] ||
    fail "the node ID for $target is not one a node takes"
  printf '%s %s\n' "$target" "$uplink" >> "$config/target-ids.new"
  case ",$uplinks," in
    *",$uplink,"*) echo "Note: $target is a node another target is too; it is pinned once" ;;
    *) uplinks=${uplinks:+$uplinks,}$uplink ;;
  esac
done
IFS=$old_ifs
set +f
[ -n "$uplinks" ] ||
  fail "no target named its node ID, and none was read before (mm-uplink names it for a single target)"
mv -f "$config/target-ids.new" "$config/target-ids"
printf 'DASSIE_UPLINKS=%s\n' "$uplinks" > "$config/uplink.env.new"
mv -f "$config/uplink.env.new" "$config/uplink.env"

for node in xrp dsusd; do
  install -m 644 "$vm/$node.env" "$config/$node.env"
done

# --- The units ------------------------------------------------------------------

for unit in dassie-mm-node@.service dassie-mm.service dassie-mm-update.service dassie-mm-update.timer; do
  install -m 644 "$vm/$unit" "/etc/systemd/system/$unit"
done
systemctl daemon-reload
systemctl enable dassie-mm.service dassie-mm-update.timer
systemctl restart dassie-mm-update.timer
# The market maker stops first, so that it does not restart with each node;
# then both nodes start on the units and settings just written, and it
# after them. A node that does not start keeps restarting, and the market
# maker waits for it, trying again every RestartSec.
systemctl stop dassie-mm.service
systemctl restart dassie-mm-node@xrp.service dassie-mm-node@dsusd.service ||
  echo "Note: a node did not start; see journalctl -u 'dassie-mm-node@*'" >&2
systemctl start dassie-mm.service
