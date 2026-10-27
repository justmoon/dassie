# Identity

Everything a node is derives from one BIP-39 recovery phrase: its Ed25519 node key and node ID, its login secret, its X25519 key for peer authentication, and the entropy its settlement schemes build wallets from. The owner keeps only the phrase, so the same phrase must always recover the same node and the same funds.

**Paths:** `crates/dassie-identity/`, `packages/wasm/`
**Read with:** [Encoding](encoding.md), [Peer protocol](peer-protocol.md), [HTTP](http.md), [XRPL](xrpl.md), [Node](node.md)

## Derivation chain

The phrase gives a master seed, and every secret is a node of an HMAC-SHA256 tree of seed paths hanging off it (`src/seed.rs`; the chain is drawn in `src/lib.rs`). The node key is the path `node`; everything the node derives hangs below it, so a node holding only its node key derives the same `node/…` secrets the browser derives from the phrase.

The chain to the node key, its public key and node ID, and every frozen path below it, are a compatibility surface. No step, no algorithm parameter and no frozen path may change, or the same phrase would give a different node: a different node ID to its peers, a different XRPL account with the owner's funds in the old one, hub keys its rules no longer name. There is no versioning to migrate through; a change is a new node.

What pins it: **golden vectors** (`tests/data/chain.json`, `keys.json` and `paths.json`) from a generator of our own (`testdata/identity/`), which computes them with libraries independent of Dassie's code. They pin phrases of every length, the master seed, node key, public key, node ID, login secret, the XRP Ledger accounts' entropy and wallets, the X25519 conversion and a shared secret, and every path below the node key with its secret, with what a hub's rules and escrow name of a hub's keys. They run natively (`tests/vectors.rs`, and a hub's keys in `crates/dassie-hub/tests/keys.rs`) and through the WASM build (`tests/interop/identity_wasm_test.ts`), so the browser derives exactly what the node derives.

## Seed paths

Every secret below the node key is a variant of `NodeSecret` (`src/paths.rs`): it takes typed arguments and writes its path itself, and no other crate writes one, so no two uses meet at a path. A new secret is a new variant at a new path, never a new meaning for an existing one. `NodeSecret::is_frozen` says which paths may never change: those whose secret something outside the release depends on. They are the XRP Ledger accounts' keys and a hub's keys, which hold funds and which a hub's rules name; the Open Payments key, which other wallets know the node by; and the STREAM receivers' secrets, whose credentials other wallets hold. Not frozen: the login secret, since the wallet and `dassie setup`, which derive the proof, ship in the release of the node that checks it ([HTTP](http.md#login-and-logout)); and the QUIC endpoints' keys for stateless resets and connection IDs, each endpoint its own, since a changed one only makes peers wait out the idle timeout once, for connections a crash left behind ([Peer protocol](peer-protocol.md#quic)).

- **A STREAM receiver's secret** names its ledger by the ledger's ID, the name its books have, not by the slug its addresses show, so the secret follows the books rather than a display name ([STREAM](stream.md#credentials-and-tokens)).
- **A hub's keys** name the hub's network and nonce, `node/hub/<network ID>/nonce/<nonce>/<key>`, since a transaction on the testnet or mainnet names no network; a member's key names the hub ID, `node/hub/member/<hub ID>/account`, which names the network already ([hub-keys-by-network](../decisions/hub-keys-by-network.md)).

Nothing but the node key hangs off the master seed directly, and the node never has the master seed: only the browser holds the phrase and the master seed, during setup and login ([HTTP](http.md)), or, for an owner who keeps the phrase on the node's host, `dassie setup` and the market maker, which wipe them once they have the node key ([headless-setup](../decisions/headless-setup.md)).

Users of the tree: the login secret ([HTTP](http.md)), the node's XRP Ledger accounts at `node/xrpl/<network ID>/account/<index>`, which every scheme on an account settles through ([XRPL](xrpl.md#the-wallet)), the STREAM receivers ([Payments](payments.md#receiving)), the Open Payments key ([Payments](payments.md#wallet-addresses)), the QUIC endpoints' keys ([Peer protocol](peer-protocol.md#quic)), and a settlement hub's keys: a validator's consensus, escrow and payout keys, a sponsor's escrow master key, and a member's account key ([Settlement hubs](hubs.md#the-escrow)).

## Node key

Signing is pure Ed25519. **Verification follows ZIP-215** (`ed25519-zebra`), not `ed25519-dalek`'s cofactorless `verify`: ZIP-215's rules are exact, so every implementation agrees on every signature, edge cases included, and being stricter than the original could split the network on edge-case signatures.

The price is **small-order keys**: under ZIP-215 a signature with `s = 0` and a small-order `R` verifies every message under a small-order public key. So every place where a key must prove something refuses weak keys (`PublicKey::is_weak`): link states and the senders of authenticated peer messages ([Peer protocol](peer-protocol.md)), rate feeds ([Rate feed](rate-feed.md#signing)), Open Payments client keys ([Payments](payments.md)) and the release key ([Release](release.md)). Keys derived from a seed are never weak.

Node keys, signatures and node IDs stay Ed25519 for good, since the chain to the node ID is fixed; post-quantum protection would cover only the peer protocol's key agreement ([roadmap](../roadmap.md#smaller-open-points)).

## Node ID

The node ID is the short, self-certifying name of a node (`src/node_id.rs`): the first 24 bytes of a SHA-256 of the public key under a versioned OID.

- **Its text** is those bytes in lowercase base32 (RFC 4648, no padding): 39 characters of `[a-z2-7]`, one form wherever a person or a protocol reads it. It is the node's segment in ILP addresses, its DNS label at a front door, `<node ID>.<domain>` ([Payments](payments.md#the-nodes-names)), what logs and the wallet show, and what configuration names. Text is read in either case, since DNS ignores case, and its three unused last bits must be zero, so that each ID has one text. Base32 rather than base64url so that one form serves DNS too; 24 bytes so that it fits a DNS label, 63 characters, with room to spare.
- **On the wire** it is its 24 bytes, through the one encoding `NodeId` gives (`Encode` and `Decode`), which every wire format uses: fixed in length, so no format carries an ID's length or bounds it, and every 24 bytes are an ID.

The ID is frozen with the [derivation chain](#derivation-chain): DNS names, clients' records and the bootstrap node lists are built from it, and a client checks the key a node signs with against the ID its host names ([Payments](payments.md#signed-responses)).

An ID received from elsewhere is only a name until checked: it must equal the ID computed from the key it comes with, and a link state, peering request or authenticated message whose ID and key disagree is refused ([Peer protocol](peer-protocol.md)).

## Object identifiers

Dassie's OIDs sit under the private arc `1.3.6.1.4.1.54392.5.1399`: `.1` for the peer protocol, `.2` for node fingerprints, `.3` for the [rate feed](rate-feed.md#signing). Under `.1`, `.1.3` signs link states and `.1.4` is the domain of a name's commitment ([Peer protocol](peer-protocol.md#names)). They are hashed into node IDs and signed into link states and feeds, so an arc is never reassigned, including `.1.1`, which the original defined and nothing uses. A new version of a signed structure takes a new OID, so a signature over the old one cannot pass as the new ([Peer protocol](peer-protocol.md)).

## X25519 key

Peer messages are authenticated with a key agreed by static-static X25519 between the two node keys ([peer-protocol-v1](../decisions/peer-protocol-v1.md)). The Ed25519-to-X25519 conversions are libsodium's, so a node's X25519 key follows from its node key alone and peers need no second key. An all-zero shared secret is refused, so a peer cannot force a secret it knows with a small-order key. How the secret keys the MACs is the peer protocol's.

## Handling of secrets

Secrets (master seed, node key, derived secrets, X25519 and shared secrets) are wiped on drop, are not `Copy`, print redacted and compare in constant time. Errors never carry a secret, so they may be logged and shown. A mistyped phrase is an error, never silently a different node. The node stores its key as the raw seed ([Node](node.md), [secrets-at-rest](../decisions/secrets-at-rest.md)).

## In the browser and in scripts

`dassie-identity` and `dassie-stream` (with `dassie-ilp`'s codecs) are built to WASM (`node --run build:wasm`) and published as `@dassie/wasm`, so the wallet derives keys with exactly the code the node runs. Their bindings are one crate of their own, `dassie-wasm`, so the core crates carry no wasm-bindgen and never touch the platform's randomness: that edge is the bindings'. Each of its features is a module of its own, `dassie_identity` and `dassie_stream`, so the wallet loads identity without STREAM. One `web` build serves browsers and Node ([node-and-pnpm](../decisions/node-and-pnpm.md)): `.` for scripts and tests, `./web` for browsers. Development-only functions (`sign`, `mnemonicFromEntropy`) are left out of `./web`.

## Decisions

- [peer-protocol-v1](../decisions/peer-protocol-v1.md): peer authentication by static-static X25519 between node keys.
- [database-continuity](../decisions/database-continuity.md): an owner recovers by entering the phrase again, which gives the same node ID and accounts.
- [secrets-at-rest](../decisions/secrets-at-rest.md): the node key stays plaintext at rest.
- [tigerstyle](../decisions/tigerstyle.md): deterministic, bounded, no OS randomness in the core.
- [node-and-pnpm](../decisions/node-and-pnpm.md): one `web` build of the WASM modules for browsers and Node.
- [hub-keys-by-network](../decisions/hub-keys-by-network.md): a hub's keys name its network.
