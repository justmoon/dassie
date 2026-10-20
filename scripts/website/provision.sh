#!/bin/sh
# Sets up dassie.land on GCP (spec D35): a bucket behind the release
# mirror's load balancer, the account that publishes it, a certificate for
# dassie.land and its subdomains, and the dassie.land zone.
#
#   scripts/website/provision.sh [--apply]
#
# Without --apply it prints the gcloud commands and runs nothing. With it,
# it runs them, as an owner of the project, once. To change the headers
# after an edit of apps/website/headers.json, run the backend bucket's
# `update` by hand with the same --custom-response-header flags; the
# website workflow's check fails until it matches.
#
# What it makes:
# - the bucket, readable by anyone, as the load balancer needs, with
#   index.html as each directory's page;
# - a service account that may write the bucket's objects and nothing
#   else, for the website workflow in the `Website` environment alone,
#   through a provider of its own in the GitHub workload identity pool
#   (release.yml's admits only the `Release` environment);
# - a backend bucket with headers.json's headers and Cloud CDN, which
#   caches by each object's own Cache-Control (scripts/website/deploy.sh);
# - in the load balancer's URL map, `dassie.land` to that backend and a
#   redirect from `www.dassie.land` to it;
# - a Certificate Manager certificate for dassie.land and *.dassie.land,
#   by DNS authorization, so it is issued before any address moves, and a
#   certificate map with it as its primary, which also serves get. and
#   sh.dassie.land;
# - the zone dassie-land, with DNSSEC, naming the load balancer for the
#   apex, www, get and sh, and keeping the domain's verification record.
#   No wildcard: no other name serves anything.
#
# What it leaves for later, in order:
# 1. Add the DNS authorization's CNAME (printed at the end) to the zone
#    that is authoritative now, and wait until
#    `gcloud certificate-manager certificates describe dassie-land` is
#    ACTIVE.
# 2. Attach the map with the command printed at the end. It replaces the
#    proxy's certificate for get. and sh.dassie.land, which the new one
#    also covers.
# 3. Point the registrar at the zone's name servers, then publish its DS
#    record (`gcloud dns dns-keys list --zone=dassie-land`).
set -eu

project=dassie-390023
project_number=825262423039
# The mirror's load balancer: its address, URL map and HTTPS proxy.
address=34.149.170.186
url_map=get-dassie-land
proxy=get-dassie-land-target-proxy
# The GitHub workload identity pool release.yml authenticates through, and
# the GitHub repository and owner, by their IDs.
pool=github
provider=dassie-website
repository=justmoon/dassie
repository_id=497132328
owner_id=53233
environment=Website
bucket=dassie-website
location=US
account=website-deploy
domain=dassie.land
zone=dassie-land

apply=0
if [ "${1:-}" = "--apply" ]; then apply=1; fi

run() {
  if [ "$apply" = 1 ]; then "$@"; else printf '%s\n' "$*"; fi
}

here=$(dirname "$0")
headers=$here/../../apps/website/headers.json
email="$account@$project.iam.gserviceaccount.com"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

run gcloud services enable storage.googleapis.com compute.googleapis.com \
  certificatemanager.googleapis.com dns.googleapis.com --project="$project"

# The bucket.
run gcloud storage buckets create "gs://$bucket" --project="$project" \
  --location="$location" --uniform-bucket-level-access
run gcloud storage buckets update "gs://$bucket" \
  --web-main-page-suffix=index.html
run gcloud storage buckets add-iam-policy-binding "gs://$bucket" \
  --member=allUsers --role=roles/storage.objectViewer

# The account that publishes, held to the bucket's objects. Only a job in
# the `Website` environment, which admits only `main`, may act as it: the
# subject of its token names the environment.
run gcloud iam service-accounts create "$account" --project="$project" \
  --display-name="dassie.land publisher (D35)" \
  --description="Writes the dassie.land bucket's objects; nothing else"
run gcloud storage buckets add-iam-policy-binding "gs://$bucket" \
  --member="serviceAccount:$email" --role=roles/storage.objectUser
run gcloud iam workload-identity-pools providers create-oidc "$provider" \
  --project="$project" --location=global --workload-identity-pool="$pool" \
  --display-name="$repository website" \
  --issuer-uri=https://token.actions.githubusercontent.com \
  --attribute-mapping="google.subject=assertion.sub,attribute.repository_id=assertion.repository_id,attribute.environment=assertion.environment" \
  --attribute-condition="assertion.repository_id == '$repository_id' && assertion.repository_owner_id == '$owner_id' && assertion.environment == '$environment'"
run gcloud iam service-accounts add-iam-policy-binding "$email" \
  --project="$project" --role=roles/iam.workloadIdentityUser \
  --member="principal://iam.googleapis.com/projects/$project_number/locations/global/workloadIdentityPools/$pool/subject/repo:$repository:environment:$environment"

# The backend, with every header of headers.json.
set -- gcloud compute backend-buckets create "$bucket" --project="$project" \
  --gcs-bucket-name="$bucket" --enable-cdn --cache-mode=USE_ORIGIN_HEADERS \
  --compression-mode=AUTOMATIC
jq -r '.[] | "\(.key): \(.value)"' "$headers" > "$work/headers"
while IFS= read -r header; do
  set -- "$@" "--custom-response-header=$header"
done < "$work/headers"
run "$@"

# The URL map: dassie.land to the backend, www.dassie.land redirected to
# it, and each page's path without its slash redirected to the path with
# it (the bucket would send it to <page>/index.html instead). The pages
# are those of apps/website/src/layout.ts; check.sh fails a page without
# its rule. The map is changed as a whole, from what it is now, and the
# script's own entries replaced: run it again, or this part alone, after
# adding a page.
backend="https://www.googleapis.com/compute/v1/projects/$project/global/backendBuckets/$bucket"
grep -oE 'path: "/[^"]+/"' "$here/../../apps/website/src/layout.ts" \
  | sed -E 's/^path: "(.*)\/"$/\1/' | jq -R . | jq -s . > "$work/pages.json"
if [ "$apply" = 1 ]; then
  gcloud compute url-maps describe "$url_map" --project="$project" \
    --format=json > "$work/url-map.json"
else
  echo '{"hostRules": [], "pathMatchers": []}' > "$work/url-map.json"
fi
jq --arg domain "$domain" --arg backend "$backend" \
  --slurpfile pages "$work/pages.json" '
  ["dassie-land", "www-dassie-land"] as $ours
  | del(.id, .kind, .selfLink, .creationTimestamp)
  | .hostRules = [.hostRules[] | select(.pathMatcher | IN($ours[]) | not)]
  | .pathMatchers = [.pathMatchers[] | select(.name | IN($ours[]) | not)]
  | .hostRules += [
      {hosts: [$domain], pathMatcher: "dassie-land"},
      {hosts: ["www." + $domain], pathMatcher: "www-dassie-land"}
    ]
  | .pathMatchers += [
      {name: "dassie-land", defaultService: $backend,
        routeRules: [$pages[0] | to_entries[] | {
          priority: (.key + 1),
          matchRules: [{fullPathMatch: .value}],
          urlRedirect: {pathRedirect: (.value + "/"),
            redirectResponseCode: "PERMANENT_REDIRECT", stripQuery: false}}]},
      {name: "www-dassie-land", defaultUrlRedirect: {
        hostRedirect: $domain, httpsRedirect: true,
        redirectResponseCode: "PERMANENT_REDIRECT", stripQuery: false}}
    ]' "$work/url-map.json" > "$work/url-map-new.json"
if [ "$apply" = 0 ]; then
  echo "# the URL map's entries for dassie.land:"
  jq '{hostRules, pathMatchers}' "$work/url-map-new.json"
fi
run gcloud compute url-maps import "$url_map" --project="$project" \
  --global --source="$work/url-map-new.json" --quiet

# The certificate, and the map that serves it for every name.
run gcloud certificate-manager dns-authorizations create "$zone" \
  --project="$project" --domain="$domain"
run gcloud certificate-manager certificates create "$zone" \
  --project="$project" --domains="$domain,*.$domain" \
  --dns-authorizations="$zone"
run gcloud certificate-manager maps create "$zone" --project="$project"
run gcloud certificate-manager maps entries create primary \
  --project="$project" --map="$zone" --certificates="$zone" --set-primary

# The zone.
run gcloud dns managed-zones create "$zone" --project="$project" \
  --dns-name="$domain." --description="dassie.land (D35)" \
  --dnssec-state=on
for name in "" www. get. sh.; do
  run gcloud dns record-sets create "$name$domain." --project="$project" \
    --zone="$zone" --type=A --ttl=300 --rrdatas="$address"
done
run gcloud dns record-sets create "$domain." --project="$project" \
  --zone="$zone" --type=CAA --ttl=3600 \
  --rrdatas='0 issue "pki.goog"','0 issue "letsencrypt.org"'
# Google's proof that the project's owners own dassie.land, which a bucket
# named after the domain, as get.dassie.land is, needs.
run gcloud dns record-sets create "hsx2jervbt73.$domain." --project="$project" \
  --zone="$zone" --type=CNAME --ttl=3600 \
  --rrdatas=gv-m66stpcescgqqr.dv.googlehosted.com.
if [ "$apply" = 1 ]; then
  challenge=$(gcloud certificate-manager dns-authorizations describe "$zone" \
    --project="$project" \
    --format='value(dnsResourceRecord.name,dnsResourceRecord.data)')
  set -- $challenge
  gcloud dns record-sets create "$1" --project="$project" --zone="$zone" \
    --type=CNAME --ttl=300 --rrdatas="$2"
  echo "The DNS authorization's record, for the zone that is authoritative now:"
  echo "  $1 CNAME $2"
  echo "The zone's name servers, for the registrar:"
  gcloud dns managed-zones describe "$zone" --project="$project" \
    --format='value(nameServers)'
else
  echo "# then the DNS authorization's CNAME, from"
  echo "# gcloud certificate-manager dns-authorizations describe $zone"
fi
echo "# once the certificate is ACTIVE:"
echo "gcloud compute target-https-proxies update $proxy --project=$project --global --certificate-map=$zone"
