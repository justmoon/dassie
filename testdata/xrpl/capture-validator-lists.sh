#!/usr/bin/env bash
# Captures crates/dassie-xrpl/tests/data/validator-lists.json
# (spec/designs/xrpl-validation.md#13-what-pins-it): the signed validator
# list each publisher's site serves, each body as it came, mainnet's in the
# order its config names the publishers.
#
# Usage: testdata/xrpl/capture-validator-lists.sh > crates/dassie-xrpl/tests/data/validator-lists.json
set -euo pipefail

SITES=(
  "mainnet https://vl.ripple.com"
  "mainnet https://unl.xrplf.org"
  "testnet https://vl.altnet.rippletest.net"
)

# Each body goes through a file, so that it keeps every byte.
body=$(mktemp)
trap 'rm -f "$body"' EXIT
captured=$(date -u +%Y-%m-%dT%H:%M:%SZ)
for entry in "${SITES[@]}"; do
  read -r network site <<<"$entry"
  curl -sS --fail -m 60 -o "$body" "$site"
  # A list, not an error page: its publisher's manifest and a signed blob.
  jq -e '.manifest and (.blob or .blobs_v2)' "$body" >/dev/null
  jq -n --arg network "$network" --arg site "$site" --rawfile body "$body" \
    '{network: $network, site: $site, body: $body}'
done | jq -s --arg captured "$captured" \
  '{source: "The signed validator lists the publishers'"'"' sites served, each body as it came: https://vl.ripple.com and https://unl.xrplf.org for mainnet, https://vl.altnet.rippletest.net for the testnet. Captured \($captured) by testdata/xrpl/capture-validator-lists.sh.",
    lists: .}'
