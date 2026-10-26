# A hub's keys name its network, except on the testnet, and a validator validates on its owner's network

**Areas:** [Settlement hubs](../areas/hubs.md#the-escrow), [Identity](../areas/identity.md#derivation-chain), [Node](../areas/node.md#configuration).

## Facts

- A validator's consensus, escrow and payout keys, and the sponsor's escrow master key, derive from the node key under a path that names the hub's nonce. The sponsor picks the nonce; the hub ID cannot name the keys, since it is the hash of rules that contain them.
- The XRP Ledger's testnet and mainnet take the same signed bytes: a transaction names no network on either ([real-money](real-money.md)). An escrow of the same address with the same signer list on both networks would be kept apart only by its sequence, which starts at the index of the ledger that created it, and the two networks' indexes are far apart ([Settlement hubs](../designs/settlement-hubs.md#2-founding-a-hub)). One key on two networks is also one key to lose for both.
- Hubs already run on the testnet, the built-in default hub among them. Their rules name their validators' keys, and their sponsors make their escrows again from the master key after a reset. A changed path would orphan them all.
- A member's key derives from the hub ID, the hash of rules whose terms name the network.
- Hubs hold no real money while the live realm is refused ([real-money](real-money.md)), yet mainnet hubs must be founded and tested before the launch, by the dev switch that turns on the live realm ([mainnet-schemes](mainnet-schemes.md)).

## Decision

- **On every network but the testnet, a hub's keys name the network,** at `node/hub/network-<network ID>/<nonce>/<name>`. **The testnet's paths stay** `node/hub/<nonce>/<name>`, without it, so every testnet hub keeps its keys. No path of one network is another's: the testnet's third segment is 64 hex digits, never `network-…`.
- **A member's key keeps its path,** `node/hub/<hub ID>/account`: the hub ID already differs by network.
- **A validator validates on one network, its owner's,** named in its validator file and the testnet unless the file names another. It accepts terms, signs a genesis and takes a certificate only on that network, and the file's own hubs must be on it. A network of the live realm is refused where the live realm is, so no file turns a node to validating hubs of real money.

## Accepted

- **Two path shapes,** the testnet's and every other network's, for good: the testnet's came first.
- **A sponsor still founds on the testnet only,** funding the escrow from its testnet XRP, until mainnet hubs are funded from the owner's mainnet book ([roadmap](../roadmap.md)).

## Rejected

- *The network in every path, the testnet's included:* every running testnet hub would lose its validators' keys and its escrow, the default hub with them.
- *Nonces that never repeat across networks, with paths unchanged:* the sponsor picks the nonce, and a careless or hostile one could reuse a testnet nonce on mainnet; a key's network would rest on a promise, not on its derivation.
- *The network in the member's path too:* the hub ID names it already, so it would add a second shape of path and no key of its own.
- *A validator that takes any network the terms name:* an owner who set up a testnet validator would find it validating hubs of real money at a sponsor's word.
