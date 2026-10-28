# Running a front door

A front door lets ordinary Open Payments clients reach Dassie nodes that have no public address, at `https://<node ID>.<domain>`, and at `https://<name>.<domain>` for a name a node holds ([spec/areas/http.md](../../spec/areas/http.md#front-doors)). It is a mode of the `dassie` binary, `dassie front-door`, and no node: it has no ledger and no wallet, and speaks the peer protocol only to read the bootstrap nodes' name lists. The installer never sets one up; these files are for an operator who runs one, such as `dassiepay.net`.

## What it needs

- **A host with a public address,** and **UDP and TCP port 443 open** to it: nodes connect over QUIC on UDP, clients over HTTPS on TCP, and nodes whose UDP is blocked carry their QUIC over a WebSocket on TCP, at `https://relay.<domain>/quic`. It also reaches the network's bootstrap nodes over QUIC, on UDP 443 out, to read who holds each name.
- **The network's bootstrap nodes,** the registrars of names, as its nodes are configured with them: `DASSIE_BOOTSTRAP_NODES`, by default the public testnet's, as for a node. Until it has read the lists of enough of them after a start, names do not resolve; node IDs do.
- **A wildcard DNS record,** `*.<domain>`, `A` (and `AAAA`, if the host has IPv6) to the host. It covers every node's name, and `relay.<domain>`, where nodes connect.
- **A certificate for `*.<domain>`.** A wildcard needs the DNS-01 challenge, for example with certbot and its plugin for the DNS provider (on Google Cloud DNS, `python3-certbot-dns-google`, with a service account that may edit that one zone, [below](#a-dns-credential-for-one-zone-alone)):

  ```sh
  certbot certonly --non-interactive --agree-tos --register-unsafely-without-email \
    --dns-google --dns-google-credentials /etc/letsencrypt/dns.json \
    -d '*.dassiepay.net' \
    --deploy-hook /etc/letsencrypt/renewal-hooks/deploy/dassie-front-door.sh
  ```

  `deploy-certificate.sh` is that hook: it copies the certificate where the front door reads it, readable by its user, so that user must exist before the first certificate. certbot runs the hook at the first issuance too, and at every renewal. The front door reads the files again every minute, so a renewal takes effect without a restart. Without an email address Let's Encrypt has no one to write to, which it no longer does about expiry anyway; give one with `-m` if you want its notices.
- **Its own key,** an Ed25519 key in `/var/lib/dassie-front-door/front-door.key`, as 32 bytes of seed or PKCS#8. The unit makes one on its first start (`dassie front-door --make-key`), and the front door derives its SLH-DSA key from it, which its node ID commits to as well. Keep it: nodes name the front door by its ID, and a new key means every node must be told again. `dassiepay.net`'s entry is every node's default relay (`DEFAULT_RELAY` in `crates/dassie-node/src/env.rs`), so a new key takes a release. Back it up like any secret.

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
# dns.resourceRecordSets.*). gcloud has no add-iam-policy-binding for a
# zone: read its policy, add the binding, and set it back.
gcloud dns managed-zones get-iam-policy "$ZONE" --project "$PROJECT" \
  --format json > zone-policy.json
# Add to zone-policy.json's "bindings" (create the list if it is missing):
#   {"role": "roles/dns.admin", "members": ["serviceAccount:<SA>"]}
gcloud dns managed-zones set-iam-policy "$ZONE" zone-policy.json --project "$PROJECT"

# Reading at the project, which certbot-dns-google needs to list the zones
# and find this one (dns.managedZones.list). It reads what DNS publishes
# anyway, and changes nothing.
gcloud projects add-iam-policy-binding "$PROJECT" \
  --member "serviceAccount:$SA" --role roles/dns.reader

# Its key, for certbot only: root's, mode 0600. Made where an administrator
# is logged in to gcloud, since the front door's host holds no credential,
# and piped straight to the host, so it is written to no other disk.
gcloud iam service-accounts keys create - --iam-account "$SA" |
  ssh root@<host> 'install -d -m 0700 /etc/letsencrypt &&
    install -m 0600 -o root -g root /dev/stdin /etc/letsencrypt/dns.json'
```

Run the VM without the default compute service account and its scopes (`gcloud compute instances create … --no-service-account --no-scopes`), so the metadata server hands out no other credential; certbot's key is then the only one on the host.

## Setting it up

1. Make its user, before anything that needs its group: `useradd --system --no-create-home --shell /usr/sbin/nologin dassie-front-door`.
2. Get the certificate as above. certbot runs the hook itself, so the front door's copy is in place at once.
3. Install `dassie` as `/usr/local/bin/dassie`, from a release bundle whose signature you checked (the installer's way, `install.sh`, also sets up a node, which a front door's host does not want).
4. Install `dassie-front-door.service` in `/etc/systemd/system/`, set `DASSIE_FRONT_DOOR_DOMAIN` in it if the domain is not `dassiepay.net`, and `systemctl enable --now dassie-front-door`.
5. Read the entry that names it: `journalctl -u dassie-front-door | grep DASSIE_RELAY`.

`dassie --help` lists the settings: each `DASSIE_FRONT_DOOR_*` variable has a flag.

## Updating it

Install the new `dassie` as in step 3, and with it the release's `dassie-front-door.service`, keeping your changes to it (such as `DASSIE_FRONT_DOOR_DOMAIN`): a new release may need more of the unit, such as the file limit (`LimitNOFILE`) its tunnels need. Then `systemctl daemon-reload && systemctl restart dassie-front-door`. Without the reload, systemd keeps running the front door under the unit it read before.

## Naming it as a node's relay

A node without a hostname keeps a connection to the `dassiepay.net` front door by default, and answers at `https://<node ID>.dassiepay.net`, its default name ([spec/areas/node.md](../../spec/areas/node.md#a-chosen-relay)). It closes that connection once it has a hostname.

To keep another front door instead, name it as the node's relay with the entry that front door logs, in the node's environment:

```sh
systemctl edit dassie
# [Service]
# Environment='DASSIE_RELAY={"id":"<node ID>","url":"https://relay.example.com","publicKey":"…","slhDsaPublicKey":"…"}'
```

and list its name for the node: `DASSIE_PUBLIC_NAMES=<node ID>.<domain>` ([spec/areas/payments.md](../../spec/areas/payments.md#the-nodes-names)). That name is also what makes the node answer at `<name>.<domain>` for each name it holds; a node not named there answers 421 to them. A node keeps a relay it is told whether or not it has a hostname. `Environment=DASSIE_RELAY=` (empty) keeps no relay at all.
