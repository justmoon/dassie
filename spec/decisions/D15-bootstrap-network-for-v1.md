# D15. Bootstrap network for v1

**Status:** active, confirmed 2026-10-11; done 2026-10-16.
**Areas:** [Network](../areas/network.md)

## Facts

The original's default Bootstrap Node List is `test1.dassie.xyz` and `test2.dassie.xyz`, two v0 nodes that refuse every v1 message ([D2](D02-peer-protocol-v0-or-v1.md)).

## Decision

The project runs its own v1 bootstrap nodes, at least three so that one failure does not block majority, under new names in `dassie.xyz`, on separate regions. They are the default Bootstrap Node List, with their node IDs checked against their public keys at startup (NET-19). [Network](../areas/network.md) states the list and how to replace it.

`boot1`, `boot2` and `boot3.dassie.xyz` run in GCP project `dassie-390023` (us-east1, europe-west3, asia-northeast1). `dassie.xyz` is a Cloud DNS zone in the same project, and their recovery phrases are the Secret Manager secrets `dassie-boot<N>-phrase`. Each also runs `xrpl-testnet`, so a new node has someone to peer with.
