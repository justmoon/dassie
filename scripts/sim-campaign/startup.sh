#!/bin/sh
# The boot script of a simulation campaign VM
# (spec/decisions/random-seed-simulations.md), which GCE runs as root at every
# boot from the instance metadata (provision.sh puts it there). It is
# idempotent: a spot VM boots afresh after each preemption.
#
# It installs Git and Nix, makes the two users the sandboxes run as, clones
# the repository, writes the supervisor's configuration from the instance
# metadata, and starts dassie-sim-campaign.service, which runs the
# supervisor (scripts/sim-campaign.ts) loop after loop from `main`. The
# toolchain is the flake's, as in CI: the supervisor runs in its `campaign`
# dev shell.
#
# A change to this script reaches the VMs only through a new instance
# template; a change to the unit, at their next boot; a change to the
# supervisor, at their next loop.
set -eu
# GCE runs this without a home, which Nix's installer, Nix and Git need.
export HOME=/root

state=/var/lib/dassie-sim-campaign
repository=https://github.com/justmoon/dassie.git
# Nix's installer, pinned by its hash; it checks the tarball it fetches.
nix_version=2.34.8
nix_installer_sha256=96c10e102c88809dd9ec0bee89200c4a51eae4c9f6d8698c26b16788d131e078

metadata() {
  curl -fsS -H "Metadata-Flavor: Google" \
    "http://metadata.google.internal/computeMetadata/v1/instance/attributes/$1"
}

export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -qy --no-install-recommends ca-certificates curl git xz-utils

# The users the sandboxes run as: one for trusted commits, whose builds
# persist between loops, and one for pull requests, whose builds do not.
# Neither can log in, and neither can read what the other writes.
for user in sim-main sim-pr; do
  id -u "$user" > /dev/null 2>&1 ||
    useradd --system --user-group --no-create-home --home-dir /nonexistent \
      --shell /usr/sbin/nologin "$user"
done

if [ ! -x /nix/var/nix/profiles/default/bin/nix ]; then
  curl -fsSL -o /tmp/nix-install \
    "https://releases.nixos.org/nix/nix-$nix_version/install"
  echo "$nix_installer_sha256  /tmp/nix-install" | sha256sum -c -
  sh /tmp/nix-install --daemon --yes < /dev/null
  rm /tmp/nix-install
fi
# Flakes, and only root may talk to the daemon: a build the daemon runs
# has the network, and with it the metadata server, so a sandbox must not
# be able to ask for one (the sandboxes also cannot see its socket).
cat > /etc/nix/nix.conf <<'EOF'
build-users-group = nixbld
experimental-features = nix-command flakes
allowed-users = root
trusted-users = root
EOF
systemctl restart nix-daemon.service

mkdir -p "$state"
chmod 755 "$state"
# The fetching sandbox resolves names through public resolvers: the VM's
# own resolver is the metadata server, which it must not reach.
printf 'nameserver 8.8.8.8\nnameserver 1.1.1.1\n' > "$state/resolv.conf"
chmod 644 "$state/resolv.conf"

# The secret's name and the GitHub App, from the instance metadata. Neither
# is secret; the key itself never touches the disk.
umask 022
{
  echo "SIM_CAMPAIGN_DIR=$state"
  echo "SIM_CAMPAIGN_SECRET=$(metadata sim-campaign-secret)"
  echo "SIM_CAMPAIGN_APP_ID=$(metadata sim-campaign-app-id)"
} > /etc/dassie-sim-campaign.env

if [ ! -d "$state/repo/.git" ]; then
  rm -rf "$state/repo"
  git clone --quiet "$repository" "$state/repo"
fi
git -C "$state/repo" fetch --quiet origin main
git -C "$state/repo" checkout --quiet --force --detach origin/main

install -m 644 "$state/repo/scripts/sim-campaign/dassie-sim-campaign.service" \
  /etc/systemd/system/dassie-sim-campaign.service
systemctl daemon-reload
systemctl enable dassie-sim-campaign.service
systemctl restart dassie-sim-campaign.service
