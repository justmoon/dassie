#!/usr/bin/env bash
# Regenerates the minisign vectors of crates/dassie-release with the
# minisign CLI, as the release workflow signs (spec
# spec/decisions/release-integrity.md):
#
#   test.pub, test.key        the test key: unencrypted, for tests only,
#                             never trusted by a release build
#   SHA256SUMS                a release's sums, of stand-in bundles
#   SHA256SUMS.minisig        its signature, trusted comment "dassie 0.1.0"
#   SHA256SUMS.legacy.minisig the same, signed without prehashing (-l)
#
# Run from this directory: nix shell nixpkgs#minisign -c ./generate.sh
# A new key changes every signature; keep the key unless it must change.
set -euo pipefail
cd "$(dirname "$0")"

if [ ! -f test.key ]; then
  minisign -G -W -p test.pub -s test.key
fi

: > SHA256SUMS
for arch in arm64 x64; do
  for ext in gz xz; do
    name="dassie-0.1.0-linux-$arch.tar.$ext"
    printf '%s  %s\n' "$(printf 'stand-in for %s' "$name" | sha256sum | cut -d' ' -f1)" "$name" >> SHA256SUMS
  done
done

minisign -S -s test.key -m SHA256SUMS -x SHA256SUMS.minisig -t "dassie 0.1.0"
minisign -S -l -s test.key -m SHA256SUMS -x SHA256SUMS.legacy.minisig -t "dassie 0.1.0"
minisign -V -p test.pub -m SHA256SUMS -x SHA256SUMS.minisig
