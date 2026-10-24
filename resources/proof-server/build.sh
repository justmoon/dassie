#!/bin/sh
# Builds the proof servers' xrpld (README.md#building):
#
#   resources/proof-server/build.sh <version> <out-dir>
#
# It builds the release of XRPLF/rippled that patches/BASE names, by its tag
# and the commit the tag must point to, with the patches in patches/
# applied. <version> is what the build reports: the tag with `+proofs.<n>`
# after it, such as `3.5.0-rc1+proofs.1`.
#
# The build is rippled's own: the toolchain of its Nix development shell,
# pinned by its flake.lock, with the glibc that its Linux builds target, and
# its dependencies from Conan, pinned by its conan.lock. So anyone can build
# the same xrpld from the same commit and patches. Bit-for-bit identity is not
# the aim: a Dassie node checks every answer, so it never needs to trust the
# binary.
#
# It writes the stripped binary as `xrpld`, its `SHA256SUMS`, and
# `BUILDINFO` with the commit built and the hashes of the two lock files.
set -eu

if [ $# -ne 2 ]; then
  echo "usage: $0 <version> <out-dir>" >&2
  exit 2
fi
version=$1
out=$(realpath -m "$2")
patches=$(realpath "$(dirname "$0")/patches")
read -r tag commit < "$patches/BASE"
jobs=$(nproc)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git clone --quiet --filter=blob:none --branch "$tag" \
  https://github.com/XRPLF/rippled "$work/rippled"
cd "$work/rippled"
# A tag can be moved; the commit cannot.
if [ "$(git rev-parse "$tag^{commit}")" != "$commit" ]; then
  echo "tag $tag is not $commit, as patches/BASE says" >&2
  exit 1
fi
git -c user.name=build -c user.email=build@localhost \
  am --quiet --committer-date-is-author-date "$patches"/*.patch

# rippled's build points the binary at the system's loader once it is
# linked, with a script that needs /bin/bash, which NixOS lacks. So the
# build leaves that to the end, where patchelf does what the script would.
nix develop --command sh -eu -c "
  export XRPLD_NO_PATCH_NIX_BINARY=1
  mkdir .build
  cd .build
  conan install .. --output-folder . --build missing --settings build_type=Release
  FORCE_XRPLD_VERSION='$version' cmake -G Ninja \\
    -DCMAKE_TOOLCHAIN_FILE:FILEPATH=build/generators/conan_toolchain.cmake \\
    -DCMAKE_BUILD_TYPE=Release -Dxrpld=ON -Dtests=OFF -Dbenchmark=OFF ..
  cmake --build . --target xrpld --parallel $jobs
  strip -o xrpld.stripped xrpld
  patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --remove-rpath xrpld.stripped
"

mkdir -p "$out"
install -m 0755 .build/xrpld.stripped "$out/xrpld"
{
  echo "tag $tag $commit"
  echo "patched $(git rev-parse HEAD)"
  echo "version $version"
  echo "flake.lock $(sha256sum flake.lock | cut -d' ' -f1)"
  echo "conan.lock $(sha256sum conan.lock | cut -d' ' -f1)"
} > "$out/BUILDINFO"
cd "$out"
sha256sum xrpld BUILDINFO > SHA256SUMS
cat BUILDINFO SHA256SUMS
