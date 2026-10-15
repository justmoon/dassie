#!/bin/sh
# Builds dassie.land on Vercel (see vercel.json), from apps/website.
#
# Vercel's build image has neither Deno nor Rust, so this installs them
# first: Deno, rustup with the toolchain rust-toolchain.toml names, and the
# wasm-bindgen CLI. Each download is pinned by version and SHA-256; the
# versions follow the Nix flake and Cargo.lock, which `build_test.ts`
# checks. The system packages it needs, a C compiler for build scripts
# among them, come from vercel.json's installCommand. Then it builds the
# WASM bindings, as `deno task build:wasm` always does, and the site into
# dist/.
#
# Locally, `deno task build:website` does the same with the flake's tools.
set -eu

DENO_VERSION=2.9.7
DENO_SHA256=c6527f24f4b16031d3ae4fa9f658d5f11534c8d84ce7dc8502420280919c3490
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
export PATH="$TOOLS/bin:$CARGO_HOME/bin:$PATH"
mkdir -p "$TOOLS/bin"

# Downloads $1 to $3 and checks it against the SHA-256 $2.
fetch() {
  curl --proto =https --tlsv1.2 -fsSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c --quiet -
}

echo "Installing Deno $DENO_VERSION"
fetch "https://github.com/denoland/deno/releases/download/v$DENO_VERSION/deno-x86_64-unknown-linux-gnu.zip" \
  "$DENO_SHA256" "$TOOLS/deno.zip"
unzip -q "$TOOLS/deno.zip" -d "$TOOLS/bin"
chmod +x "$TOOLS/bin/deno"

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
deno install --frozen
deno task build:wasm
deno task build:website
