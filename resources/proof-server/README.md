# Running a proof server

A proof server is an XRP Ledger server, rippled's xrpld, patched with three public methods that let a Dassie node prove what it books without trusting the server: `validations`, `tx_proof` and `state_proof` ([spec/designs/xrpl-validation.md](../../spec/designs/xrpl-validation.md#3-the-proof-api)). The project runs one per network, `mainnet.xrpl.dassieland.net` and `testnet.xrpl.dassieland.net`, which the node names by default. Anyone can run another: it is relied on to answer, never trusted, so a node gains nothing from a server it knows and loses nothing to one it does not.

These files build the patched xrpld and run it on GCP: [`build.sh`](build.sh) builds it, [`provision.sh`](provision.sh) makes the VMs, and [`startup.sh`](startup.sh) sets each up at every boot.

## The patch

The patch is [`patches/`](patches/): two commits on a release of rippled, as `git format-patch` writes them.

- **libxrpl:** `SHAMap::getProofBranch` and `Validations::getAllForLedger`, with their unit tests.
- **xrpld:** the three handlers, with their unit tests.

It touches nothing else xrpld does, so it moves to each new release with a rebase:

1. Apply the patches to the new tag with `git am`.
2. Resolve what conflicts, and run rippled's tests (its `xrpl.rpc` suites and `xrpl_tests`).
3. Write the patches again with `git format-patch <tag>`.

The same commits on rippled's `develop` are what is offered upstream.

## Building

```sh
resources/proof-server/build.sh <rippled-tag> <version> <out-dir>
```

It builds xrpld from a release tag of rippled with the patches applied, with rippled's own toolchain:

- the compilers of its Nix development shell, pinned by its `flake.lock`;
- its dependencies from Conan, pinned by its `conan.lock`.

`<version>` is the tag with `+proofs.<n>` after it, such as `3.5.0-rc1+proofs.1`, so `server_info` shows which build a server runs. It needs Nix with flakes, and a few tens of gigabytes for Conan's cache and for the build, which runs in `$TMPDIR`.

It writes `xrpld`, `BUILDINFO` (the tag's commit, the patched commit and the hashes of the two lock files) and `SHA256SUMS`. Upload all three to the builds bucket:

```sh
gcloud storage cp <out-dir>/* gs://dassie-xrpld/<version>/
```

A server installs `https://storage.googleapis.com/dassie-xrpld/<version>/xrpld`, by that URL and its SHA-256.

## Setting them up

```sh
resources/proof-server/provision.sh <xrpld-url> <xrpld-sha256>          # prints the commands
resources/proof-server/provision.sh <xrpld-url> <xrpld-sha256> --apply  # runs them, once
```

It makes, in project `dassie-390023`:

- **A service account with no role,** and no secret anywhere: a proof server holds nothing worth taking.
- **The bucket `dassie-xrpld`,** readable by anyone, for the builds.
- **A network open to TCP 80 and 443 only.**
- **A VM per network** in us-east1, with a static address:
  - mainnet: e2-standard-8 with 250 GB of SSD;
  - testnet: e2-standard-4 with 150 GB of balanced disk.
- **The Cloud DNS zone `dassieland.net`,** with DNSSEC, and an `A` record per server. The domain's registrar must then delegate to the zone's name servers and publish its DS record (`gcloud dns dns-keys list --zone dassieland-net`).

At every boot, `startup.sh` does the following, logging to the serial console (`gcloud compute instances get-serial-port-output xrpl-proofs-<network>`):

- reads the network, the host name and the build from the instance's metadata;
- installs the build after checking its hash;
- installs this directory's files from `main`:
  - `xrpld-<network>.cfg` and `validators-<network>.txt` as xrpld's configuration;
  - `xrpld.service`;
  - `Caddyfile`;
- starts xrpld and Caddy.

**xrpld:**

- **History:** it follows its network, and keeps about two days of history. That is the history bound a node relies on ([spec/designs/xrpl-validation.md](../../spec/designs/xrpl-validation.md#6-the-account-on-proofs)). It answers `lgrNotFound` beyond that.
- **Ports:** its public ports listen on `127.0.0.1` alone. Its peer port reaches out to the network.
- **Validators:** each server follows its network's published validator lists. That decides only which ledgers it follows: a node checks every answer against its own UNL.

**Caddy** gets a certificate from Let's Encrypt and forwards WebSocket and JSON-RPC to xrpld:

- **Per-client limits.** xrpld charges each request to the client address Caddy forwards (`secure_gateway`), so one client that floods it gets `slowDown`, not everyone.
- **What it forwards.** Caddy sets that address itself and drops the client's `Forwarded` and `X-User`. xrpld takes the first address it is given, and lifts its limits for any user a gateway names.

## Updating

**A new build:** build and upload it, then point the VM at it and restart it.

```sh
gcloud compute instances add-metadata xrpl-proofs-<network> --zone us-east1-b \
  --metadata xrpld-url=<url>,xrpld-sha256=<sha256>
gcloud compute instances stop xrpl-proofs-<network> --zone us-east1-b
gcloud compute instances start xrpl-proofs-<network> --zone us-east1-b
```

Stop and start, not `reset`: a reset cuts the power, and xrpld does not close its databases. A restart loses the validations xrpld holds in memory, which it hears again within seconds. A changed configuration on `main` needs only the restart. To try one on a server before it merges, set the metadata `config-ref` to its branch, and remove it once it has merged.

**After a testnet reset,** xrpld holds a chain the network no longer has. Set `wipe-db` to a value it has not had and restart the VM: `startup.sh` wipes the database once per value, and xrpld syncs anew.

```sh
gcloud compute instances add-metadata xrpl-proofs-testnet --zone us-east1-b \
  --metadata wipe-db=$(date +%F)
gcloud compute instances stop xrpl-proofs-testnet --zone us-east1-b
gcloud compute instances start xrpl-proofs-testnet --zone us-east1-b
```

## Checking one

```sh
curl -s https://<host> -d '{"method":"server_info"}' | jq .result.info.build_version
curl -s https://<host> -d '{"method":"state_proof","params":[{"keys":["<64 hex>"]}]}' | jq .result
```

`server_info` names the build. `state_proof`, `tx_proof` and `validations` answer with proofs that a client checks itself.
