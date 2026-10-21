#!/bin/sh
# Checks that dassie.land is served as the site expects
# (spec/decisions/website-hosting.md):
#
#   scripts/website/check.sh [<url>] [<curl option>...]
#
# <url> defaults to https://dassie.land/. Options after it go to every
# curl, as `--resolve dassie.land:443:<address>` to ask the load balancer
# before DNS names it.
#
# - the home page and an asset carry every header of
#   apps/website/headers.json, which the backend bucket adds: a bucket
#   provisioned with other headers fails here, after the deploy;
# - pages are revalidated, and assets cached for good;
# - every page the header links to is there, and its path without its
#   slash redirects to the path with it, with a 308 from the load
#   balancer's rule for it, not the bucket's own redirect to index.html;
# - the STREAM toy's WebAssembly is served as application/wasm.
set -eu

url=${1:-https://dassie.land/}
if [ $# -gt 0 ]; then shift; fi
url=${url%/}
here=$(dirname "$0")
headers=$here/../../apps/website/headers.json
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failed=0

fail() {
  echo "check: $*" >&2
  failed=1
}

# Fetches path $1, with the curl options after it, into $work/body, its
# response headers into $work/head.
fetch() {
  path=$1
  shift
  curl --proto =https --tlsv1.2 -sS --retry 3 -o "$work/body" -D "$work/head" \
    "$@" "$url$path"
}

# The value of response header $1 in $work/head, or nothing.
header() {
  tr -d '\r' < "$work/head" | awk -v name="$1" '
    BEGIN { name = tolower(name) }
    index($0, ":") { key = tolower(substr($0, 1, index($0, ":") - 1)) }
    key == name { sub(/^[^:]*:[ \t]*/, ""); print; exit }'
}

status() {
  tr -d '\r' < "$work/head" | awk 'NR == 1 { print $2 }'
}

# Every header of headers.json, as in $work/head, for $1.
expect_headers() {
  jq -r '.[] | "\(.key)\t\(.value)"' "$headers" > "$work/expected"
  while IFS="$(printf '\t')" read -r key value; do
    actual=$(header "$key")
    [ "$actual" = "$value" ] || fail "$1: $key is '$actual', not '$value'"
  done < "$work/expected"
}

fetch / "$@"
[ "$(status)" = 200 ] || fail "/: status $(status)"
expect_headers /
case $(header cache-control) in
  *no-cache* | *max-age=0*) ;;
  *) fail "/: Cache-Control '$(header cache-control)' does not revalidate" ;;
esac

# Every page the header links to: its path without the slash redirects to
# the path with it, and the page is there.
pages=$(grep -oE 'href="/[a-z0-9/-]+/"' "$work/body" | sed -E 's/^href="(.*)"$/\1/' | sort -u)
[ -n "$pages" ] || fail "/: links to no page"
for page in $pages; do
  bare=${page%/}
  fetch "$bare" "$@"
  [ "$(status)" = 308 ] || fail "$bare: status $(status), not a 308"
  case $(header location) in
    "$page" | https://dassie.land"$page" | https://dassie.land:443"$page") ;;
    *) fail "$bare: redirects to '$(header location)'" ;;
  esac
  fetch "$page" "$@"
  [ "$(status)" = 200 ] || fail "$page: status $(status)"
done

fetch /tools/stream-toy/ "$@"
[ "$(status)" = 200 ] || fail "/tools/stream-toy/: status $(status)"
scripts=$(grep -oE '/assets/[A-Za-z0-9_.-]+\.js' "$work/body" | sort -u)
[ -n "$scripts" ] || fail "/tools/stream-toy/: loads no script"
wasm=
for script in $scripts; do
  fetch "$script" "$@"
  [ "$(status)" = 200 ] || fail "$script: status $(status)"
  [ -n "$wasm" ] || wasm=$(grep -oE '[A-Za-z0-9_-]+\.wasm' "$work/body" | head -n 1 || true)
done
first=$(printf '%s\n' "$scripts" | head -n 1)
if [ -n "$first" ]; then
  fetch "$first" "$@"
  expect_headers "$first"
  case $(header cache-control) in
    *immutable*) ;;
    *) fail "$first: Cache-Control '$(header cache-control)' is not immutable" ;;
  esac
fi

if [ -z "$scripts" ]; then
  :
elif [ -z "$wasm" ]; then
  fail "/tools/stream-toy/: its scripts name no .wasm"
else
  fetch "/assets/$wasm" "$@"
  [ "$(status)" = 200 ] || fail "/assets/$wasm: status $(status)"
  case $(header content-type) in
    application/wasm*) ;;
    *) fail "/assets/$wasm: Content-Type '$(header content-type)'" ;;
  esac
fi

[ "$failed" = 0 ] || exit 1
echo "check: $url is served as expected"
