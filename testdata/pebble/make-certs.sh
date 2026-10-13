#!/bin/sh
# Makes the TLS certificate Pebble serves its ACME API with: a test CA
# (root.pem), and a leaf for localhost signed by it (cert.pem, key.pem).
# The CA's key is thrown away, so nothing else can be signed with it.
#
# The daemon trusts root.pem through DASSIE_ACME_CA_FILE in the end-to-end
# test. The files are committed; run this only to replace them.
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout "$work/ca.key" -out root.pem -days 36500 \
  -subj "/CN=Dassie test ACME API CA" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign"

openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout key.pem -out "$work/leaf.csr" -subj "/CN=localhost"

printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n' \
  > "$work/leaf.ext"
openssl x509 -req -in "$work/leaf.csr" -CA root.pem -CAkey "$work/ca.key" \
  -CAcreateserial -CAserial "$work/ca.srl" -days 36500 \
  -extfile "$work/leaf.ext" -out cert.pem
