#!/bin/sh
# Reads the passphrase of the market maker's node
# (spec/areas/market-maker-vm.md) from Secret Manager with the VM's service
# account, as root, before dassie-mm.service starts (its ExecStartPre=+).
# It goes into a file in the unit's runtime directory, on tmpfs: 0400,
# owned by dassie-mm, and gone when the unit stops. Nothing here prints the
# phrase or the token.
set -eu

dir=${RUNTIME_DIRECTORY:?"run this from dassie-mm.service"}
metadata=http://metadata.google.internal/computeMetadata/v1
secret=mm-phrase

from_metadata() {
  curl -fsS --max-time 10 -H "Metadata-Flavor: Google" "$metadata/$1"
}

project=$(from_metadata project/project-id)
token=$(from_metadata instance/service-accounts/default/token | jq -er .access_token)

umask 077
file="$dir/node.phrase"
staged="$dir/.node.phrase"
rm -f "$staged"
# The token goes to curl on its standard input, not its command line,
# which every process on the VM can read.
printf 'Authorization: Bearer %s\n' "$token" |
  curl -fsS --max-time 20 -H @- \
    "https://secretmanager.googleapis.com/v1/projects/$project/secrets/$secret/versions/latest:access" |
  jq -er .payload.data | base64 -d > "$staged" || true
# A failed request leaves the file empty rather than stopping the
# pipeline.
if [ ! -s "$staged" ]; then
  rm -f "$staged"
  echo "Error: cannot read the secret $secret" >&2
  exit 1
fi
chown dassie-mm:dassie-mm "$staged"
chmod 0400 "$staged"
mv -f "$staged" "$file"
