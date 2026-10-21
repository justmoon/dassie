#!/bin/sh
# Lets the release workflow route https://get.dassie.land/flake/latest.tar.gz
# (scripts/flake-latest.ts, spec/decisions/nix-flake-on-the-mirror.md).
#
#   scripts/flake-latest/provision.sh [--apply]
#
# Without --apply it prints the gcloud commands and runs nothing. With it,
# it runs them, as an owner of the project, once, before the first release
# whose workflow routes the flake.
#
# What it makes:
# - a backend bucket over the mirror's bucket without Cloud CDN, which the
#   route serves the flake from: the CDN would keep the old version under
#   the same URL;
# - a role that may read, validate and update URL maps, and use backend
#   buckets in them, and nothing else, for the release workflow's service
#   account. Compute has no roles on a single URL map, so it holds for the
#   project's, which serve get., sh. and dassie.land: what the release
#   workflow can change there is no more than it can already do by
#   replacing install.sh.
#
# The route itself, and the path matcher for get.dassie.land it lives in,
# the release workflow makes (scripts/flake-latest.ts).
set -eu

project=dassie-390023
bucket=get.dassie.land
backend=get-dassie-land-uncached
role=mirrorFlakeRoute
account=dassie-release@dassie-390023.iam.gserviceaccount.com

apply=0
if [ "${1:-}" = "--apply" ]; then apply=1; fi

run() {
  if [ "$apply" = 1 ]; then "$@"; else printf '%s\n' "$*"; fi
}

run gcloud compute backend-buckets create "$backend" --project="$project" \
  --gcs-bucket-name="$bucket" --no-enable-cdn \
  --description="get.dassie.land without Cloud CDN, for /flake/latest.tar.gz (scripts/flake-latest.ts)"
run gcloud iam roles create "$role" --project="$project" \
  --title="Route the mirror's flake" \
  --description="Points get.dassie.land/flake/latest.tar.gz at a release (scripts/flake-latest.ts)" \
  --permissions=compute.urlMaps.get,compute.urlMaps.update,compute.urlMaps.validate,compute.backendBuckets.get,compute.backendBuckets.use,compute.globalOperations.get \
  --stage=GA
run gcloud projects add-iam-policy-binding "$project" \
  --member="serviceAccount:$account" --role="projects/$project/roles/$role" \
  --condition=None
