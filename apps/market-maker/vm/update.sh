#!/bin/sh
# Updates Dassie on the market maker's VM (spec/areas/market-maker-vm.md),
# as root, when dassie-mm-update.timer says so.
#
# `dassie update --unit` chooses, verifies and installs the release as on
# any install (spec/areas/release.md#dassie-update), installs none of the
# release's units, since the VM runs units of its own, and restarts both
# nodes in place of dassie.service; the market maker restarts with them.
# If a node does not come up, the updater goes back as it does for
# dassie.service: `current` back, each node's database from before a
# migration put back, the old version started again, and the rollback
# recorded, so that the next update passes over that version until the
# mirror offers another.
set -eu

base=/opt/dassie
units="--unit dassie-mm-node@xrp.service=/var/lib/dassie-mm/xrp
--unit dassie-mm-node@dsusd.service=/var/lib/dassie-mm/dsusd"

if ! "$base/current/bin/dassie" --help | grep -q -- "--unit <name>.service"; then
  # A version from before `dassie update --unit`: it updates as it always
  # has, once, to a release that has it.
  exec sh "$(dirname "$0")/update-before-unit.sh"
fi

status=0
# shellcheck disable=SC2086 # the options are words
"$base/current/bin/dassie" update $units || status=$?
# A rollback stops the nodes, and the market maker with them, and starts
# only the nodes again. A market maker that does not start is no reason to
# fail: systemd starts it again, and its journal says why.
if [ "$status" -ne 0 ]; then
  systemctl start dassie-mm.service ||
    echo "Note: dassie-mm.service did not start; systemd tries again" >&2
fi
exit "$status"
