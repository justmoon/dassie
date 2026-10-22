#!/bin/sh
# The boot script of the DSUSD keeper's VM
# (spec/areas/dsusd-keeper.md#its-vm), which GCE runs as root at every boot
# from the instance metadata (provision.sh puts it there). It is
# idempotent.
#
# It installs Git, jq and Nix as the simulation campaign's VMs do
# (scripts/sim-campaign/startup.sh), clones the repository, and installs
# and starts the keeper's timer, which runs the keeper (vm/run.sh) every
# few minutes from a fresh checkout of `main`. The toolchain is Node and
# pnpm of the flake's nixpkgs, as CI's lockfile check takes them.
set -eu
# GCE runs this without a home, which Nix's installer, Nix and Git need.
export HOME=/root

state=/var/lib/dsusd-keeper
repository=https://github.com/justmoon/dassie.git
# Nix's installer, pinned by its hash; it checks the tarball it fetches.
nix_version=2.34.8
nix_installer_sha256=96c10e102c88809dd9ec0bee89200c4a51eae4c9f6d8698c26b16788d131e078

export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -qy --no-install-recommends ca-certificates curl git jq xz-utils

# An e2-micro has 1 GB of memory, which evaluating nixpkgs can outgrow.
if [ ! -f /swapfile ]; then
  fallocate -l 2G /swapfile
  chmod 600 /swapfile
  mkswap /swapfile
fi
swapon /swapfile 2> /dev/null || true

if [ ! -x /nix/var/nix/profiles/default/bin/nix ]; then
  curl -fsSL -o /tmp/nix-install \
    "https://releases.nixos.org/nix/nix-$nix_version/install"
  echo "$nix_installer_sha256  /tmp/nix-install" | sha256sum -c -
  sh /tmp/nix-install --daemon --yes < /dev/null
  rm /tmp/nix-install
fi
# Each change of the flake's nixpkgs brings a new Node and pnpm, and
# nothing roots the old ones: Nix collects them whenever the disk runs
# low, so the VM never fills its disk.
cat > /etc/nix/nix.conf <<'EOF'
build-users-group = nixbld
experimental-features = nix-command flakes
allowed-users = root
trusted-users = root
min-free = 2147483648
max-free = 6442450944
EOF
systemctl restart nix-daemon.service

mkdir -p "$state"
chmod 700 "$state"
if [ ! -d "$state/repo/.git" ]; then
  rm -rf "$state/repo"
  git clone --quiet "$repository" "$state/repo"
fi
git -C "$state/repo" fetch --quiet origin main
git -C "$state/repo" checkout --quiet --force --detach origin/main

for unit in dassie-dsusd-keeper.service dassie-dsusd-keeper.timer; do
  install -m 644 "$state/repo/apps/dsusd-keeper/vm/$unit" "/etc/systemd/system/$unit"
done
systemctl daemon-reload
systemctl enable dassie-dsusd-keeper.timer
systemctl restart dassie-dsusd-keeper.timer
