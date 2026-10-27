#!/bin/sh
# Runs the market maker on its VM (spec/areas/market-maker-vm.md), as
# dassie-mm, once dassie-mm.service's ExecStartPre put the two passphrases
# in its runtime directory (phrases.sh).
#
# 1. Sets each node up from its phrase with `dassie setup`, which claims a
#    fresh node, checks that one set up before is the phrase's, and adds
#    the schemes it names that are missing: each node joins the testnet's
#    default hub of its asset, the XRP node the XRP hub and the DSUSD node
#    the DSUSD hub, with the owner books it funds the hub from, which peer
#    with nobody: XRP's, and DSUSD's on the DSUSD node, whose XRP book pays
#    its hub's gas (spec/areas/hubs.md#in-the-node). `--no-hub`: the
#    DSUSD node joins no XRP hub, and neither joins a hub it does not name.
#    A hub's scheme names no rules, so it takes the default hub of its
#    asset, which runs only once its rules are built into the release
#    (spec/areas/market-maker-vm.md). It changes no scheme a node already
#    runs, so a node with a database from before hubs starts afresh.
# 2. Writes the market maker's config (crates/dassie-cli/src/market_maker/
#    config.rs) beside the phrases, from the settings startup.sh wrote
#    from the instance metadata (MM_*, in /etc/dassie-mm/settings.env).
# 3. Runs `dassie market-maker` in its place, so that systemd stops it.
set -eu

dir=${RUNTIME_DIRECTORY:?"run this from dassie-mm.service"}
dassie=/opt/dassie/current/bin/dassie
# The testnet's public server, and DSUSD's issuer (spec/decisions/dsusd.md),
# which the node's DSUSD scheme names too.
xrpl_server=wss://s.altnet.rippletest.net:51233
dsusd_issuer=rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M

# An XRP Ledger scheme's config as an owner book, which peers with nobody.
book='{"peers":false}'

# setup <node> <dassie setup's flags>…
setup() {
  node=$1
  shift
  DASSIE_IPC_SOCKET_PATH="/run/dassie-mm-$node/dassie.sock" \
    "$dassie" setup --passphrase-file "$dir/$node.phrase" --no-hub "$@"
}

setup xrp \
  --scheme "xrpl-testnet-xrp=$book" \
  --scheme hub-xrpl-testnet-xrp
setup dsusd \
  --scheme "xrpl-testnet-xrp=$book" \
  --scheme "xrpl-testnet-dsusd=$book" \
  --scheme hub-xrpl-testnet-dsusd

umask 077
jq -n \
  --arg targets "${MM_TARGETS:?}" \
  --arg server "$xrpl_server" \
  --arg issuer "$dsusd_issuer" \
  --arg dir "$dir" \
  --argjson xrpTradeSize "${MM_XRP_TRADE_SIZE:?}" \
  --argjson xrpLow "${MM_XRP_BAND_LOW:?}" \
  --argjson xrpHigh "${MM_XRP_BAND_HIGH:?}" \
  --argjson dsusdTradeSize "${MM_DSUSD_TRADE_SIZE:?}" \
  --argjson dsusdLow "${MM_DSUSD_BAND_LOW:?}" \
  --argjson dsusdHigh "${MM_DSUSD_BAND_HIGH:?}" \
  --argjson marginPpm "${MM_MARGIN_PPM:?}" \
  --argjson slippagePpm "${MM_SLIPPAGE_PPM:?}" \
  '{
    targets: ($targets | split(",")),
    xrpl: { server: $server, token: { currency: "DSUSD", issuer: $issuer } },
    xrp: {
      socket: "/run/dassie-mm-xrp/dassie.sock",
      ledger: "hub-xrpl-testnet-xrp+xrp",
      ownerBook: "xrpl-testnet-xrp+xrp",
      passphraseFile: "\($dir)/xrp.phrase",
      tradeSize: $xrpTradeSize,
      band: { low: $xrpLow, high: $xrpHigh }
    },
    token: {
      socket: "/run/dassie-mm-dsusd/dassie.sock",
      ledger: "hub-xrpl-testnet-dsusd+usd",
      ownerBook: "xrpl-testnet-dsusd+usd",
      passphraseFile: "\($dir)/dsusd.phrase",
      tradeSize: $dsusdTradeSize,
      band: { low: $dsusdLow, high: $dsusdHigh }
    },
    marginPpm: $marginPpm,
    slippagePpm: $slippagePpm
  }' > "$dir/config.json"

exec "$dassie" market-maker --config "$dir/config.json"
