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
# 2. Writes the market maker's config (crates/dassie-cli/src/market_maker/
#    config.rs) beside the phrase, from the settings startup.sh wrote from
#    the instance metadata (MM_*, in /etc/dassie-mm/settings.env). It names
#    no targets: they are the node's peers, the uplinks it is pinned to.
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
# (spec/areas/rate-feed.md#bands, spec/decisions/one-balance-per-asset.md).
DASSIE_IPC_SOCKET_PATH=$socket "$dassie" setup \
  --passphrase-file "$dir/node.phrase" \
  --hub hub-xrpl-testnet-xrp --hub hub-xrpl-testnet-dsusd

umask 077
jq -n \
  --arg socket "$socket" \
  --arg phrase "$dir/node.phrase" \
  --arg server "$xrpl_server" \
  --arg issuer "$dsusd_issuer" \
  --argjson xrpTradeSize "${MM_XRP_TRADE_SIZE:?}" \
  --argjson xrpLow "${MM_XRP_BAND_LOW:?}" \
  --argjson xrpHigh "${MM_XRP_BAND_HIGH:?}" \
  --argjson dsusdTradeSize "${MM_DSUSD_TRADE_SIZE:?}" \
  --argjson dsusdLow "${MM_DSUSD_BAND_LOW:?}" \
  --argjson dsusdHigh "${MM_DSUSD_BAND_HIGH:?}" \
  --argjson marginPpm "${MM_MARGIN_PPM:?}" \
  --argjson slippagePpm "${MM_SLIPPAGE_PPM:?}" \
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

exec "$dassie" market-maker --config "$dir/config.json"
