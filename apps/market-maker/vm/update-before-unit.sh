#!/bin/sh
# Updates Dassie on the market maker's VM (spec/areas/market-maker-vm.md)
# while it runs a version from before `dassie update --unit`, which
# update.sh hands it to. It goes once the VM runs a release that has
# `--unit`.
#
# `dassie update` chooses, verifies and installs the release as on any
# install (spec/areas/release.md#dassie-update), but restarts nothing
# (--no-restart): the units it brings go to a scratch directory, since the
# VM runs units of its own, not dassie.service. Once it installed a
# version, this restarts both nodes and then the market maker. If a node
# does not come up, it points `current` back at the version it named
# before, records the rollback in last-update as the updater does, so
# that the next update passes over that version until the mirror offers
# another, and starts the old version again.
#
# A rollback does not put back the database from before a migration, as
# the updater does for dassie.service: a node that the new version
# migrated stays down on the old one, which the journal says.
set -eu

base=/opt/dassie
units="${STATE_DIRECTORY:?"run this from dassie-mm-update.service"}/units"
nodes="dassie-mm-node@xrp.service dassie-mm-node@dsusd.service"

mkdir -p "$units"
before=$(readlink "$base/current")
DASSIE_SYSTEMD_UNIT_DIR="$units" "$base/current/bin/dassie" update --no-restart
outcome=$(sed -n 's/^outcome=//p' "$base/last-update" 2> /dev/null || true)
after=$(readlink "$base/current")
if [ "$outcome" != updated ] || [ "$after" = "$before" ]; then
  exit 0
fi

# Restarts both nodes, which must report readiness in time, and then the
# market maker, which stops first so that it does not restart with each
# node. A market maker that does not start is no reason to go back:
# systemd starts it again, and its journal says why.
restart() {
  systemctl stop dassie-mm.service
  # shellcheck disable=SC2086 # the unit names are words
  systemctl restart $nodes || return 1
  systemctl start dassie-mm.service ||
    echo "Note: dassie-mm.service did not start; systemd tries again" >&2
}

echo "Restarting the nodes and the market maker on dassie $after"
if restart; then
  exit 0
fi
problem="a node did not start on dassie $after"
echo "Error: $problem; going back to dassie $before" >&2

# As install.sh and the updater switch it: one rename, under their lock.
exec 9< "$base/.update.lock"
flock -w 600 9 || {
  echo "Error: an update is running; dassie $after stays" >&2
  exit 1
}
ln -sfn "$before" "$base/.current-$$"
mv -T "$base/.current-$$" "$base/current"
sync

# The updater's record (crates/dassie-cli/src/update.rs, LastUpdate), so
# that `dassie update` and `dassie status` know this version did not
# start.
record="$base/.last-update-$$"
{
  echo "at=$(date +%s%3N)"
  echo "outcome=rolled-back"
  echo "target=$after"
  echo "problem=$problem"
  echo "rolled-back=$after"
  echo "rolled-back-problem=$problem"
} > "$record"
chmod 0644 "$record"
mv -f "$record" "$base/last-update"
sync
exec 9<&-

if ! restart; then
  echo "Error: dassie $before does not start again either; see journalctl -u 'dassie-mm-node@*'" >&2
fi
exit 1
