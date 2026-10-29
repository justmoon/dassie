#!/bin/sh
# Sets up the market maker's VM on GCP (spec/areas/market-maker-vm.md):
#
#   apps/market-maker/vm/provision.sh [--apply]
#
# Without --apply it prints the gcloud commands and runs nothing. With it,
# it runs them, as an owner of the project, once; startup.sh, the units and
# the scripts come from `main` at each boot, so a change to them needs no
# provisioning again (one to startup.sh itself needs
# `gcloud compute instances add-metadata … --metadata-from-file`).
#
# What it makes:
# - a service account of the market maker's own, with no role in the
#   project (which also holds the bootstrap nodes' phrases): it can read
#   the secret with its node's passphrase, and nothing else;
# - a network of its own with no firewall rule: nothing reaches the VM
#   (GCP denies ingress by default), and the VM reaches out to the targets,
#   the XRP Ledger testnet and its faucet, the mirror and GitHub;
# - an e2-small VM whose startup script is startup.sh. It needs no SSH:
#   the node and the market maker log to the serial console as well as
#   the journal, so
#   `gcloud compute instances get-serial-port-output dassie-mm --zone=us-east1-b`
#   shows them.
#
# The settings are instance metadata, each with a default in startup.sh:
# mm-targets, the core nodes to trade through, URLs separated by commas
# (the three bootstrap nodes by default), whose node IDs are read from
# their /.well-known/dassie/node and which the node pins as its uplinks;
# mm-curve-ppm, mm-margin-ppm, mm-threshold-ppm and mm-slippage-ppm, in
# parts per million; and mm-xrp-cap and mm-dsusd-cap, the most one trade
# pays, in the node's ledger units. To change one: `gcloud compute
# instances add-metadata dassie-mm --zone=us-east1-b
# --metadata=mm-margin-ppm=2000`, then reset the VM. A value
# with commas needs another separator between settings (`gcloud topic
# escaping`): `--metadata=^;^mm-targets=https://boot1.dassie.xyz,https://boot2.dassie.xyz`.
#
# The secret exists before this runs: a passphrase of the test realm of
# the market maker's node, which nobody else uses: whoever reads it holds
# the node's funds. To make it, generate a phrase as the wallet's setup
# does (`generatePhrase("test", Date.now()).text` of @dassie/wasm) and
#
#   … | gcloud secrets create mm-phrase --project=dassie-390023 \
#     --replication-policy=automatic --data-file=-
set -eu

project=dassie-390023
region=us-east1
zone=us-east1-b
machine_type=e2-small
account=dassie-mm
secret=mm-phrase
network=dassie-mm
instance=dassie-mm

apply=0
if [ "${1:-}" = "--apply" ]; then apply=1; fi

run() {
  if [ "$apply" = 1 ]; then "$@"; else printf '%s\n' "$*"; fi
}

here=$(dirname "$0")
email="$account@$project.iam.gserviceaccount.com"

run gcloud services enable compute.googleapis.com secretmanager.googleapis.com \
  --project="$project"

run gcloud iam service-accounts create "$account" --project="$project" \
  --display-name="Market maker (spec/areas/market-maker-vm.md)" \
  --description="Runs the market maker's VM; reads only its node's passphrase"

# The binding is on the secret, not the project: the account can read this
# secret's versions and nothing else.
run gcloud secrets add-iam-policy-binding "$secret" --project="$project" \
  --member="serviceAccount:$email" --role=roles/secretmanager.secretAccessor

run gcloud compute networks create "$network" --project="$project" \
  --subnet-mode=custom
run gcloud compute networks subnets create "$network" --project="$project" \
  --network="$network" --region="$region" --range=10.81.0.0/24

# The VM needs an external address to reach out: there is no NAT on its
# network.
run gcloud compute instances create "$instance" --project="$project" \
  --zone="$zone" \
  --machine-type="$machine_type" \
  --subnet="$network" \
  --service-account="$email" \
  --scopes=cloud-platform \
  --image-family=debian-13 \
  --image-project=debian-cloud \
  --boot-disk-size=20GB \
  --boot-disk-type=pd-balanced \
  --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring \
  --metadata-from-file=startup-script="$here/startup.sh" \
  --metadata="block-project-ssh-keys=true"
