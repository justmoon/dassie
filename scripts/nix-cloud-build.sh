#!/usr/bin/env bash
# Builds a commit's flake attributes on the Vorstand Nix cloud
# (builds.vorstand.app, project dassie), for CI and the release workflow.
#
#   scripts/nix-cloud-build.sh build <commit> <attr>...
#   scripts/nix-cloud-build.sh cancel
#
# `build` follows the build's log and exits 0 only once it succeeded,
# printing the outputs' store paths last. `cancel` stops the build that
# `build` started on this runner, for a workflow run that was cancelled.
# Both read the project's token from NIX_CLOUD_TOKEN. With NIX_CLOUD_PIN=true,
# `build` asks the cloud to pin the build: its cache keeps the outputs, with
# all they need, until the build is unpinned, rather than while it has room.
# Pins count against the project's storage; when that is full, the cloud
# refuses a pinned build (409). To unpin one, `DELETE /v1/builds/<id>/pin`
# with the token, or "Unpin" on the project's page in Vorstand Code.
#
# The Nix cloud answers 429 while it holds as many builds as it takes:
# `build` then asks again, waiting longer each time (or as long as it says),
# for at most `submit_wait` seconds, so a busy cloud delays a check rather
# than fails it.
#
# The log stream can break off before the build ends. Its chunks never
# change, so `build` follows it again from the start and skips the bytes it
# has shown, a bounded number of times. The build's status answers 200 only
# once it succeeded (422 once it failed, 409 while it runs), so a log cut
# short never passes for a green build.
set -euo pipefail

api=https://nix.vorstand.app/v1/builds
follow_attempts=20
submit_wait=$((40 * 60))
job_file="${RUNNER_TEMP:-/tmp}/nix-cloud-job"

usage() {
  echo "usage: $0 build <commit> <attr>... | $0 cancel" >&2
  exit 2
}

if [ -z "${NIX_CLOUD_TOKEN:-}" ]; then
  echo "::error::No NIX_CLOUD_TOKEN: the Nix cloud builds nothing without it" >&2
  exit 1
fi
auth="Authorization: Bearer $NIX_CLOUD_TOKEN"

build() {
  [ $# -ge 2 ] || usage
  local commit=$1
  shift
  local body job
  body=$(jq -n --arg c "$commit" --argjson pin "${NIX_CLOUD_PIN:-false}" \
    '{commit: $c, attrs: $ARGS.positional} + (if $pin then {pin: true} else {} end)' --args "$@")
  job=$(submit "$body")
  if [ -z "$job" ] || [ "$job" = null ]; then
    echo "::error::The Nix cloud started no build" >&2
    exit 1
  fi
  printf '%s\n' "$job" > "$job_file"

  local log="${RUNNER_TEMP:-/tmp}/nix-cloud-$job.log"
  : > "$log"
  local attempt
  for ((attempt = 1; attempt <= follow_attempts; attempt++)); do
    if curl -fsSN -H "$auth" "$api/$job/log" | tail -c "+$(($(stat -c %s "$log") + 1))" | tee -a "$log"; then
      break
    fi
    # The warning on a line of its own, where the stream broke off mid-line.
    if [ -n "$(tail -c 1 "$log")" ]; then echo; fi
    echo "::warning::The Nix cloud's log stream broke off; following it again ($attempt of $follow_attempts)"
    sleep 5
  done

  curl -fsS -H "$auth" "$api/$job" | jq -r '.outputs[][]'
}

# Starts a build and prints its ID, asking again while the cloud is full.
submit() {
  local body=$1 started=$SECONDS delay=30 response headers status wait
  response=$(mktemp)
  headers=$(mktemp)
  while :; do
    status=$(curl -sS -o "$response" -D "$headers" -w '%{http_code}' \
      -H "$auth" --json "$body" "$api") || status=000
    if [ "$status" != 429 ]; then
      break
    fi
    wait=$(awk 'tolower($1) == "retry-after:" { print $2 + 0 }' "$headers" | tail -n 1)
    [ -n "$wait" ] && [ "$wait" -gt 0 ] || wait=$delay
    if [ $((SECONDS - started + wait)) -gt "$submit_wait" ]; then
      echo "::error::The Nix cloud was full for $((submit_wait / 60)) minutes" >&2
      exit 1
    fi
    echo "::notice::The Nix cloud is full; asking again in ${wait}s" >&2
    sleep "$wait"
    delay=$((delay * 2 > 120 ? 120 : delay * 2))
  done
  if [ "${status:0:1}" != 2 ]; then
    echo "::error::The Nix cloud refused the build ($status): $(cat "$response")" >&2
    exit 1
  fi
  jq -r .id "$response"
}

cancel() {
  [ $# -eq 0 ] || usage
  if [ ! -f "$job_file" ]; then
    echo "No build was started on this runner: nothing to cancel"
    return
  fi
  local job
  job=$(cat "$job_file")
  curl -fsS -H "$auth" -X POST "$api/$job/cancel" | jq -r '"Build \(.id): \(.status)"'
}

[ $# -ge 1 ] || usage
command=$1
shift
case "$command" in
  build) build "$@" ;;
  cancel) cancel "$@" ;;
  *) usage ;;
esac
