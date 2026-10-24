#!/bin/sh
# The boot script of a proof server's VM (README.md), which GCE runs as root
# at every boot from the instance metadata (provision.sh puts it there). It
# is idempotent.
#
# It reads from the instance metadata which network the server follows, its
# host name, and the xrpld build to run, by URL and SHA-256. It installs
# that build after checking its hash, the configuration for the network from
# this directory on `main` (or the branch the metadata `config-ref` names),
# and Caddy in front of it. So a new build is new metadata and a reset
# (README.md#updating), and a changed configuration needs only a reset.
#
# A testnet reset leaves xrpld with a chain the network no longer has. To
# start it on an empty database, set the metadata `wipe-db` to a value it
# has not had before and reset the VM: the database is wiped once per value.
set -eu
export HOME=/root

state=/var/lib/xrpld

metadata() {
  curl -fsS -H 'Metadata-Flavor: Google' \
    "http://metadata.google.internal/computeMetadata/v1/instance/attributes/$1"
}

network=$(metadata proof-network)
host=$(metadata proof-host)
xrpld_url=$(metadata xrpld-url)
xrpld_sha256=$(metadata xrpld-sha256)
wipe_db=$(metadata wipe-db 2> /dev/null || true)
# The branch whose configuration it installs: `main`, unless the metadata
# names another, to try a change on a server before it merges.
ref=$(metadata config-ref 2> /dev/null || echo main)
raw="https://raw.githubusercontent.com/justmoon/dassie/$ref/resources/proof-server"
case "$network" in
  mainnet | testnet) ;;
  *)
    echo "proof-server: unknown network '$network'" > /dev/console
    exit 1
    ;;
esac

export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -qy --no-install-recommends ca-certificates curl caddy

# xrpld's user and directories, as its packages make them.
if ! id xrpld > /dev/null 2>&1; then
  useradd --system --home-dir "$state" --shell /usr/sbin/nologin xrpld
fi
install -d -m 0750 -o xrpld -g xrpld "$state" /var/log/xrpld
install -d -m 0755 /etc/xrpld

# The build, only if its hash is the one the metadata names.
if ! echo "$xrpld_sha256  /usr/local/bin/xrpld" | sha256sum -c --status 2> /dev/null; then
  curl -fsSL -o /tmp/xrpld "$xrpld_url"
  echo "$xrpld_sha256  /tmp/xrpld" | sha256sum -c -
  install -m 0755 /tmp/xrpld /usr/local/bin/xrpld
  rm /tmp/xrpld
fi

fetch() {
  curl -fsSL -o "$2" "$raw/$1"
}
fetch "xrpld-$network.cfg" /etc/xrpld/xrpld.cfg
fetch "validators-$network.txt" /etc/xrpld/validators.txt
fetch xrpld.service /etc/systemd/system/xrpld.service
fetch Caddyfile /tmp/Caddyfile
sed "s/@HOST@/$host/" /tmp/Caddyfile > /etc/caddy/Caddyfile
rm /tmp/Caddyfile

if [ -n "$wipe_db" ] && [ "$(cat "$state/wiped" 2> /dev/null || true)" != "$wipe_db" ]; then
  systemctl stop xrpld.service 2> /dev/null || true
  rm -rf "$state/db"
  echo "$wipe_db" > "$state/wiped"
  echo "proof-server: wiped the database ($wipe_db)" > /dev/console
fi

systemctl daemon-reload
systemctl enable xrpld.service caddy.service
systemctl restart xrpld.service
systemctl reload-or-restart caddy.service
echo "proof-server: $network at $host runs $(/usr/local/bin/xrpld --version | head -1)" > /dev/console
