#!/bin/sh
# Runs the market maker on its VM (spec/areas/market-maker-vm.md), as
# dassie-mm, once dassie-mm.service's ExecStartPre put the two passphrases
# in its runtime directory (phrases.sh).
#
# 1. Sets each node up from its phrase with `dassie setup`, which claims a
#    fresh node, checks that one set up before is the phrase's, and adds
#    the node's one settlement scheme if it is missing.
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

# setup <node> <scheme>
setup() {
  DASSIE_IPC_SOCKET_PATH="/run/dassie-mm-$1/dassie.sock" \
    "$dassie" setup --passphrase-file "$dir/$1.phrase" --scheme "$2"
}

setup xrp xrpl-testnet
setup dsusd xrpl-testnet-dsusd

umask 077
# The targets: one in the form every release reads, so that a boot that
# brings these scripts before the release that reads `targets` still runs;
# several as `targets`.
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
  '($targets | split(",")) as $urls
  | (if ($urls | length) == 1 then { target: { url: $urls[0] } } else { targets: $urls } end)
  + {
    xrpl: { server: $server, token: { currency: "DSUSD", issuer: $issuer } },
    xrp: {
      socket: "/run/dassie-mm-xrp/dassie.sock",
      ledger: "xrpl-testnet+xrp",
      passphraseFile: "\($dir)/xrp.phrase",
      tradeSize: $xrpTradeSize,
      band: { low: $xrpLow, high: $xrpHigh }
    },
    token: {
      socket: "/run/dassie-mm-dsusd/dassie.sock",
      ledger: "xrpl-testnet-dsusd+usd",
      passphraseFile: "\($dir)/dsusd.phrase",
      tradeSize: $dsusdTradeSize,
      band: { low: $dsusdLow, high: $dsusdHigh }
    },
    marginPpm: $marginPpm,
    slippagePpm: $slippagePpm
  }' > "$dir/config.json"

exec "$dassie" market-maker --config "$dir/config.json"
