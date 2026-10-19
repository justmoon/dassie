#!/bin/sh
# Vercel's Ignored Build Step (vercel.json's ignoreCommand): exits 0, which
# skips the deployment, only when nothing the site is built from changed
# since the commit Vercel last deployed on this branch. Anything else
# builds:
# - a first deployment on a branch;
# - a last deployed commit this clone lacks, as after a force push or
#   beyond the clone's depth;
# - a change to any path below.
#
# The paths are the site, the packages it imports, the crates its WASM
# modules are built from, and what pins their builds. build_test.ts checks
# that the list keeps up with them. It reads the workspace's dependencies,
# which include dassie-node-core through dassie-stream's `sim` feature: the
# WASM build leaves that off, but watching it costs only a spare build.
set -u

previous=${VERCEL_GIT_PREVIOUS_SHA:-}
[ -n "$previous" ] || exit 1
cd ../.. || exit 1
git cat-file -e "$previous^{commit}" 2> /dev/null || exit 1
exec git diff --quiet "$previous" HEAD -- \
  apps/website \
  packages/identity-wasm \
  packages/rpc \
  packages/ui \
  crates/dassie-identity \
  crates/dassie-ilp \
  crates/dassie-node-core \
  crates/dassie-oer \
  crates/dassie-stream \
  Cargo.toml \
  Cargo.lock \
  rust-toolchain.toml \
  package.json \
  pnpm-workspace.yaml \
  pnpm-lock.yaml \
  .npmrc \
  scripts/build-wasm.ts
