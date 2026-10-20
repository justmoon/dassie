# Periphery

A Dassie node needs a public IP address and a certificate for its hostname: other nodes reach it at the URL in its link state, and so do Open Payments clients. Most computers people own have neither. They sit behind a NAT, on a phone, or in a browser tab.

This design divides the network in two:

- **The core** is the network as it is: nodes with a public IP, expected to be stable and long-lived. They register, keep the node list, and forward packets for others.
- **The periphery** is every other node. A periphery node still keeps its own books, settles, sends and receives, and serves Open Payments. But it connects only to core nodes, never to another periphery node, and never forwards a packet for anyone else.

A periphery node always opens its connections itself, to nodes with a public IP, so no NAT stands in its way, and no hole punching is needed. The rest of the design covers what that takes:

- a transport that both sides can send on (§2);
- addresses and peerings for nodes outside the node list (§3, §4);
- a way to reach a periphery node by its ID, for its owner's apps and for Open Payments clients (§5–§7);
- receiving payments without paying anything first (§8).

This is a design ([D26](../decisions/D26-core-and-periphery.md)). Of it, QUIC between core nodes is built ([Peer protocol](../areas/peer-protocol.md#quic)), and so are a node's names, its signed responses and the `dassie` field of §7 ([Payments](../areas/payments.md#the-nodes-names)); the rest is not built yet. Durations and sizes in it are starting points, to be set from the deterministic simulation when the parts are built.

## 1. Concepts

| Term | Meaning |
|---|---|
| **Core node** | A node with a public IP that registers and keeps the node list ([Network](../areas/network.md#membership)). |
| **Periphery node** | A node that connects only to core nodes, never forwards, and does not register. |
| **Uplink** | A core node a periphery node peers with. |
| **Downlink** | A periphery node, as its uplink sees it. |
| **Relay** | A server that carries streams to the periphery nodes connected to it. Every uplink is a relay for its downlinks; a periphery node may also connect to a relay it chooses (§5). |
| **Front door** | An HTTPS server that takes requests from ordinary Open Payments clients and passes them to a periphery node through a relay, for example at `<id>.dassiepay.net` (§7). |
| **Record** | A periphery node's signed statement of where it can be reached: its uplinks and relays (§5). |
| **Home nodes** | The core nodes that keep a periphery node's record, computed from its ID (§5). |
| **Free receiver** | A periphery node that receives payments without having paid anything (§8). |

**What changes for the core.** Peer links move to QUIC (§2), core nodes take downlinks in a table of their own (§4), and they relay for their downlinks and keep records as home nodes (§5). Otherwise core nodes register, peer and route as before.

## 2. Transport: QUIC on every peer link

Every peer link, core or periphery, runs over QUIC. Peer traffic is heavy: a busy core node forwards a great many packets. One `POST /peer` per message carries an HTTP request for each packet, while QUIC streams carry the same messages over one connection that either side can send on.

- **Messages.** Each request is a bidirectional QUIC stream: the request is a v1 `PeerMessage`, the response a v1 `PeerResponse`. Both have the same bytes, MACs and replay rules as over HTTPS ([Peer protocol](../areas/peer-protocol.md)). The [statuses](../areas/peer-protocol.md#statuses) travel ahead of the body, with the same meaning. Messages keep their own authentication, so the transport does not need to be trusted for that.
- **Either side opens streams.** The core node sends to its downlink over the connection the downlink opened. That is all a periphery node needs to receive: it never accepts a connection.
- **Keys, not certificates.** The TLS 1.3 handshake inside QUIC uses a self-signed certificate for each side's Ed25519 node key, and each side checks that key against the node ID it expects. No certificate authority is involved, and no hostname has to match. The ALPN is `dassie/1`. A client that is not a node (an owner's app, a front door) presents a key of its own (§6, §7).
- **One connection per pair of nodes.** When two core nodes dial each other at once, the connection the lower node ID opened stays, as crossed peering requests resolve ([Network](../areas/network.md#asking-and-answering)).
- **Liveness.** Heartbeats ([Network](../areas/network.md#advertised-peers)) also keep the NAT's mapping open. A periphery node that loses its connection dials again, backing off from 1 s to 60 s.
- **Where UDP is blocked,** a periphery node falls back to TLS over TCP on port 443, with each stream framed by length, or to WebSocket. A browser node (§9) uses WebTransport, which can trust a self-signed certificate by its hash (`serverCertificateHashes`), or WebSocket.
- **Link-state URLs** keep their syntax, `https://host[:port]` ([Peer protocol](../areas/peer-protocol.md#link-states)). The host and port then name the QUIC endpoint, on UDP, 443 by default. A link-state extension says that a node speaks QUIC; a node that does not say so is reached over `POST /peer`, until a release drops it.

## 3. Periphery nodes

- **No URL.** A periphery node's link state has an empty `url` and a `periphery` extension entry ([Peer protocol](../areas/peer-protocol.md#link-states)). An empty URL is valid only with that entry. Every node must agree on what verifies ([Verification](../areas/peer-protocol.md#verification)), and this rule is safe only because a periphery node's link state never floods: only its uplinks see it, in its peering request, so only nodes that take downlinks need the rule.
- **No registration.** A periphery node registers with no bootstrap node, is in no node list, and takes no place in any node table. Its link state does not flood. Core nodes keep their downlinks apart from the node table (§4).
- **Peering.** A periphery node asks core nodes to peer, as junior ([Settlement](../areas/settlement.md#credit)), choosing them from the node list among those that say in their link state that they take downlinks. A core node accepts a downlink without probing a URL: the open connection shows the downlink is there. It never asks a downlink to peer.
- **Addresses.** Each uplink gives its downlink a child address over ILDCP, `<uplink address>.<downlink ID>`. A downlink with several uplinks has an address under each, and its STREAM receiver accepts packets for all of them. An incoming payment names one: that of an uplink the node is connected to when it creates the payment.
- **Routing.** An uplink routes everything beneath a downlink's child address to that downlink ([Connector](../areas/connector.md#addresses-and-endpoints)). Since the address lies beneath the uplink's own, the rest of the network needs no route of its own.
- **No forwarding.** A periphery node sends its own packets through its uplinks and takes packets addressed to it. It forwards nothing: a Prepare from an uplink that is not addressed beneath one of its own addresses gets F02.
- **Losing an uplink.** A periphery node keeps two uplinks where it can. If one goes, payments through it stop, and payments through the other continue. The node's balances with the one that went stay on the books ([D12](../decisions/D12-peering-lifecycle-and-liveness.md)).

## 4. Downlinks on a core node

A core node keeps its peerings with other core nodes, and its downlinks, under separate limits.

- **Core relationships:** at most 32 settlement channels with core nodes over the node's life, as against 512 for every peer today. A channel outlives its peering ([Settlement](../areas/settlement.md#settlement-channels)), so this limit needs [closing settled relationships](../roadmap.md#not-built-yet): one whose peering has ended, with nothing owed either way, nothing in flight and no settlement on its way, is closed and frees its place. Its ledger accounts stay, at zero, since the journal refers to them.
- **Downlinks** live in a table of their own, not in the node table. Each row holds:
  - the downlink's node ID and public key;
  - its scheme, and the scheme's data for it (on the XRP Ledger, its account address);
  - the credit we extend it;
  - when it was last heard from, and its replay floor;
  - what was settled each way, and the settlement message on its way, if any.

  With the downlink's two ledger accounts, that is about 0.5 KB per downlink on disk. A peer today costs about 1 KB on disk and 1.5–2 KB in memory, most of it the link state, kept both as bytes and parsed, which a downlink does not need.
- **In memory only while connected.** A downlink's replay guard, MAC keys and peering state are loaded when it connects, and dropped once it has been gone for a while. Its replay floor is on disk, as for any peer.
- **Limits:** at most 10 000 downlinks peered at once, and at most 100 000 rows, about 50 MB. A row with nothing owed either way and no peering is closed. When the rows are full, the node refuses new downlinks.
- **No withdrawal for room.** A downlink is not in any link state, so withdrawing it would only stop packets to it, and payments to it are what raise its room when it owes nothing. A downlink that owes nothing is never withdrawn or ended for room ([Network](../areas/network.md#advertised-peers)); one that owes and does not settle is ended, as a core peer is.
- **The XRPL scheme** keeps its peers' addresses in a table keyed by account, not in its state ([XRP Ledger](../areas/xrpl.md#state-and-restarts)). With many downlinks, its state would grow with every one and be written whole at every checkpoint, and a scheme that kept only the newest addresses could not recognise a settlement from a downlink it forgot.

## 5. Reaching a periphery node

A periphery node cannot be dialled. Everything that needs to reach it (its owner's app, a front door with an Open Payments request) goes through a relay: a server it keeps a connection to.

**Relays.** Every uplink is a relay for its downlinks: they are connected anyway. A periphery node may also keep a connection to a relay it chooses, which carries only relayed streams, never peer traffic. That relay is its fast path, and it keeps the node reachable before it has any peering, or while lookups fail.

**A relayed stream** is opened at a relay with a header naming the destination node ID and a purpose (`owner` or `payments`). The relay opens a stream to that node over the node's connection and joins the two. Inside it, the client runs its own TLS 1.3 session with the node, pinned to the node's key, so the relay sees only ciphertext and its size. A relay limits relayed streams per destination and per client address. Relayed traffic is light: owner calls and Open Payments, never packets.

**The record.** A periphery node signs a record of where it can be reached:

```
PeripheryRecord ::= SEQUENCE {
  type         OBJECT IDENTIFIER,                    -- its own, so a record never passes for a link state
  nodeId       NodeId,
  publicKey    OCTET STRING (SIZE(32)),
  sequence     UINT64,                               -- a timestamp, as a link state's
  expiresAtMs  UINT64,
  uplinks      SEQUENCE (SIZE(0..8)) OF NodeId,
  relays       SEQUENCE (SIZE(0..4)) OF SEQUENCE {
                 host  VisibleString (SIZE(1..253)),
                 port  UINT16,
                 key   OCTET STRING (SIZE(32)) } }
SignedPeripheryRecord ::= SEQUENCE { record OCTET STRING CONTAINING PeripheryRecord, signature OCTET STRING (SIZE(64)) }
```

A record lasts 30 minutes and is renewed every 10, and whenever it changes.

**Home nodes.** Every core node knows the same core nodes: the majority node list ([Network](../areas/network.md#node-lists-and-the-majority)). A periphery node's home nodes are the five nodes of that list that rank highest by `SHA-256("dassie home" ‖ periphery ID ‖ core ID)`. Anyone with the list computes the same five, so a lookup is a single request, with no routing through a distributed table.

- **Storing.** An uplink stores its downlink's record with the downlink's home nodes, as an authenticated peer message, when the downlink connects and whenever the downlink renews it. A home node takes a record only from a core node the record names as an uplink, and a record is about 200 bytes.
- **Sharing a home node.** A core node can name any number of keys it made itself as its downlinks, since keys cost nothing. So a home node bounds its records in all, and gives each submitting core node a fair share of them: when it is full, a new record displaces the oldest of the core node that holds the most, never one of a node within its share. A few registered core nodes can then crowd out only each other, not the records of every other uplink.
- **Forging.** A record is signed by its node, so a home node can withhold it but not forge it. A newer sequence replaces an older one.
- **Looking up.** A node or front door asks one of the five home nodes, and the next if it has no answer. It caches a record until it expires.
- **When the list changes,** a periphery node's home nodes may change; its uplinks store the record with the new ones at the next renewal.

**Reaching a node,** a front door or an owner's app tries, in order:

1. its own connection to the node, if the node chose it as relay;
2. the relays the node names in its wallet address document (§7), if it has fetched it;
3. a lookup at the node's home nodes, then one of the uplinks or relays from the record.

**Grinding.** An attacker who wants to hide a node can generate core node keys until five of them rank highest for its ID. Each must register and stay in the majority, under the limits of [registration](../areas/network.md#registration) and, later, its fee. It can only withhold the record, and a relay the node chose still reaches it.

## 6. Owner apps

The wallet is never served through a relay or a front door. The owner enters the passphrase into the wallet's page to log in ([Wallet](../areas/wallet.md#key-derivation)), so a page served by anyone else could keep it, and a passphrase cannot be revoked. A periphery node's owner uses the wallet on `localhost`, or an app: a desktop app built with Tauri, or a mobile app.

- **The node serves the wallet, `/api/*` and `/rpc` only on its own listeners.** On a relayed stream it speaks only the owner protocol below, or Open Payments.
- **Pairing.** On `localhost`, the wallet shows a code, as a QR code, with the node ID and a one-time pairing secret that expires in minutes. The app makes a device key (Ed25519) and sends its public key with the secret. The node keeps a bounded list of devices, which the owner can revoke one by one.
- **Calls.** The app reaches the node directly on the local network, or through a relay (§5) with purpose `owner`. Either way, it runs TLS 1.3 with the node's key pinned and its device key as the client's, and then the same RPC contract as the wallet ([RPC](../areas/rpc.md#the-contract)). Revoking a device closes its sessions at once.
- **Consent.** An Open Payments grant that needs the owner ([Payments](../areas/payments.md#the-owners-consent)) is shown in the app, at once if it is connected, otherwise when it next connects. A pending grant still lapses as it does today.

## 7. Open Payments without a public address

Open Payments clients reach a wallet address by DNS and HTTPS, so whoever answers for the domain is trusted. A periphery node has no domain of its own to answer for. Front doors answer instead, and Dassie-aware clients check, by the node's key, that the answer came from the node.

**Front doors.**

- **Names.** A front door serves `<label>.<domain>`, where `<label>` is the node's [label](../areas/identity.md#node-label).
- **What it serves.** It terminates TLS with a wildcard certificate and passes to the node, through a relay with purpose `payments`, only the [Open Payments routes](../areas/payments.md#http-routes). It returns only JSON, within the same size limits. It never serves the wallet, nor any page the node returns, so nobody can host pages on its domain.
- **Inside the relayed stream,** it runs TLS with the node's key pinned, so the relay sees only ciphertext. The front door itself sees the requests in plain text.
- **Consent.** `GET /gnap/interact/{id}/{key}` answers with a fixed page from the front door itself, "Approve this in your Dassie app", which asks for nothing. The page checks a JSON route for the grant's state, and sends the client on to its finish URI once the owner decided (§6).
- **Anyone can run one,** and every front door reaches every node (§5). `dassiepay.net` is a default, not a requirement, and a node is not tied to it.
- **No Public Suffix List entry is needed.** The list makes each name its own site to browsers, which matters where names serve pages that run scripts or set cookies. A front door serves no page whose content a node controls (only nodes' JSON, and its own fixed consent page) and sets no cookies, so its names share only a reputation.

**The owner's own domain.** An owner can name the wallet address with a domain of their own, which can be hosted anywhere:

- `pay.example.com` is a CNAME to a front door, which serves the wallet address there with a certificate for that name;
- a TXT record at `_dassie.pay.example.com` names the node ID.

The payment pointer is then `$pay.example.com`.

**The node's names** are built ([Payments](../areas/payments.md#the-nodes-names)): a node without a hostname answers to its default front door name and the names its owner adds, each request under the name it came through, and refuses any other. What is left is the owner adding names in the wallet rather than in the node's environment, beside the TXT record that names the node.

**Checking the answer.** A node signs its Open Payments responses with its node key, and wallet address documents carry its ID in a `dassie` field ([Payments](../areas/payments.md#signed-responses)). Dassie's own client is still to check them: for an owner's domain it looks up the TXT record, with DNSSEC or DNS over HTTPS, since plain DNS answers can be forged, and checks the key against the node ID the record names. The node's chosen relays are to join the `dassie` field (§5).

**What clients that do not check trust.** An ordinary Open Payments client takes what the front door says. The incoming payment it reads includes the STREAM shared secret, and a front door whose operator also sits on the payment's path could fulfill the payment's packets itself and keep them. The node tells its owner so in plain words, naming the front door's operator, and the owner can turn off Open Payments through front doors: Dassie nodes can still pay it, with a check that leaves nothing to trust.

**Proposed for Open Payments.** Two additions would let every client check:

- **Signed responses,** by a key that the wallet address names;
- **The STREAM secret encrypted** to the client's key, which Open Payments clients already have, since they sign their requests with it and the grant records it.

They are to be proposed to the Interledger Foundation. Until they are adopted, the `dassie` fields carry the same check for Dassie's own clients.

## 8. Free receivers

A new user can receive payments without paying anything up front. A free receiver is an ordinary periphery node that peers with one uplink and has not yet paid anything.

- **No advance payment.** As junior, it extends the uplink a credit of 5 XRP, and the uplink extends it nothing ([Settlement](../areas/settlement.md#credit)). The uplink asks it to settle at once, as any senior asks its junior. Without an account on the ledger, its scheme cannot settle (`Unavailable`), so it pays nothing. Once it has been paid more than the uplink's trigger, the uplink stops asking. The node does not warn its owner about this, since it is expected.
- **Spending by netting.** What it received is what the uplink owes it. Sending through the same uplink lowers that debt, and touches no ledger. It can spend from its first payment.
- **The first settlement creates its account.** It asks the uplink to settle once the uplink owes it nearly its credit: 4.85 of 5 XRP at the default largest packet of 0.1 XRP. The uplink settles half the way to its own trigger, 2.35 XRP ([Positions and thresholds](../areas/settlement.md#positions-and-thresholds)), more than the base reserve of 1 XRP. That Payment creates the receiver's account, and the reserve comes out of the receiver's own money. Nothing asks the owner to act.
- **The uplink risks nothing.** It extends no credit, so the receiver can spend only what it was paid. It pays only the ledger's fee for the settlement.
- **The receiver risks its credit.** It can lose up to 5 XRP if its uplink disappears. The wallet says so: "*uplink* holds up to 5 XRP for you".
- **Limits.** It has one uplink, and so one address, until it has funds to peer elsewhere, and it cannot change uplinks without being settled first.

**The XRPL scheme** needs two rules for this ([XRP Ledger](../areas/xrpl.md#following-the-account)):

- **An account that does not exist yet** is synced with a balance of zero: its first activation happens before the account exists, so the Payment that creates it is booked as what it is, the uplink's settlement, by its memo and sender, not as a deposit. The reserve then goes through `equity/suspense`, as a balance below the reserve already does. A free receiver's scheme asks no faucet.
- **A settlement to an account that does not exist** is at least the base reserve plus one largest packet, or the scheme is `Unavailable` for that peer. The ledger refuses a smaller Payment that would create an account with a `tec` result, which still costs the fee, and the settlement would be tried again and again.

The first rule narrows a condition for real money, "an account's creation is a deposit" ([Real money](../roadmap.md#real-money), [XRP Ledger](../areas/xrpl.md#real-money)): it holds for every creation but one made by a peer's settlement, and its wording changes when this is built.

**Ending a free receiver's peering** follows [D28](../decisions/D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md):

- **The uplink evicts** a free receiver that has received nothing and sent nothing for 30 days. Ending the peering, the uplink pays nothing, and it still asks the receiver to settle once. A receiver whose account exists by then, and holds more than the reserve, answers as any junior does, paying half the way to its own trigger. Either way it loses at most the credit it extends. Its row stays, and counts toward the rows' limit (§4), until the receiver peers again and is settled as usual, or nothing is owed.
- **A free receiver that leaves** asks the uplink to settle once, as any node ending a peering does. Below the reserve, the uplink has nothing it can send yet, and the receiver stays owed.
- **On the uplink,** free receivers take at most half of its downlinks.

## 9. Browser nodes

A browser node is a short-lived periphery node, for demos: it lets someone join the network for a while before they install anything.

- It runs the node's core compiled to WebAssembly, as `identity-wasm` runs `dassie-identity`, with its state in the page's storage.
- It connects to one uplink over WebTransport or WebSocket (§2), as a free receiver (§8).
- It is reachable for as long as the page is open. A demo can pay it from another node, and it can send back through its uplink.

## 10. How it fits the node

- **`dassie-node`** gains a QUIC listener and client, the TCP and WebSocket fallbacks, relayed streams, and the front door's HTTP side.
- **`dassie-peer`** gains the downlink table, the `periphery` extension, records, home nodes and lookups, and the transport's link-state extension.
- **`dassie-connector`** routes beneath a downlink's child address, and a periphery node's STREAM receiver takes its child addresses.
- **The XRPL scheme** gains its table of peers' addresses and the two rules of §8.
- **The RPC contract** (`proto/dassie/ui/v1`) gains pairing and devices.
- **The apps** are a desktop app built with Tauri and a mobile app, both over the RPC contract.
- **The simulations** ([Testing](../areas/testing.md#the-simulations)) run periphery nodes that lose their uplinks, relays and home nodes that withhold, ground core keys that capture home slots, free receivers whose first settlement creates their account, free receivers evicted while owed, and downlinks over the limits.

## 11. Open questions

- **Core nodes over TCP.** Whether core nodes must also serve the TCP fallback, or may require UDP.
- **Choosing uplinks.** Which core nodes a periphery node picks by default, and whether the operator of a front door it uses counts.
- **Front doors.** Whether anyone may run one, or only core nodes. Dassie-aware clients check signatures either way.
- **A cold lookup** adds a round trip to a home node before the first stream. Caching and relays make it rare; whether that is enough is for measurement.
- **Withdrawal for room without debt.** §4 exempts downlinks that owe nothing. A core junior that has spent what it paid in advance is in the same position: withdrawing it also stops the payments that would raise its room.
