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
# What it makes: a role that may read, validate and update URL maps, use
# backend buckets in them and follow the update, and nothing else, for the
# release workflow's service account. Compute has no roles on a single URL
# map, so it holds for the project's, which serve get., sh. and dassie.land:
# what the release workflow can change there is no more than it can already
# do by replacing install.sh.
#
# The route itself, and the path matcher for get.dassie.land it lives in,
# the release workflow makes (scripts/flake-latest.ts). It needs the mirror's
# load balancer to be EXTERNAL_MANAGED, for the route's headers, and its
# backend for get.dassie.land to stay without Cloud CDN.
set -eu

project=dassie-390023
role=mirrorFlakeRoute
account=dassie-release@dassie-390023.iam.gserviceaccount.com

apply=0
if [ "${1:-}" = "--apply" ]; then apply=1; fi

run() {
  if [ "$apply" = 1 ]; then "$@"; else printf '%s\n' "$*"; fi
}

run gcloud iam roles create "$role" --project="$project" \
  --title="Route the mirror's flake" \
  --description="Points get.dassie.land/flake/latest.tar.gz at a release (scripts/flake-latest.ts)" \
  --permissions=compute.urlMaps.get,compute.urlMaps.update,compute.urlMaps.validate,compute.backendBuckets.use,compute.globalOperations.get \
  --stage=GA
run gcloud projects add-iam-policy-binding "$project" \
  --member="serviceAccount:$account" --role="projects/$project/roles/$role" \
  --condition=None
