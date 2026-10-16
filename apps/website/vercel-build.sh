#!/bin/sh
# Builds dassie.land on Vercel (see vercel.json), from apps/website.
#
# Vercel's build image has neither the Node and pnpm the repo pins nor Rust,
# so this installs them first: Node, pnpm, rustup with the toolchain
# rust-toolchain.toml names, and the wasm-bindgen CLI. Each download is
# pinned by version and SHA-256; the versions follow the Nix flake and
# Cargo.lock, which `build_test.ts` checks. The system packages it needs, a
# C compiler for build scripts among them, come from vercel.json's
# installCommand. Then it builds the WASM bindings, as `node --run
# build:wasm` always does, and the site into dist/.
#
# Locally, `node --run build:website` does the same with the flake's tools.
set -eu

NODE_VERSION=26.11.1
NODE_SHA256=15fc043a51a5783f2f4b634561091658f5a7f0e3db03f997dca3a8033a33e45b
PNPM_VERSION=11.27.0
PNPM_SHA256=14ed04e631e4563abf3570dda1a365316683149a482b741a2db37e495f6b9cdb
RUSTUP_VERSION=1.29.1
RUSTUP_SHA256=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71
WASM_BINDGEN_VERSION=0.2.129
WASM_BINDGEN_SHA256=82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e

if [ "$(uname -s)-$(uname -m)" != Linux-x86_64 ]; then
  echo "vercel-build.sh: only for x86-64 Linux, as Vercel builds on" >&2
  exit 1
fi

ROOT=$(cd ../.. && pwd)
TOOLS=$(mktemp -d)
trap 'rm -rf "$TOOLS"' EXIT
export CARGO_HOME="$TOOLS/cargo" RUSTUP_HOME="$TOOLS/rustup"
export PATH="$TOOLS/node/bin:$TOOLS/pnpm:$TOOLS/bin:$CARGO_HOME/bin:$PATH"
mkdir -p "$TOOLS/bin" "$TOOLS/node" "$TOOLS/pnpm"

# Downloads $1 to $3 and checks it against the SHA-256 $2.
fetch() {
  curl --proto =https --tlsv1.2 -fsSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c --quiet -
}

echo "Installing Node $NODE_VERSION"
fetch "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-x64.tar.gz" \
  "$NODE_SHA256" "$TOOLS/node.tar.gz"
tar -xzf "$TOOLS/node.tar.gz" -C "$TOOLS/node" --strip-components 1

# pnpm's standalone build: one binary with its own Node, and the dist/ it
# loads from next to it.
echo "Installing pnpm $PNPM_VERSION"
fetch "https://github.com/pnpm/pnpm/releases/download/v$PNPM_VERSION/pnpm-linux-x64.tar.gz" \
  "$PNPM_SHA256" "$TOOLS/pnpm.tar.gz"
tar -xzf "$TOOLS/pnpm.tar.gz" -C "$TOOLS/pnpm"

echo "Installing Rust (rustup $RUSTUP_VERSION)"
fetch "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/x86_64-unknown-linux-gnu/rustup-init" \
  "$RUSTUP_SHA256" "$TOOLS/rustup-init"
chmod +x "$TOOLS/rustup-init"
"$TOOLS/rustup-init" -y --quiet --no-modify-path --profile minimal --default-toolchain none
# The toolchain rust-toolchain.toml names, with the WASM target.
(cd "$ROOT" && rustup toolchain install && rustup target add wasm32-unknown-unknown)

echo "Installing wasm-bindgen $WASM_BINDGEN_VERSION"
fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$WASM_BINDGEN_VERSION/wasm-bindgen-$WASM_BINDGEN_VERSION-x86_64-unknown-linux-musl.tar.gz" \
  "$WASM_BINDGEN_SHA256" "$TOOLS/wasm-bindgen.tar.gz"
tar -xzf "$TOOLS/wasm-bindgen.tar.gz" -C "$TOOLS" --strip-components 1 \
  "wasm-bindgen-$WASM_BINDGEN_VERSION-x86_64-unknown-linux-musl/wasm-bindgen"
mv "$TOOLS/wasm-bindgen" "$TOOLS/bin/"

cd "$ROOT"
pnpm install --frozen-lockfile
node --run build:wasm
node --run build:website
