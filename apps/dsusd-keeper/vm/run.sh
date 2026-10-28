#!/bin/sh
# The DSUSD keeper on its VM (spec/areas/dsusd-keeper.md#its-vm), from the
# repository's root, as dassie-dsusd-keeper.service starts it: it follows
# the pool for an hour.
#
# It reads the issuer's and the keeper's seeds from Secret Manager with the
# VM's service account, into this process's environment only: they never touch
# the disk. Then it installs the keeper's one dependency from the lockfile
# and runs it, with Node and pnpm of the flake's nixpkgs.
set -eu

metadata=http://metadata.google.internal/computeMetadata/v1

from_metadata() {
  curl -fsS --max-time 10 -H "Metadata-Flavor: Google" "$metadata/$1"
}

project=$(from_metadata project/project-id)
token=$(from_metadata instance/service-accounts/default/token | jq -er .access_token)

secret() {
  curl -fsS --max-time 20 -H "Authorization: Bearer $token" \
    "https://secretmanager.googleapis.com/v1/projects/$project/secrets/$1/versions/latest:access" |
    jq -er .payload.data | base64 -d
}

DSUSD_ISSUER_SEED=$(secret dsusd-issuer-seed)
DSUSD_KEEPER_SEED=$(secret dsusd-keeper-seed)
# A failed request leaves a seed empty rather than stopping the pipeline;
# the keeper refuses to run without both.
export DSUSD_ISSUER_SEED DSUSD_KEEPER_SEED

exec /nix/var/nix/profiles/default/bin/nix shell --inputs-from . \
  nixpkgs#nodejs_26 nixpkgs#pnpm_11 --command sh -c '
    pnpm install --frozen-lockfile --prod --filter @dassie/dsusd-keeper \
      --reporter=silent --config.update-notifier=false &&
    exec node apps/dsusd-keeper/main.ts'
