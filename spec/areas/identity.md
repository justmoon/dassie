# Identity

Everything a node is derives from one BIP-39 recovery phrase: its Ed25519 node key, its node ID, its login secret, its X25519 key for peer authentication, and the entropy its settlement schemes build wallets from. The same phrase always recovers the same node, so these derivations never change.

**Paths:** `crates/dassie-identity/`, `packages/identity-wasm/`
**Read with:** [Encoding](encoding.md), [Peer protocol](peer-protocol.md), [HTTP](http.md), [XRPL](xrpl.md), [Node](node.md)

## Derivation chain

```
phrase ──PBKDF2──▶ master seed (64 B) ──HMAC "node"──▶ node key (Ed25519 seed, 32 B)
                       │                                ├─ public key ─▶ node ID
                       └─ dev/session                   ├─ node/login            (login secret)
                                                        ├─ node/settlement-module/<scheme>/<path>
                                                        ├─ node/stream-server/<ledger slug>
                                                        ├─ node/open-payments-client
                                                        └─ X25519 secret
```

Every byte of this chain is a compatibility surface: no step of it, and no path in it, may change, or the same phrase would give a different node.

- **`node`, `node/login` and `node/settlement-module/<scheme>/<path>`** are the original's. The same phrase must give the same node ID, the same login secret and the same settlement entropy as the original, so that an owner recovers their node and their funds with it. The XRPL wallet is derived from that entropy, byte for byte as the original did ([XRPL](xrpl.md#the-wallet)).
- **`node/stream-server/<ledger slug>`** keys the STREAM receivers. Changing it would void the STREAM credentials the node has handed out for its incoming payments ([STREAM](stream.md#credentials-and-tokens)).
- **`node/open-payments-client`** is the key the node signs its Open Payments requests with and publishes at its wallet addresses. Changing it would change the key other wallets know the node by ([Payments](payments.md#wallet-addresses)).

The golden vectors ([Test vectors](#test-vectors)) pin the original's part of the chain, the XRPL wallets included.

## Recovery phrase

BIP-39 with the English wordlist only.

- **Lengths.** 12, 15, 18, 21 or 24 words, for 16, 20, 24, 28 or 32 bytes of entropy. The checksum is the first `entropy_bits / 32` bits of `SHA-256(entropy)`.
- **New phrases** have 12 words, drawn from an RNG the caller injects. The wallet's `generateMnemonic` draws from the platform (`crypto.getRandomValues`).
- **Parsing** accepts any amount of ASCII whitespace between words and words in any case. It refuses:
  - input longer than 1024 bytes;
  - any non-ASCII character, since no English word has one;
  - a word count other than the five above;
  - a word not on the list (the error names its position, counted from 1);
  - a wrong checksum.

  A mistyped phrase is reported, never turned silently into a different node.
- **The canonical phrase** is lowercase words separated by single spaces. The seed is always computed from it.
- **The master seed** is `PBKDF2-HMAC-SHA512(password = canonical phrase, salt = "mnemonic", 2048 rounds, 64 bytes)`: BIP-39 with an empty passphrase. Dassie never uses a BIP-39 passphrase.

Only the browser holds the phrase and the master seed, during setup and login. The node keeps only the node key.

## Seed paths

A seed path names a secret in a tree of HMAC-SHA256 keys. The key of a path `a/b/c` hanging off key `k` is:

```
k_a   = HMAC-SHA256(key = k,   msg = utf8("a"))
k_ab  = HMAC-SHA256(key = k_a, msg = utf8("b"))
k_abc = HMAC-SHA256(key = k_ab, msg = utf8("c"))      -- 32 bytes
```

- **Valid paths** have 1 to 16 segments separated by `/`, at most 256 bytes in all. Each segment is non-empty and printable ASCII without the space (`0x21`–`0x7E`). An empty segment (`node//login`, a trailing `/`) is refused.
- **From the master seed,** every segment is folded, starting with the first. The node key is the path `node`.
- **From the node key,** a path must be `node` or start with `node/`. `node` is the key itself; `node/a/b` folds `a` and `b` from the node key. Any other path hangs off the master seed, which the node does not have, and is refused. Both routes give the same secret for the same `node/…` path.

The paths in use:

| Path | What it is | Used by |
|---|---|---|
| `node` | The node key (Ed25519 seed) | everything below |
| `node/login` | The login secret | [HTTP](http.md): setup and login |
| `dev/session` | A deterministic development session token. A sibling of `node`, so only a holder of the phrase derives it. Nothing in the node accepts it. | dev tools, vectors |
| `node/settlement-module/<scheme>/<path>` | A settlement scheme's entropy. `<scheme>` is 2 to 128 characters of `[a-z0-9-]`; `<path>` is a valid seed path. | [XRPL](xrpl.md#the-wallet): `xrpl-testnet` derives its wallet from `node/settlement-module/xrpl-testnet/seed-testnet` (the original's mainnet module used `xrpl`/`seed-mainnet`) |
| `node/stream-server/<ledger slug>` | The STREAM server secret of the receiver on a ledger. The slug is the part of the ledger ID before `+`: `stub`, `xrpl-testnet`. Derived, so issued credentials stay valid across restarts. | [Payments](payments.md#receiving), [STREAM](stream.md#credentials-and-tokens) |
| `node/open-payments-client` | The Ed25519 seed of the key the node signs its Open Payments requests with | [Payments](payments.md#wallet-addresses) |

## Node key

The node key is an Ed25519 key. Its 32-byte seed is the secret at `node`.

- **Signing** is pure Ed25519 (RFC 8032: no prehash, no context).
- **Verification follows ZIP-215** (with `ed25519-zebra`, its reference implementation): cofactored, accepting non-canonical encodings of `R` and of the public key, requiring a canonical `s`. These rules are exact, so every implementation agrees on every signature, edge cases included. `ed25519-dalek`'s own `verify` is cofactorless and would refuse some signatures ZIP-215 accepts.
- **Small-order keys.** Under ZIP-215, a signature with `s = 0` and a small-order `R` verifies for every message under a small-order public key, so such a signature proves nothing. `PublicKey::is_weak` tells them apart. Every place where a key must prove something refuses weak keys: link states and the sender keys of authenticated peer messages ([Peer protocol](peer-protocol.md)), Open Payments client keys ([Payments](payments.md)) and the release key ([Release](release.md)). Keys derived from a seed are never weak.
- **Public keys** are parsed from their 32-byte compressed encoding, which must decode to a curve point. Non-canonical encodings are accepted, as ZIP-215 accepts them. The node ID and signatures are computed over the 32 bytes as received.
- **DER forms** exist for compatibility with the original's PEM keys. Only these exact encodings decode:

  ```
  PKCS#8 (48 B):  30 2e 02 01 00 30 05 06 03 2b 65 70 04 22 04 20 || seed32
  SPKI   (44 B):  30 2a 30 05 06 03 2b 65 70 03 21 00 || pub32
  ```

  The SPKI form has its unused-bits byte (`00`), which the original's constant left out. The node stores its key as the raw 32-byte seed, not as PEM ([Node](node.md)).
- **Post-quantum.** Node keys, signatures and node IDs stay Ed25519, since the chain from the phrase to the node ID is fixed. A post-quantum scheme would protect only the peer protocol's key agreement, with a key of its own carried in the link state; it is not built ([roadmap](../roadmap.md#smaller-open-points)).

## Node ID

The node ID is the short, self-certifying name of a node.

```
input  = OER SEQUENCE { versionPrefix OBJECT IDENTIFIER 1.3.6.1.4.1.54392.5.1399.2.1,
                        nodePublicKey OCTET STRING }
       = 0d 2b 06 01 04 01 83 a8 78 05 8a 77 02 01 || 20 || publicKey32       (47 bytes)
digest = SHA-256(input)
b64    = base64url(digest[0..24]) without padding                            (32 characters)
nodeId = "d" || b64 with trailing 'A' characters removed
```

- A computed ID is `d` followed by up to 32 characters of `[A-Za-z0-9_-]`, so 1 to 33 characters in all. Stripping the trailing `A`s is a quirk of the original, kept because node IDs must not change.
- **Parsing** an ID from elsewhere applies the wire schema: 2 to 45 characters of `[A-Za-z0-9_-]`. Whether an ID belongs to a key is a separate check: it must equal the ID computed from the key. A link state, a peering request or an authenticated message whose ID and key disagree is refused ([Peer protocol](peer-protocol.md)).
- **Order.** IDs compare as byte strings. Where two nodes' requests cross, the lower ID wins ([Network](network.md)).
- **ILP address.** A node's address is `<scheme>.das.<nodeId>`, where `<scheme>` is `test` in the `test` realm and `g` in the `live` realm ([Connector](connector.md), [Node](node.md)).

## Object identifiers

All Dassie OIDs sit under `1.3.6.1.4.1.54392.5.1399`.

| OID | Name | Use |
|---|---|---|
| `…1399.1` | `dassiePeerProtocolId` | prefix of the peer protocol's OIDs |
| `…1399.1.2` | `dassiePeerNodeInfoId` | v0 signed node info; a v0 link state |
| `…1399.1.3` | v1 `PeerNodeInfo` | v1 link states, so a v0 signature is not a v1 link state ([Peer protocol](peer-protocol.md)) |
| `…1399.2` | `dassieNodeFingerprintId` | prefix of node fingerprints |
| `…1399.2.1` | `dassieNodeFingerprintV1Id` | the node ID's version prefix (above). OER: `0d 2b 06 01 04 01 83 a8 78 05 8a 77 02 01` |

`…1399.1.1` (`dassiePeerSignedHelloId`) was defined by the original and is unused.

## X25519 key

Peer messages are authenticated with a key agreed by static-static X25519 between the two node keys ([D2](../decisions/D02-peer-protocol-v0-or-v1.md)). The conversions are libsodium's, so a node's X25519 key follows from its node key alone:

```
x25519 secret = clamp(SHA-512(seed)[0..32])            -- crypto_sign_ed25519_sk_to_curve25519
x25519 public = u = (1 + y) / (1 − y) mod p            -- crypto_sign_ed25519_pk_to_curve25519, from the Ed25519 point
shared        = X25519(secret, peer's x25519 public)
```

- An all-zero shared secret is refused (RFC 7748 §6.1): a peer key of small order would let the peer force a secret it knows.
- Any 32 bytes are accepted as an X25519 public key, as RFC 7748 requires; the zero check catches the weak ones.
- How the shared secret keys the MACs is the peer protocol's ([Peer protocol](peer-protocol.md)).

## Login secret

The login secret is the 32-byte secret at `node/login`.

- The browser derives it from the phrase, through the master seed. The node derives the same bytes from its node key.
- Setup sends the node key and the login secret as proof; login sends the login secret ([HTTP](http.md)).
- The node compares a proof in constant time. A proof of the wrong length is refused straight away, since the length is public.

## Handling of secrets

- The master seed, the node key, derived secrets, the login secret, X25519 secrets, shared secrets and the PKCS#8 encoding are wiped from memory when dropped. They cannot be copied implicitly, print as `<redacted>`, and compare only in constant time.
- The crate is deterministic ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)). It reads no clock and no OS randomness: randomness comes from an RNG the caller passes in. It does not allocate (`no_std`).
- Every error describes bad input, never a bug: phrase errors (too long, non-ASCII, word count, unknown word, checksum), entropy length, seed path, a path not under `node`, scheme ID, node ID, public key, signature, DER, a non-contributory Diffie-Hellman result, and base64url. Errors carry no secret, so they may be logged and shown. Bugs are assertions.
- Base64url here is unpadded RFC 4648 §5, decoded strictly: no padding, no whitespace, and the unused low bits of the last character must be zero, so each byte string has one accepted encoding.

## In the browser and in scripts

`dassie-identity` is also built to WASM, so the wallet derives keys with exactly the code the node runs.

- **The build.** `node --run build:wasm` (`scripts/build-wasm.ts`) builds `dassie-identity` and `dassie-stream` (which includes `dassie-ilp`'s packet codecs) for `wasm32-unknown-unknown` with the `wasm` feature and the `wasm` profile, then binds each with the `wasm-bindgen` CLI, `--target web`, into `packages/identity-wasm/gen/web`. The CLI's version must equal the `wasm-bindgen` crate's in `Cargo.lock`, or the script fails.
- **One `web` build** serves browsers and Node ([D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md)). The package `@dassie/identity-wasm` exports:
  - `.` (`mod.ts`): for scripts and tests. It instantiates both modules at once from the files next to it, and exports the identity functions and the STREAM and ILP bindings.
  - `./web` (`web.ts`): for browsers. `loadIdentity()` fetches the module from the page's own origin (bundlers emit it as an asset) once per page and only then hands out the functions, so none can be called too early. A failed load is not remembered, so a later call tries again.
  - `./web/stream` and `./stream`: STREAM ([STREAM](stream.md#in-the-browser-and-in-scripts)).
- **The identity functions.** Keys and secrets cross the boundary as `Uint8Array`s, phrases as strings; invalid input throws with the error's text.

| Function | Does |
|---|---|
| `generateMnemonic()` | a new 12-word phrase from the platform's RNG |
| `validateMnemonic(phrase)` | whether the phrase parses: known words and checksum |
| `normalizeMnemonic(phrase)` | the canonical phrase |
| `mnemonicFromEntropy(entropy)` | the phrase of 16–32 bytes of entropy (the dev environment's vanity table; not in `./web`) |
| `deriveNodeKey(phrase)` | the node key |
| `loginSecret(nodeKey)` | the login secret |
| `devSessionToken(phrase)` | the secret at `dev/session` (not in `./web`) |
| `publicKey(nodeKey)` | the Ed25519 public key |
| `nodeId(publicKey)` | the node ID |
| `sign(nodeKey, message)` | an Ed25519 signature (not in `./web`) |

## Test vectors

The vectors are generated from the original by `testdata/original/` and checked by the native tests and, through the WASM build, by `tests/interop/identity_wasm_test.ts`, so the wallet derives exactly what the node derives.

- **The chain** (`tests/data/chain.json`): 64 phrases from the dev vanity table, each with entropy, phrase, master seed, node key, public key, node ID, login secret, development session token, and the XRPL testnet and mainnet entropy and wallets ([XRPL](xrpl.md#the-wallet)). The login secret derived from the master seed equals the one derived from the node key.
- **The bootstrap nodes of the original:**
  - public key `QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM` (base64url) → `dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW`;
  - public key `tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ` → `dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U`.
- **Seed `01`×32:** public key `8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c`, node ID `dagvMuaJff2H5DrnJw8llmK-WZ83WrSzY`.
- **Seed `03`×32:** Ed25519 public key `ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1`, X25519 public key `75e270df2952c57ba8367ba8618c178f9fe50db2799d304e74e918d985686146`.
- **Shared secret:** seed `02`×32 (public key `ce8d3ad1ccb633ec7b70c17814a5c76ecd029685050d344745ba05870e587d59`) with the seed-`03` public key gives `6b43e8f2fc80e7891620dba266ebfd61a3cd1a37515f29c039c5883952d6d32d`.
- **Fixes** each have a test of their own: a bad checksum and an unknown word are refused, the SPKI has its unused-bits byte, PKCS#8 decodes only exactly, the login proof is checked in constant time, empty path segments are refused, and a non-contributory X25519 result is refused. The ZIP-215 tests check the small-order cases, refuse a non-canonical `s`, and check that keys derived from seeds are never weak.

## Decisions

- [D2](../decisions/D02-peer-protocol-v0-or-v1.md): peer authentication by static-static X25519 between node keys; the X25519 conversion is this crate's, the MAC scheme the peer protocol's.
- [D3](../decisions/D03-database-continuity.md): a v0 owner recovers by entering the phrase again, which gives the same node ID and accounts.
- [D10](../decisions/D10-secrets-at-rest.md): the node key stays plaintext at rest.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): deterministic, bounded, no OS randomness in the core.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): one `web` build of the WASM modules for browsers and Node.

## Original

- [3 §2](../original/03-peer-network.md): node key, seed paths, node ID, OIDs and X25519 conversion in the original.
- [2 §8](../original/02-node-runtime.md): setup, login and the derived values.
- [5 §4.5](../original/05-accounting-and-settlement.md): settlement-module entropy.
- [4 §3.1](../original/04-interledger.md): the node ID as the connector computed it.
