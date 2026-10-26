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

This is a design ([core-and-periphery](../decisions/core-and-periphery.md)). Of it, QUIC between core nodes is built ([Peer protocol](../areas/peer-protocol.md#quic)), and so are relayed streams and a node's chosen relay of §5 ([Peer protocol](../areas/peer-protocol.md#relayed-streams)), a node's names, its signed responses and the `dassie` field of §7 ([Payments](../areas/payments.md#the-nodes-names)), and front doors that reach the nodes which chose them as their relay (§7, [HTTP](../areas/http.md#front-doors)). So are periphery nodes, their peerings with their uplinks and their child addresses (§3), described in the area docs; the connection an uplink sends to its downlink on (§2), over which every uplink already relays to its downlinks (§5); and the downlinks' bound, their places for connections apart from core peers' and everyone else's, and their exemption from withdrawal for room (§4); and the free receivers' half of an uplink's downlinks (§8, [Network](../areas/network.md#downlinks)). The rest is not built yet ([roadmap](../roadmap.md#periphery)). Durations and sizes in it are starting points, to be set from the deterministic simulation when the parts are built.

## 1. Concepts

| Term | Meaning |
|---|---|
| **Core node** | A registered node that its peers have promoted, in the core the signed data defines ([promotion-to-core](../decisions/promotion-to-core.md)). |
| **Periphery node** | A node that connects only to core nodes and never forwards. Every node starts as one, and registers only to become core ([promotion-to-core](../decisions/promotion-to-core.md)). |
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

- **Messages.** Each request is a bidirectional QUIC stream: the request is a v1 `PeerMessage`, the response a v1 `PeerResponse`. Both have the bytes of the [peer protocol](../areas/peer-protocol.md). The [statuses](../areas/peer-protocol.md#statuses) travel ahead of the body, with the same meaning. A message is the message of the node whose key its connection proved ([Peer protocol](../areas/peer-protocol.md#authentication)).
- **Either side opens streams**, as built ([Peer protocol](../areas/peer-protocol.md#quic)): the core node sends to its downlink over the connection the downlink opened. That is all a periphery node needs to receive: it never needs to accept a connection.
- **Keys, not certificates.** The TLS 1.3 handshake inside QUIC uses a self-signed certificate for each side's Ed25519 node key, and each side checks that key against the node ID it expects. No certificate authority is involved, and no hostname has to match. The ALPN is `dassie/2` ([Peer protocol](../areas/peer-protocol.md#versions)). A client that is not a node (an owner's app, a front door) presents a key of its own (§6, §7).
- **One connection per pair of nodes.** When two core nodes dial each other at once, the connection the lower node ID opened stays, as crossed peering requests resolve ([Network](../areas/network.md#asking-and-answering)).
- **Liveness.** Heartbeats ([Network](../areas/network.md#advertised-peers)) also keep the NAT's mapping open. A periphery node that loses its connection dials again, backing off from 1 s to 60 s.
- **Where UDP is blocked,** a periphery node carries the same QUIC packets over a WebSocket at `/quic` on the core node's HTTPS listener, one packet per binary message, so the connection inside, its streams and its places are QUIC's as over UDP ([quic-over-websocket](../decisions/quic-over-websocket.md)). Every core node serves it at its URL's host and port. A periphery node dials over UDP first, and the tunnel too once the handshake has not completed within a head start; the first connection to prove the key is kept. It does not check the outer certificate, since QUIC inside pins the node key, so a proxy that intercepts TLS does not stop it.
- **Link-state URLs** keep their syntax, `https://host[:port]` ([Peer protocol](../areas/peer-protocol.md#link-states)). The host and port then name the QUIC endpoint, on UDP, 443 by default. Every node speaks QUIC, so no link state says so ([quic-only](../decisions/quic-only.md)).

## 3. Periphery nodes

Built: who is a periphery node, its link state without a URL, its peerings with its uplinks without a probe, its routes, its child addresses and that it forwards nothing are in [Network](../areas/network.md#periphery-nodes) and [Connector](../areas/connector.md#a-periphery-node).

## 4. Downlinks on a core node

A core node keeps its peerings with other core nodes, and its downlinks, under separate limits.

- **Core relationships:** at most 32 settlement channels open with core nodes, as against 512 today. A channel outlives its peering, and closes once idle, to disk, which frees its place ([Settlement](../areas/settlement.md#settlement-channels)). Its ledger accounts close to disk with it, with what they show, and come back with it ([resident-peers](../decisions/resident-peers.md)).
- **Downlinks** live in a table of their own, not in the node table. Each row holds:
  - the downlink's node ID and public key;
  - its scheme, and the scheme's data for it (on the XRP Ledger, its account address);
  - the credit we extend it;
  - when it was last heard from;
  - what was settled each way, and the settlement message on its way, if any.

  With the downlink's two ledger accounts, that is about 0.5 KB per downlink on disk. A peer today costs about 1 KB on disk and 1.5–2 KB in memory, most of it the link state, kept both as bytes and parsed, which a downlink does not need.
- **In memory only while connected** ([resident-peers](../decisions/resident-peers.md)). A downlink's peering, settlement channel and ledger accounts are read in when its connection proves its key, and dropped once it has no connection and nothing still needs them: no transfer of its pending, no settlement of ours on its way, no hold, no termination under way. Everything is on disk already, so dropping writes nothing. Its timers run from the disk while it is away.
- **Limits:** at most 10 000 downlinks peered at once, and at most 100 000 rows, about 50 MB. Memory holds only the connected ones and those something still needs, bounded by the downlinks' open channels, and for each other one with an open channel a small entry: its node ID, whether it paid us and whether it is peered. A row with no peering, idle as a closed channel is, is closed. When the rows are full, the node refuses new downlinks. What is built is one bound on downlinks, which live in the node table, and whose channels close once idle, as every channel does ([Network](../areas/network.md#downlinks)); their QUIC connections hold places of their own, with room for that many downlinks ([Peer protocol](../areas/peer-protocol.md#quic)).
- **No withdrawal for room** for a downlink that owes nothing is built ([Network](../areas/network.md#advertised-peers)).
- **The XRPL scheme** keeps its peers' addresses in a table keyed by account, not in its state ([XRP Ledger](../areas/xrpl.md#state-and-restarts)). With many downlinks, its state would grow with every one and be written whole at every checkpoint, and a scheme that kept only the newest addresses, or only those of downlinks in memory, could not recognise a settlement from a downlink it forgot.

## 5. Reaching a periphery node

A periphery node cannot be dialled. Everything that needs to reach it (its owner's app, a front door with an Open Payments request) goes through a relay: a server it keeps a connection to.

**Relays and relayed streams** are built ([Peer protocol](../areas/peer-protocol.md#relayed-streams)): every QUIC endpoint relays streams to the nodes connected to it, never dialling them, with TLS end to end between client and node and bounds per destination and per client; and a node keeps a connection to a relay it chooses, without peering with it, by default the `dassiepay.net` front door while it has no hostname ([Node](../areas/node.md#a-chosen-relay), [default-relay](../decisions/default-relay.md)). That relay is the node's fast path, and it keeps the node reachable before it has any peering, or while lookups fail. Every uplink is a relay for its downlinks too, since they are connected anyway (§4): that is built with downlinks. What is left is how clients find the relays that reach a node, below.

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

**Home nodes.** Every node computes the same core from the same signed data ([promotion-to-core](../decisions/promotion-to-core.md)); the majority node list alone is not the core, since it also holds registered nodes not yet promoted. A periphery node's home nodes are the five core nodes that rank highest by `SHA-256("dassie home" ‖ periphery ID ‖ core ID)`. Anyone with the same signed data computes the same five, so a lookup is a single request, with no routing through a distributed table.

- **Storing.** An uplink stores its downlink's record with the downlink's home nodes, as an authenticated peer message, when the downlink connects and whenever the downlink renews it. A home node takes a record only from a core node the record names as an uplink, and a record is about 200 bytes.
- **Sharing a home node.** A core node can name any number of keys it made itself as its downlinks, since keys cost nothing. So a home node bounds its records in all, and gives each submitting core node a fair share of them: when it is full, a new record displaces the oldest of the core node that holds the most, never one of a node within its share. A few registered core nodes can then crowd out only each other, not the records of every other uplink.
- **Forging.** A record is signed by its node, so a home node can withhold it but not forge it. A newer sequence replaces an older one.
- **Looking up.** A node or front door asks one of the five home nodes, and the next if it has no answer. It caches a record until it expires.
- **When the list changes,** a periphery node's home nodes may change; its uplinks store the record with the new ones at the next renewal.

**Reaching a node,** a front door or an owner's app tries, in order:

1. its own connection to the node, if the node chose it as relay;
2. the relays the node names in its wallet address document (§7), if it has fetched it;
3. a lookup at the node's home nodes, then one of the uplinks or relays from the record.

**Grinding.** An attacker who wants to hide a node can generate core node keys until five of them rank highest for its ID. Each must register, stay in the majority and be promoted into the core ([promotion-to-core](../decisions/promotion-to-core.md)), under the limits of [registration](../areas/network.md#registration) and, later, its fee. It can only withhold the record, and a relay the node chose still reaches it.

## 6. Owner apps

The wallet is never served through a relay or a front door. The owner enters the passphrase into the wallet's page to log in ([Wallet](../areas/wallet.md#key-derivation)), so a page served by anyone else could keep it, and a passphrase cannot be revoked. A periphery node's owner uses the wallet on `localhost`, as the [desktop app](../areas/desktop.md) does for the node it runs, or an app paired with a node on another machine: a desktop or a mobile app.

- **The node serves the wallet, `/api/*` and `/rpc` only on its own listeners.** On a relayed stream it speaks only the owner protocol below, or Open Payments ([HTTP](../areas/http.md#relayed-streams)).
- **Pairing and calls are built** ([RPC](../areas/rpc.md#paired-devices)): the owner on the node's machine starts a pairing, the app makes a device key (Ed25519) and claims it on a stream with purpose `owner`, directly or through a relay (§5), with TLS pinned to the node's key and proving the device key, and then calls the wallet's RPC until the owner revokes it. What is left is the apps themselves, and finding the node from the local network without its URL or a relay in the code.
- **Consent.** An Open Payments grant that needs the owner ([Payments](../areas/payments.md#the-owners-consent)) is shown in the app, at once if it is connected, otherwise when it next connects. A pending grant still lapses as it does today.

## 7. Open Payments without a public address

Open Payments clients reach a wallet address by DNS and HTTPS, so whoever answers for the domain is trusted. A periphery node has no domain of its own to answer for. Front doors answer instead, and Dassie-aware clients check, by the node's key, that the answer came from the node.

**Front doors** are built ([HTTP](../areas/http.md#front-doors)): a front door serves `<node ID>.<domain>` with a wildcard certificate, passes Open Payments alone to the node the ID names, over a relayed stream it opens on the connection the node keeps to it, and returns only JSON, signed by the node, under a policy that lets nothing run, but for a static page of its own at a name's root that says what the name is. It never serves the wallet, nor any page a node returns, and sets no cookies, so its names need no Public Suffix List entry: they share only a reputation. What is left:

- **Consent.** `GET /gnap/interact/{id}/{key}` is to answer with a fixed page from the front door itself, "Approve this in your Dassie app", which asks for nothing. The page checks a JSON route for the grant's state, and sends the client on to its finish URI once the owner decided (§6). Until owner apps exist, the front door answers it with a fixed error.
- **Every front door reaches every node** (§5), through the relays a node's record names; a built front door reaches only the nodes that chose it as their relay. `dassiepay.net` is a default, not a requirement, and a node is not tied to it.

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

**Superseded in part.** Peers settle only over hubs ([hubs-only](../decisions/hubs-only.md)), so a free receiver is paid into its uplink's hub: the uplink's `pay` opens its hub account once the uplink owes it the hub's minimum for a new account ([roadmap](../roadmap.md#joining-by-being-paid)). The direct settlement on the XRP Ledger below, the credit it extends as a junior, the first settlement that creates its XRPL account and the XRPL scheme's two rules for it, are superseded and not to be built. The rest of this section, spending by netting, its limits, ending its peering and the free half of the downlinks, still holds, read with the hub in place of the XRP Ledger.

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

**Ending a free receiver's peering** follows [ending-a-peering](../decisions/ending-a-peering.md):

- **The uplink evicts** a free receiver that has received nothing and sent nothing for 30 days. Ending the peering, the uplink pays nothing, and it still asks the receiver to settle once. A receiver whose account exists by then, and holds more than the reserve, answers as any junior does, paying half the way to its own trigger. Either way it loses at most the credit it extends. Its row stays, and counts toward the rows' limit (§4), until the receiver peers again and is settled as usual, or nothing is owed.
- **A free receiver that leaves** asks the uplink to settle once, as any node ending a peering does. Below the reserve, the uplink has nothing it can send yet, and the receiver stays owed.
- **On the uplink,** free receivers take at most half of its downlinks. This is built ([Network](../areas/network.md#downlinks), [free-receivers-half](../decisions/free-receivers-half.md)): a downlink is a free receiver until the uplink's books take a settlement of at least one largest packet from it, and stays a paying one after. Once free receivers hold half of the downlinks' places, a new one is refused; one that paid is bounded by the downlinks' bound alone, and no downlink is ended or displaced for the rule. So keys made for nothing fill only the free half. A newcomer gets in past a full free half by paying the uplink one largest packet on the ledger before it asks, which the uplink holds for it and books as its settlement as it peers ([pay-to-enter](../decisions/pay-to-enter.md)); the uplink's side is built, the newcomer's is not.

## 9. Browser nodes

A browser node is a short-lived periphery node, for demos: it lets someone join the network for a while before they install anything.

- It runs the node's core compiled to WebAssembly, as `@dassie/wasm` runs `dassie-identity`, with its state in the page's storage.
- It connects to one uplink by QUIC over a WebSocket (§2), with quinn's protocol state machine compiled to WebAssembly, as a free receiver (§8). The browser checks the outer certificate, so its uplink needs a publicly issued one. A browser sends an origin with the handshake, which `/quic` refuses so that no page can have its visitors hold a node's places ([quic-over-websocket](../decisions/quic-over-websocket.md)): browser nodes need a rule of their own, such as a path that takes them only from the origins of the pages that run them, with places of their own.
- It is reachable for as long as the page is open. A demo can pay it from another node, and it can send back through its uplink.

## 10. How it fits the node

- **`dassie-node`**: its QUIC endpoint, QUIC over a WebSocket, relayed streams, signed Open Payments answers and front doors, which serve the WebSocket too, are built.
- **`dassie-peer`** gains the downlink table, the `periphery` extension, records, home nodes and lookups, and the transport's link-state extension.
- **`dassie-connector`** routes beneath a downlink's child address, and a periphery node's STREAM receiver takes its child addresses.
- **The XRPL scheme** gains its table of peers' addresses and the two rules of §8, which [hubs-only](../decisions/hubs-only.md) supersedes: a free receiver is paid into its uplink's hub instead ([roadmap](../roadmap.md#joining-by-being-paid)).
- **The RPC contract** (`proto/dassie/owner/v1`) gains pairing and devices.
- **The apps** paired with a node are a desktop app and a mobile app, both over the RPC contract. The [desktop app](../areas/desktop.md) that runs its own node is built.
- **The simulations** ([Testing](../areas/testing.md#the-simulations)) run periphery nodes that lose their uplinks, relays and home nodes that withhold, ground core keys that capture home slots, free receivers paid into their uplink's hub (their first settlement creating their XRPL account is superseded by [hubs-only](../decisions/hubs-only.md)), free receivers evicted while owed, and downlinks over the limits.

## 11. Open questions

- **Choosing uplinks.** Which core nodes a periphery node picks by default, and whether the operator of a front door it uses counts.
- **Front doors.** Whether anyone may run one, or only core nodes. Dassie-aware clients check signatures either way.
- **A cold lookup** adds a round trip to a home node before the first stream. Caching and relays make it rare; whether that is enough is for measurement.
- **Withdrawal for room without debt.** §4 exempts downlinks that owe nothing. A core junior that has spent what it paid in advance is in the same position: withdrawing it also stops the payments that would raise its room.
