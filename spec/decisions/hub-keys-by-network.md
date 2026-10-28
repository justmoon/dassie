# A hub's keys name its network, and a validator validates on its owner's network

**Areas:** [Settlement hubs](../areas/hubs.md#the-escrow), [Identity](../areas/identity.md#seed-paths), [Node](../areas/node.md#configuration).

## Facts

- A validator's consensus, escrow and payout keys, and the sponsor's escrow master key, derive from the node key under a path that names the hub's nonce. The sponsor picks the nonce; the hub ID cannot name the keys, since it is the hash of rules that contain them.
- The XRP Ledger's testnet and mainnet take the same signed bytes: a transaction names no network on either ([real-money](real-money.md)). An escrow of the same address with the same signer list on both networks would be kept apart only by its sequence, which starts at the index of the ledger that created it, and the two networks' indexes are far apart ([Settlement hubs](../designs/settlement-hubs.md#2-founding-a-hub)). One key on two networks is also one key to lose for both.
- A hub's rules name its validators' keys, and its sponsor makes its escrow again from the master key after a reset. A changed path orphans every hub founded under the old one. The testnet's hubs are founded again when the testnet resets, so its paths could still change.
- A member's key derives from the hub ID, the hash of rules whose terms name the network.
- Hubs hold no real money while the live realm is refused ([real-money](real-money.md)), yet mainnet hubs must be founded and tested before the launch, by the dev switch that turns on the live realm ([mainnet-schemes](mainnet-schemes.md)).

## Decision

- **A hub's keys name its network,** on every network, the testnet's included: `node/hub/<network ID>/nonce/<nonce>/<key>`.
- **A member's key names no network,** at `node/hub/member/<hub ID>/account`: the hub ID already differs by network. No member path is a validator's, since a network ID is a number and `member` is not.
- **A validator validates on one network, its owner's,** named in its validator file and the testnet unless the file names another. It accepts terms, signs a genesis and takes a certificate only on that network, and the file's own hubs must be on it. A network of the live realm is refused where the live realm is, so no file turns a node to validating hubs of real money.

## Accepted

- **The testnet's hubs end,** the default hub among them, and are founded again with their keys at these paths. Until the testnet's default hub is, none is built in, and the wallet does not offer the hub scheme.
- **A sponsor founds only on its realm's network,** funding the escrow from its XRP there: the testnet in the test realm, mainnet in the live realm, which runs only where it may ([Settlement hubs](../areas/hubs.md#founding-a-hub), [real-money](real-money.md)).

## Rejected

- *The testnet's paths without the network, and every other network's with it:* two shapes of path for good, to keep hubs that the testnet's reset ends anyway.
- *Nonces that never repeat across networks, with paths unchanged:* the sponsor picks the nonce, and a careless or hostile one could reuse a testnet nonce on mainnet; a key's network would rest on a promise, not on its derivation.
- *The network in the member's path too:* the hub ID names it already, so it would add a segment and no key of its own.
- *A validator that takes any network the terms name:* an owner who set up a testnet validator would find it validating hubs of real money at a sponsor's word.
