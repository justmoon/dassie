#!/bin/sh
# Sets up the DSUSD keeper's VM on GCP (spec/areas/dsusd-keeper.md#its-vm):
#
#   apps/dsusd-keeper/vm/provision.sh [--apply]
#
# Without --apply it prints the gcloud commands and runs nothing. With it,
# it runs them, as an owner of the project, once; startup.sh and the units
# it installs come from `main` at each boot and each run, so a change to
# them needs no provisioning again (one to startup.sh itself needs
# `gcloud compute instances add-metadata … --metadata-from-file`).
#
# What it makes:
# - a service account of the keeper's own, with no role in the project
#   (which also holds the bootstrap nodes' phrases): it can read the two
#   secrets with the issuer's and the keeper's seeds, and nothing else;
# - a network of its own with no firewall rule: nothing reaches the VM
#   (GCP denies ingress by default), and the VM reaches out to the testnet,
#   its faucet, the exchange and GitHub;
# - an e2-micro VM in a region where one is in the free tier, whose startup
#   script is startup.sh. It needs no SSH: each run's line goes to the
#   serial console as well as the journal, so
#   `gcloud compute instances get-serial-port-output dsusd-keeper` shows it.
#
# The two secrets exist before this runs, each a seed of an XRP Ledger
# account (spec/decisions/dsusd.md): the issuer's address is what nodes
# name DSUSD by, so its seed is made once and kept. To make them:
#
#   node -e 'import("xrpl").then(({ Wallet }) => console.log(Wallet.generate().seed))' |
#     gcloud secrets create dsusd-issuer-seed --project=dassie-390023 \
#       --replication-policy=automatic --data-file=-
#
# and the same for dsusd-keeper-seed.
set -eu

project=dassie-390023
# us-east1 is one of the regions with an e2-micro in the free tier.
region=us-east1
zone=us-east1-b
machine_type=e2-micro
account=dsusd-keeper
secrets="dsusd-issuer-seed dsusd-keeper-seed"
network=dsusd-keeper
instance=dsusd-keeper

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
  --display-name="DSUSD keeper (spec/decisions/dsusd.md)" \
  --description="Runs the DSUSD keeper's VM; reads only the issuer's and the keeper's seeds"

# The bindings are on the secrets, not the project: the account can read
# these two secrets' versions and nothing else.
for secret in $secrets; do
  run gcloud secrets add-iam-policy-binding "$secret" --project="$project" \
    --member="serviceAccount:$email" --role=roles/secretmanager.secretAccessor
done

run gcloud compute networks create "$network" --project="$project" \
  --subnet-mode=custom
run gcloud compute networks subnets create "$network" --project="$project" \
  --network="$network" --region="$region" --range=10.80.0.0/24

# The free tier's disk is 30 GB of standard persistent disk. The VM needs
# an external address to reach out: there is no NAT on its network.
run gcloud compute instances create "$instance" --project="$project" \
  --zone="$zone" \
  --machine-type="$machine_type" \
  --subnet="$network" \
  --service-account="$email" \
  --scopes=cloud-platform \
  --image-family=debian-13 \
  --image-project=debian-cloud \
  --boot-disk-size=30GB \
  --boot-disk-type=pd-standard \
  --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring \
  --metadata-from-file=startup-script="$here/startup.sh" \
  --metadata="block-project-ssh-keys=true"
