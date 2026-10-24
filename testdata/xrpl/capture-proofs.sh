#!/usr/bin/env bash
# Captures the proof servers' answers for crates/dassie-xrpl/tests/data/proof-api.json
# (spec/designs/xrpl-validation.md#13-what-pins-it), read-only, over JSON-RPC
# at API version 1, on each network:
#
# - an anchoring round as the account of the tests' seed ([7; 32], whose
#   account no ledger holds) asks it: `state_proof` at the validated ledger
#   of the skip list of recent ledgers, the account's root and the fee
#   settings, and `validations` of that ledger;
# - at that ledger: its binary header from `ledger`, and `state_proof` of
#   the skip list, an account root, a trust line and keys it does not hold;
# - a transaction of the newest ledger at or below it with a few: `tx` with
#   binary, and `tx_proof` in its ledger.
#
# Usage: testdata/xrpl/capture-proofs.sh > crates/dassie-xrpl/tests/data/proof-api.json
set -euo pipefail

SKIP_LIST=B4979A36CDC7F3D3D5C31A4EAE2AC7D7209DDA877588B9AFC66799692AB0D66B
FEES=4BC50C9B0D8515D3EAAE1E74B29A95804346C491EE1A95BF25E4AAB854A6A651
# The root of rEhh6f9rj5UUBhFzGGaxS5zYU2CCqKFXBC, the account of entropy [7; 32].
TEST_ROOT=A294ECE048F4CD2BB2B4AD6EED9DB5F0E56E40B72BC8FB9BB99302DD50BCDB89
# Keys no ledger holds, by chance: each ends its branch at an empty slot or
# at another key's leaf.
ABSENT_KEYS='["0000000000000000000000000000000000000000000000000000000000000001","D4551E0000000000000000000000000000000000000000000000000000000000","7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"]'

rpc() {
  local url=$1 method=$2 params=$3
  curl -sS -m 60 "$url" -H 'content-type: application/json' \
    -d "{\"method\":\"$method\",\"params\":[$(jq -c '. + {api_version: 1}' <<<"$params")]}" |
    jq '.result'
}

# network url line_holder line_issuer line_currency
capture() {
  local network=$1 url=$2 holder=$3 issuer=$4 currency=$5
  local captured anchor hash index validations header
  captured=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  anchor=$(rpc "$url" state_proof \
    "{\"keys\":[\"$SKIP_LIST\",\"$TEST_ROOT\",\"$FEES\"],\"ledger_index\":\"validated\"}")
  hash=$(jq -r '.ledger_hash' <<<"$anchor")
  index=$(jq '.ledger_index' <<<"$anchor")
  validations=$(rpc "$url" validations "{\"ledger_hash\":\"$hash\"}")
  header=$(rpc "$url" ledger "{\"ledger_hash\":\"$hash\",\"binary\":true}")
  # The newest ledger at or below the anchor with a few transactions, so a
  # branch has more than the root above its leaf.
  local at count tx_hash
  for ((at = index; at > index - 20; at--)); do
    count=$(rpc "$url" ledger "{\"ledger_index\":$at,\"transactions\":true}" |
      jq '.ledger.transactions | length')
    if ((count >= 3)); then break; fi
  done
  tx_hash=$(rpc "$url" ledger "{\"ledger_index\":$at,\"transactions\":true}" |
    jq -r '.ledger.transactions[1]')
  local tx tx_proof account root line keys state
  tx=$(rpc "$url" tx "{\"transaction\":\"$tx_hash\",\"binary\":true}")
  tx_proof=$(rpc "$url" tx_proof "{\"tx_hash\":\"$tx_hash\",\"ledger_index\":$at}")
  account=$(rpc "$url" tx "{\"transaction\":\"$tx_hash\"}" | jq -r '.Account')
  root=$(rpc "$url" ledger_entry "{\"account_root\":\"$account\",\"ledger_hash\":\"$hash\"}" |
    jq -r '.index')
  line=$(rpc "$url" ledger_entry \
    "{\"ripple_state\":{\"accounts\":[\"$holder\",\"$issuer\"],\"currency\":\"$currency\"},\"ledger_hash\":\"$hash\"}" |
    jq -r '.index')
  keys=$(jq -c --arg skip "$SKIP_LIST" --arg root "$root" --arg line "$line" \
    '[$skip, $root, $line] + .' <<<"$ABSENT_KEYS")
  state=$(rpc "$url" state_proof "{\"keys\":$keys,\"ledger_hash\":\"$hash\"}")
  jq -n --arg network "$network" --arg url "$url" --arg captured "$captured" \
    --arg account "$account" --arg holder "$holder" --arg issuer "$issuer" \
    --argjson anchor "$anchor" --argjson validations "$validations" \
    --argjson header "$header" --argjson tx "$tx" --argjson tx_proof "$tx_proof" \
    --argjson state "$state" \
    '{network: $network, source: $url, captured: $captured,
      tx_account: $account, line: {holder: $holder, issuer: $issuer},
      anchor: $anchor, validations: $validations, ledger: $header,
      state_proof: $state, tx: $tx, tx_proof: $tx_proof}'
}

{
  capture mainnet https://mainnet.xrpl.dassieland.net \
    rhWTXC2m2gGGA9WozUaoMm6kLAVPb1tcS3 rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De \
    524C555344000000000000000000000000000000
  capture testnet https://testnet.xrpl.dassieland.net \
    rEHay2mS1P7cxn982mMJ7p9uYyB7j3pymT rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M \
    4453555344000000000000000000000000000000
} | jq -s \
  '{source: "Answers of the proof servers at mainnet.xrpl.dassieland.net and testnet.xrpl.dassieland.net (rippled 3.5.0-rc1+proofs.3, with the patch in resources/proof-server/patches), as they gave them over JSON-RPC at API version 1, by testdata/xrpl/capture-proofs.sh. On each network: an anchoring round of the account of entropy [7; 32] (`state_proof` at the validated ledger of the skip list of recent ledgers, its root, which no ledger holds, and the fee settings, then `validations` of that ledger); at that ledger, `ledger` with binary and `state_proof` of the skip list, an account root, a trust line and three keys it does not hold; and a transaction of the newest ledger at or below it with a few, by `tx` with binary and `tx_proof`. Each ledger says when it was captured.",
    ledgers: .}'
