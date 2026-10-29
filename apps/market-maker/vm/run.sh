#!/bin/sh
# Runs the market maker on its VM (spec/areas/market-maker-vm.md), as
# dassie-mm, once dassie-mm.service's ExecStartPre put its node's
# passphrase in its runtime directory (phrase.sh).
#
# 1. Sets the node up from its phrase with `dassie setup`, which claims a
#    fresh node, checks that one set up before is the phrase's, and joins
#    the hubs `--hub` names, unless the node is in them: the testnet's
#    default hub of XRP and its default hub of DSUSD, with the owner books
#    it funds them from, which peer with nobody: XRP's, which also pays the
#    DSUSD hub's gas, and DSUSD's (spec/areas/hubs.md#in-the-node). A hub's
#    scheme names no rules, so the node stores the rules of the default hub
#    of its asset; setup fails until they are built into the release
#    (spec/areas/market-maker-vm.md). It changes no scheme a node already
#    runs, so a node with a database from before hubs starts afresh.
#    Then it sets each hub's target, what the node keeps there, to 10 of
#    the hub's asset.
# 2. Writes the market maker's config (crates/dassie-cli/src/market_maker/
#    config.rs) beside the phrase, from the settings startup.sh wrote from
#    the instance metadata (MM_*, in /etc/dassie-mm/settings.env). It names
#    no targets: they are the node's peers, the uplinks it is pinned to. A
#    release from before the market maker valued its inventory by a curve
#    refuses that config (`dassie market-maker --check`), and gets one of
#    bands instead, so it runs until the update timer installs a newer one.
# 3. Runs `dassie market-maker` in its place, so that systemd stops it.
set -eu

dir=${RUNTIME_DIRECTORY:?"run this from dassie-mm.service"}
dassie=/opt/dassie/current/bin/dassie
socket=/run/dassie-mm-node/dassie.sock
# The testnet's public server, and DSUSD's issuer (spec/decisions/dsusd.md),
# which the node's DSUSD scheme names too.
xrpl_server=wss://s.altnet.rippletest.net:51233
dsusd_issuer=rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M

# Each hub's funding sweeps what the node is paid beyond the ceiling into
# the owner book, which the market maker's rebalances spend from
# (spec/areas/rate-feed.md#holdings, spec/decisions/one-balance-per-asset.md).
DASSIE_IPC_SOCKET_PATH=$socket "$dassie" setup \
  --passphrase-file "$dir/node.phrase" \
  --hub hub-xrpl-testnet-xrp --hub hub-xrpl-testnet-dsusd

# Each hub keeps a hundred of the largest packets, 10 of its asset, not the
# default thousand: the rebalances spend from the owner books, so the node
# keeps little in its hubs, and a trade, which pays from a hub, pays at most
# the cap of 10 (spec/areas/market-maker-vm.md#settings). Set at every
# start, so a node set up before takes it too. Here rather than in the
# metadata's settings, since the caps' defaults rest on it, and this script
# comes from main at every boot, while the binary is the installed release: a
# release from before the target could be set knows no `hub target`, and
# keeps a hub's old ceiling of 10 anyway, so the market maker runs on it.
# Any other failure stops it, as it would rebalance from empty books.
hub_target=10
for hub in hub-xrpl-testnet-xrp hub-xrpl-testnet-dsusd; do
  if ! out=$(DASSIE_IPC_SOCKET_PATH=$socket "$dassie" hub target "$hub" "$hub_target" 2>&1); then
    case $out in
      *'unknown hub command "target"'*)
        echo "Note: this release sets no hub target; $hub keeps its own." >&2
        ;;
      *)
        echo "$out" >&2
        exit 1
        ;;
    esac
  fi
done

umask 077
jq -n \
  --arg socket "$socket" \
  --arg phrase "$dir/node.phrase" \
  --arg server "$xrpl_server" \
  --arg issuer "$dsusd_issuer" \
  --argjson xrpCap "${MM_XRP_CAP:-10000000000}" \
  --argjson dsusdCap "${MM_DSUSD_CAP:-10000000000}" \
  --argjson curvePpm "${MM_CURVE_PPM:-10000}" \
  --argjson marginPpm "${MM_MARGIN_PPM:-2000}" \
  --argjson thresholdPpm "${MM_THRESHOLD_PPM:-50000}" \
  --argjson slippagePpm "${MM_SLIPPAGE_PPM:-10000}" \
  '{
    socket: $socket,
    passphraseFile: $phrase,
    xrpl: { server: $server, token: { currency: "DSUSD", issuer: $issuer } },
    xrp: {
      ledger: "hub-xrpl-testnet-xrp+xrp",
      ownerBook: "xrpl-testnet-xrp+xrp",
      cap: $xrpCap
    },
    token: {
      ledger: "hub-xrpl-testnet-dsusd+usd",
      ownerBook: "xrpl-testnet-dsusd+usd",
      cap: $dsusdCap
    },
    curvePpm: $curvePpm,
    marginPpm: $marginPpm,
    thresholdPpm: $thresholdPpm,
    slippagePpm: $slippagePpm
  }' > "$dir/config.json"

# A release from before the curve knows no --check, and reads only a config
# of bands: it gets one, with the defaults its own startup.sh had, until
# the update timer installs a release that reads the config above. The
# settings it reads are the metadata's, where a VM set up before has them.
if ! checked=$("$dassie" market-maker --config "$dir/config.json" --check 2>&1); then
  # The check's own words, in case a release that reads the curve refused
  # the config for another reason, such as a setting out of its range.
  echo "Note: the release refuses the config ($checked); it gets one of bands" >&2
  jq -n \
    --arg socket "$socket" \
    --arg phrase "$dir/node.phrase" \
    --arg server "$xrpl_server" \
    --arg issuer "$dsusd_issuer" \
    --argjson xrpTradeSize "${MM_XRP_TRADE_SIZE:-2000000000}" \
    --argjson xrpLow "${MM_XRP_BAND_LOW:-20000000000}" \
    --argjson xrpHigh "${MM_XRP_BAND_HIGH:-80000000000}" \
    --argjson dsusdTradeSize "${MM_DSUSD_TRADE_SIZE:-2000000000}" \
    --argjson dsusdLow "${MM_DSUSD_BAND_LOW:-5000000000}" \
    --argjson dsusdHigh "${MM_DSUSD_BAND_HIGH:-40000000000}" \
    --argjson marginPpm "${MM_MARGIN_PPM:-10000}" \
    --argjson slippagePpm "${MM_SLIPPAGE_PPM:-10000}" \
    '{
      socket: $socket,
      passphraseFile: $phrase,
      xrpl: { server: $server, token: { currency: "DSUSD", issuer: $issuer } },
      xrp: {
        ledger: "hub-xrpl-testnet-xrp+xrp",
        ownerBook: "xrpl-testnet-xrp+xrp",
        tradeSize: $xrpTradeSize,
        band: { low: $xrpLow, high: $xrpHigh }
      },
      token: {
        ledger: "hub-xrpl-testnet-dsusd+usd",
        ownerBook: "xrpl-testnet-dsusd+usd",
        tradeSize: $dsusdTradeSize,
        band: { low: $dsusdLow, high: $dsusdHigh }
      },
      marginPpm: $marginPpm,
      slippagePpm: $slippagePpm
    }' > "$dir/config.json"
fi

exec "$dassie" market-maker --config "$dir/config.json"
