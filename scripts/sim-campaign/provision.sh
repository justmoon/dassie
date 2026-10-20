#!/bin/sh
# Sets up the simulation campaign's VMs on GCP (spec D27):
#
#   scripts/sim-campaign/provision.sh [--apply]
#
# Without --apply it prints the gcloud commands and runs nothing. With it,
# it runs them, as an owner of the project, once; to change the template
# later, bump `template` and roll the group onto it.
#
# What it makes:
# - a service account of the campaign's own, with no role in the project:
#   the VMs run unreviewed code from pull requests in the project that
#   holds the bootstrap nodes' phrases. It can read one secret, the GitHub
#   App's private key, and nothing else;
# - that secret, empty: add the key with
#   `gcloud secrets versions add sim-campaign-github --data-file=<key.pem>`;
# - a network of its own with no firewall rule, so nothing reaches the VMs
#   and they reach nothing of the bootstrap nodes';
# - a spot instance template whose startup script is startup.sh, and a
#   regional managed instance group, which recreates a VM that spot took.
#
# The GitHub App needs read and write access to issues, pull requests and
# commit statuses on justmoon/dassie, and nothing else; its ID goes in
# app_id below. Without an App, a dedicated account's fine-grained token
# with the same access can be the secret instead, with app_id empty.
set -eu

project=dassie-390023
region=europe-west1
zones=europe-west1-b,europe-west1-c,europe-west1-d
# 56 vCPUs of AMD Milan, 112 GB: the cheapest many-core spot machine in the
# region (see the pull request that added this for the prices).
machine_type=c2d-highcpu-56
size=1
account=sim-campaign
secret=sim-campaign-github
app_id=
template=sim-campaign-1
network=sim-campaign

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
  --display-name="Simulation campaign (D27)" \
  --description="Runs the simulation campaign's VMs; reads only the GitHub secret"

run gcloud secrets create "$secret" --project="$project" \
  --replication-policy=automatic
# The binding is on the secret, not the project: the account can read this
# secret's versions and nothing else.
run gcloud secrets add-iam-policy-binding "$secret" --project="$project" \
  --member="serviceAccount:$email" --role=roles/secretmanager.secretAccessor

run gcloud compute networks create "$network" --project="$project" \
  --subnet-mode=auto

run gcloud compute instance-templates create "$template" --project="$project" \
  --machine-type="$machine_type" \
  --provisioning-model=SPOT \
  --instance-termination-action=DELETE \
  --network="$network" \
  --service-account="$email" \
  --scopes=cloud-platform \
  --image-family=debian-13 \
  --image-project=debian-cloud \
  --boot-disk-size=100GB \
  --boot-disk-type=pd-balanced \
  --shielded-secure-boot --shielded-vtpm --shielded-integrity-monitoring \
  --metadata-from-file=startup-script="$here/startup.sh" \
  --metadata="sim-campaign-secret=projects/$project/secrets/$secret/versions/latest,sim-campaign-app-id=$app_id,block-project-ssh-keys=true"

run gcloud compute instance-groups managed create "$account" --project="$project" \
  --region="$region" \
  --zones="$zones" \
  --target-distribution-shape=any \
  --template="$template" \
  --size="$size"
