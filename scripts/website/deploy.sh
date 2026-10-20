#!/bin/sh
# Publishes dassie.land: copies the built site into its bucket (spec D35),
# which the mirror's load balancer serves.
#
#   scripts/website/deploy.sh <dist> <bucket>
#
# <dist> is apps/website/dist after `node --run build:website`; gcloud must
# be signed in as an account that may write the bucket's objects. The
# headers every response gets are on the backend bucket (provision.sh); the
# cache headers are on each object, set here.
#
# A page never names an asset that is not there yet: the assets go up
# first, then the pages, and only then is anything deleted. The files are
# compared by checksum, not time, since each build writes them anew, so an
# unchanged file keeps the metadata it was uploaded with.
set -eu

if [ $# -ne 2 ]; then
  echo "usage: $0 <dist> <bucket>" >&2
  exit 2
fi
dist=$1
remote=gs://$2

if [ ! -f "$dist/index.html" ] || [ ! -d "$dist/assets" ]; then
  echo "$dist is not a built site: no index.html or assets/" >&2
  exit 1
fi

# The assets' names hold a hash of their content: cached for good.
gcloud storage rsync --recursive --checksums-only \
  --cache-control="public, max-age=31536000, immutable" \
  "$dist/assets" "$remote/assets"
# WebAssembly compiles while it streams only as application/wasm, which
# not every MIME table knows.
gcloud storage objects update "$remote/assets/*.wasm" \
  --content-type=application/wasm

# The pages, and anything else outside assets/, revalidated on every load.
gcloud storage rsync --recursive --checksums-only \
  --cache-control="no-cache" \
  --exclude="^assets/" \
  "$dist" "$remote"

# What the build no longer has: older pages and assets of earlier builds.
gcloud storage rsync --recursive --checksums-only \
  --delete-unmatched-destination-objects \
  "$dist" "$remote"
