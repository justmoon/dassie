# Rate feed

A core node's public, cheap statement of what it converts at: for each ordered pair of its ledgers, the rate a packet gets, with its fee and each ledger's largest packet. It is a signed UDP datagram that the node makes once and sends to every subscriber, with no retries and no state beyond the subscriber list. Market makers watch feeds and trade through nodes whose rates make it worth it, which moves those nodes' holdings back toward balance.

`dassie-feed` holds the contract without IO: the wire format and its signature, the subscriber table and its cookies, when to publish, and a subscriber's side. The node service that publishes a feed, its socket and how a feed is found, and the market maker are not built yet ([roadmap](../roadmap.md#rate-feed-and-market-makers)).

**Paths:** `crates/dassie-feed/`, `testdata/feed/`
**Read with:** [Connector](connector.md#exchange-rates), [Identity](identity.md), [Encoding](encoding.md), [Node](node.md)

## What a feed says

- **Who and when.** The node's ID and public key, a sequence in milliseconds that grows with every feed the node makes, even across a clock that went back, and a time after which the feed no longer counts.
- **The fee,** in parts per million, as the connector takes it ([Connector](connector.md#fees-and-spread)).
- **The ledgers,** by ID string, each with its largest packet, in that ledger's units. The IDs are strings rather than the node's closed list of ledgers, so a subscriber reads a feed that names a ledger it does not know, and a new ledger needs no new feed version. The largest packet is per ledger although the node has one value today ([roadmap](../roadmap.md#not-built-yet)), so that per-ledger values need no new version either.
- **A rate per ordered pair** the node converts between: what one unit in gets out, scaled by 10^15, spread included. A packet that comes in with `amount` goes out with `floor((amount − fee) × rate / 10^15)`. A pair the node does not convert, for want of a fresh rate or an active scheme, is left out; a rate is never zero.

The node is to publish exactly the rates its connector applies, so that a feed says what a packet gets; that is the node service's to keep.

## Wire format

Canonical OER ([Encoding](encoding.md)), starting with a version byte. `src/wire.rs` has the layouts; three messages share them:

- **Subscribe,** from a subscriber, with the cookie the node handed it if it has one, padded with zeros to a fixed size;
- **Challenge,** from the node, handing out a cookie;
- **Feed,** from the node, signed.

Every datagram fits in `MAX_DATAGRAM_BYTES`, which one packet carries on any path IPv6 runs on, so a feed is never fragmented. The bounds on the ledgers and rates are chosen so that the largest feed they allow fits, which a constant assertion checks; signing never meets a feed that does not.

**Every value has one encoding.** Ledgers are in ascending order of their IDs and rates of their pairs, each once; padding is zeros; and decoding refuses anything else, so a feed's bytes are exactly what was signed, and a decoded datagram re-encodes to its bytes (a property test). Decoders never panic, whatever the bytes, and every list is bounded before it is read.

**Versions.** A datagram of another version is refused. Within a version, no layout may change: subscribers such as market makers run software of their own, outside the node's release ([overview](../overview.md#frozen-surfaces)). A change is a new version byte, and a feed of the new version is signed under a new OID.

## Signing

A feed is signed by the node key (Ed25519) over its own bytes, which start with the OID `…1399.3.1` ([Identity](identity.md#object-identifiers)), so a feed's signature never passes for a link state's or anything else the node signs.

A subscriber verifies by ZIP-215 over exactly the bytes received ([Identity](identity.md#node-key)), refuses a weak key, and refuses a feed whose node ID is not its key's or not the node it subscribed to. It then takes a feed only if its sequence is after the last one it took and it has not expired, so a replayed or delayed datagram never sets rates back.

A feed is public, signed but not encrypted: it says nothing the node's packets would not.

## Subscribers

A node sends its feed to each address in its subscriber table, once per feed, and forgets it.

### The cookie

UDP's source address is whatever the sender writes, so a node that sent its feed to every address a Subscribe came from could be aimed at a victim. An address is subscribed only once it has shown that it receives there:

- **A Subscribe without a valid cookie** gets a Challenge carrying one, and nothing else. A Subscribe is padded to at least a Challenge's size, so a node never answers an address it has not proven with more bytes than it got from it, and cannot amplify an attack.
- **The cookie is a MAC** (HMAC-SHA256, truncated) of the address and the epoch, under a secret the node draws when it starts and keeps in memory only (`src/cookie.rs`). So the node keeps nothing for an address until it is proven, nobody computes a cookie for an address whose datagrams they do not receive, and a cookie proves only the address and port it was made for. An IPv4 address mapped into IPv6 is the IPv4 address.
- **A cookie is taken in its epoch and the next,** so it lives one to two epochs. One from the epoch before is still taken, and the node hands out a fresh one with the answer, so a subscriber that renews in time is never challenged again.

### The table

- **Bounded in all and per client** (an IPv4 address, or an IPv6 /56, as the node's other per-client limits: `ClientKey` in `dassie-node-core`), so one party cannot take every place.
- **A full table refuses newcomers** and evicts nobody: every entry is a proven address that asked within its lifetime, and evicting one for a newcomer would let a party with many addresses push the others out. A refused subscriber is sent nothing, and tries again at its next renewal.
- **An entry lasts a fixed time,** renewed by every Subscribe with a valid cookie, and expires otherwise. Expiry follows the time the caller passes in.
- **In memory only.** After a restart the secret is new, every cookie is void, and subscribers are challenged and come back at their next renewal.

Every answer is a value the caller carries out: challenge, subscribed (perhaps with a fresh cookie), refused or ignored. A datagram that is not a Subscribe, or does not decode, gets nothing.

### Subscribing

A subscriber sends a Subscribe to the node at a fixed interval, well within an entry's lifetime and a cookie's, with the last cookie it was handed, and again at once when a Challenge hands it a new one (`src/receiver.rs`). It takes datagrams only from the node's address.

## Publishing

A node's rates move with every conversion, but a feed costs a signature and a datagram per subscriber. So a node publishes at most once per `FEED_MIN_INTERVAL_MS` while its rates change, and otherwise a heartbeat every `FEED_HEARTBEAT_MS`, so subscribers know the node is alive and its rates still hold. A change within the interval is published when the interval ends, never lost. A feed counts for a few heartbeats, so that one or two lost on the way do not leave a subscriber without rates (`src/schedule.rs`).

## Test vectors

`testdata/feed/vectors.json` pins the wire format: Subscribes, a Challenge, feeds up to the largest the bounds allow, and feeds that decode but must not verify, all signed with a fixed key. There is no other implementation to take them from, so the crate's own generator makes them (`examples/feed_vectors.rs`, which says how to run it); Ed25519 signs deterministically, so running it again gives the same file. `tests/vectors.rs` decodes each and makes its bytes again. A changed vector is a new feed version.

## Decisions

- [tigerstyle](../decisions/tigerstyle.md): deterministic, bounded, every decision a value.
