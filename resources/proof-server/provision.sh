#!/bin/sh
# Sets up the two proof servers on GCP (README.md):
#
#   resources/proof-server/provision.sh <xrpld-url> <xrpld-sha256> [--apply]
#
# Without --apply it prints the gcloud commands and runs nothing. With it,
# it runs them, as an owner of the project, once. startup.sh and the files
# it installs come from `main` at each boot, so a change to them needs only
# a restart of the VM (one to startup.sh itself needs
# `gcloud compute instances add-metadata … --metadata-from-file`).
#
# What it makes:
# - a service account of the servers' own, with no role and no secret: a
#   proof server holds nothing to steal, since every answer proves itself;
# - a network of their own, open to TCP 80 and 443 only: Caddy answers
#   clients and Let's Encrypt there, and xrpld reaches out to its peers;
# - a static address and a VM per network, whose startup script is
#   startup.sh. Neither needs SSH: startup.sh logs to the serial console;
# - the Cloud DNS zone dassieland.net, with DNSSEC, and a record per
#   server. The domain's registrar must delegate to the zone's name servers
#   and publish its DS record (`gcloud dns dns-keys list --zone
#   dassieland-net`) before Caddy can get a certificate.
#
# - a bucket, readable by anyone, for the builds of build.sh, which the VMs
#   install from: its URL and SHA-256 are the arguments, and startup.sh
#   refuses a build whose hash differs.
set -eu

project=dassie-390023
region=us-east1
zone=us-east1-b
account=proof-server
network=proof-server
bucket=dassie-xrpld
dns_zone=dassieland-net
domain=dassieland.net

if [ $# -lt 2 ]; then
  echo "usage: $0 <xrpld-url> <xrpld-sha256> [--apply]" >&2
  exit 2
fi
xrpld_url=$1
xrpld_sha256=$2
apply=0
if [ "${3:-}" = "--apply" ]; then apply=1; fi
# A branch to take the configuration from instead of `main` (startup.sh),
# for servers set up before this directory merges.
config_ref=${CONFIG_REF:+,config-ref=$CONFIG_REF}

run() {
  if [ "$apply" = 1 ]; then "$@"; else printf '%s\n' "$*"; fi
}

here=$(dirname "$0")
email="$account@$project.iam.gserviceaccount.com"

run gcloud services enable compute.googleapis.com dns.googleapis.com \
  --project="$project"

run gcloud iam service-accounts create "$account" --project="$project" \
  --display-name="Proof servers (resources/proof-server)" \
  --description="Runs the XRP Ledger proof servers' VMs; has no role"

run gcloud compute networks create "$network" --project="$project" \
  --subnet-mode=custom
run gcloud compute networks subnets create "$network" --project="$project" \
  --network="$network" --region="$region" --range=10.82.0.0/24
run gcloud compute firewall-rules create proof-server-allow-https \
  --project="$project" --network="$network" --direction=INGRESS \
  --allow=tcp:80,tcp:443 --source-ranges=0.0.0.0/0

# The bucket may be made first, so that the build is in it before a VM
# boots.
if [ "$apply" = 1 ] &&
  gcloud storage buckets describe "gs://$bucket" --project="$project" > /dev/null 2>&1; then
  echo "bucket $bucket exists"
else
  run gcloud storage buckets create "gs://$bucket" --project="$project" \
    --location="$region" --uniform-bucket-level-access
  run gcloud storage buckets add-iam-policy-binding "gs://$bucket" \
    --member=allUsers --role=roles/storage.objectViewer
fi

# The zone was made first, so that the domain could be delegated to it.
if [ "$apply" = 1 ] &&
  gcloud dns managed-zones describe "$dns_zone" --project="$project" > /dev/null 2>&1; then
  echo "zone $dns_zone exists"
else
  run gcloud dns managed-zones create "$dns_zone" --project="$project" \
    --dns-name="$domain." --dnssec-state=on \
    --description="Dassie's XRP Ledger proof servers"
fi

# Both on SSD: xrpld writes small blocks, thousands a second while it
# syncs, and a balanced disk's IOPS cap held the testnet's first sync back
# for hours. The testnet has less traffic but a larger state than mainnet.
for server in mainnet:e2-standard-8:250GB:pd-ssd testnet:e2-standard-4:150GB:pd-ssd; do
  IFS=: read -r name machine_type disk_size disk_type <<EOF
$server
EOF
  instance="xrpl-proofs-$name"
  host="$name.xrpl.$domain"

  run gcloud compute addresses create "$instance" --project="$project" \
    --region="$region"
  if [ "$apply" = 1 ]; then
    address=$(gcloud compute addresses describe "$instance" \
      --project="$project" --region="$region" --format='value(address)')
  else
    address='<address>'
  fi

  run gcloud compute instances create "$instance" --project="$project" \
    --zone="$zone" \
    --machine-type="$machine_type" \
    --subnet="$network" \
    --address="$address" \
    --service-account="$email" \
    --scopes=logging-write \
    --image-family=debian-13 \
    --image-project=debian-cloud \
    --boot-disk-size="$disk_size" \
    --boot-disk-type="$disk_type" \
    --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring \
    --metadata-from-file=startup-script="$here/startup.sh" \
    --metadata="block-project-ssh-keys=true,proof-network=$name,proof-host=$host,xrpld-url=$xrpld_url,xrpld-sha256=$xrpld_sha256$config_ref"

  run gcloud dns record-sets create "$host." --project="$project" \
    --zone="$dns_zone" --type=A --ttl=300 --rrdatas="$address"
done
