# Running a front door

A front door lets ordinary Open Payments clients reach Dassie nodes that have no public address, at `https://<label>.<domain>` ([spec/areas/http.md](../../spec/areas/http.md#front-doors)). It is a mode of the `dassie` binary, `dassie front-door`, and no node: it has no ledger, no peer protocol and no wallet. The installer never sets one up; these files are for an operator who runs one, such as `dassiepay.net`.

## What it needs

- **A host with a public address,** and **UDP and TCP port 443 open** to it: nodes connect over QUIC on UDP, clients over HTTPS on TCP.
- **A wildcard DNS record,** `*.<domain>`, `A` (and `AAAA`, if the host has IPv6) to the host. It covers every node's name, and `relay.<domain>`, where nodes connect.
- **A certificate for `*.<domain>`.** A wildcard needs the DNS-01 challenge, for example with certbot and its plugin for the DNS provider (on Google Cloud DNS, `python3-certbot-dns-google`, with a service account that may edit that one zone, [below](#a-dns-credential-for-one-zone-alone)):

  ```sh
  certbot certonly --dns-google --dns-google-credentials /etc/letsencrypt/dns.json \
    -d '*.dassiepay.net' \
    --deploy-hook /etc/letsencrypt/renewal-hooks/deploy/dassie-front-door.sh
  ```

  `deploy-certificate.sh` is that hook: it copies the certificate where the front door reads it, readable by its user. The front door reads the files again every minute, so a renewal takes effect without a restart.
- **Its own key,** an Ed25519 key in `/var/lib/dassie-front-door/front-door.key`, as 32 bytes of seed or PKCS#8. The unit makes one on its first start (`dassie front-door --make-key`). Keep it: nodes name the front door by it, and a new key means every node must be told again. Back it up like any secret.

## A DNS credential for one zone alone

The front door's host faces the internet, so whoever takes it over holds the credential certbot uses. Give it the least that DNS-01 needs: changing records in the front door's own zone. On Google Cloud, where one project may hold other zones (for Dassie's own project, `dassie-390023`, also `dassie.land` with the installer's names, and `dassie.xyz` with the bootstrap nodes), never grant DNS rights at the project:

```sh
PROJECT=dassie-390023
ZONE=dassiepay-net
SA=dassiepay-certbot@$PROJECT.iam.gserviceaccount.com

# A service account for certbot alone.
gcloud iam service-accounts create dassiepay-certbot --project "$PROJECT" \
  --display-name "certbot for dassiepay.net"

# Record changes in this one managed zone only: roles/dns.admin bound to
# the zone, not the project (or a custom role with dns.changes.* and
# dns.resourceRecordSets.*).
gcloud dns managed-zones add-iam-policy-binding "$ZONE" --project "$PROJECT" \
  --member "serviceAccount:$SA" --role roles/dns.admin

# Reading at the project, which certbot-dns-google needs to list the zones
# and find this one (dns.managedZones.list). It reads what DNS publishes
# anyway, and changes nothing.
gcloud projects add-iam-policy-binding "$PROJECT" \
  --member "serviceAccount:$SA" --role roles/dns.reader

# Its key, for certbot only: root's, mode 0600.
gcloud iam service-accounts keys create /etc/letsencrypt/dns.json --iam-account "$SA"
chmod 0600 /etc/letsencrypt/dns.json
```

Run the VM without the default compute service account and its scopes (`gcloud compute instances create … --no-service-account --no-scopes`), so the metadata server hands out no other credential; certbot's key is then the only one on the host.

## Setting it up

1. Install `dassie` as `/usr/local/bin/dassie`, from a release bundle whose signature you checked (the installer's way, `install.sh`, also sets up a node, which a front door's host does not want).
2. Make its user: `useradd --system --no-create-home --shell /usr/sbin/nologin dassie-front-door`.
3. Get the certificate as above, and run the hook once: `RENEWED_LINEAGE=/etc/letsencrypt/live/dassiepay.net sh deploy-certificate.sh`.
4. Install `dassie-front-door.service` in `/etc/systemd/system/`, set `DASSIE_FRONT_DOOR_DOMAIN` in it if the domain is not `dassiepay.net`, and `systemctl enable --now dassie-front-door`.
5. Read the entry that names it: `journalctl -u dassie-front-door | grep DASSIE_RELAY`.

`dassie --help` lists the settings: each `DASSIE_FRONT_DOOR_*` variable has a flag.

## Naming it as a node's relay

A node without a public address keeps a connection to the front door by naming it as its relay ([spec/areas/node.md](../../spec/areas/node.md#a-chosen-relay)), with the entry the front door logs, in the node's environment:

```sh
systemctl edit dassie
# [Service]
# Environment='DASSIE_RELAY={"id":"d…","url":"https://relay.dassiepay.net","publicKey":"…"}'
```

The node then answers at `https://<label>.dassiepay.net`, its default name. Under another front door's domain, the node also lists that name: `DASSIE_PUBLIC_NAMES=<label>.<domain>` ([spec/areas/payments.md](../../spec/areas/payments.md#the-nodes-names)).
