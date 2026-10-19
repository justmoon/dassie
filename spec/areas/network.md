# Network

How Dassie nodes find each other and stay connected: the node table, link states and how they spread, network membership through bootstrap nodes, registration, peering and its lifecycle, bans, and routing. The messages these use are in [Peer protocol](peer-protocol.md).

**Paths:** `crates/dassie-peer/src/node/`, `crates/dassie-peer/src/table.rs`, `crates/dassie-peer/src/routing.rs`, `crates/dassie-peer/src/channel.rs`, `crates/dassie-peer/src/persist.rs`, `crates/dassie-peer/src/config.rs`, `crates/dassie-peer/src/hooks.rs`, `crates/dassie-node/src/env.rs` (bootstrap list)
**Read with:** [Peer protocol](peer-protocol.md), [Settlement](settlement.md), [Connector](connector.md), [Identity](identity.md), [Node](node.md)

## The peer node

`dassie-peer`'s `PeerNode` is a sans-IO state machine. It takes inbound requests, the results of its own requests and the passing of time, and gives back replies, requests to send, events, routes and what to persist. It reads no clock, RNG or socket of its own; `dassie-node` drives it and writes its changes in the same transaction as the ledger's ([Node](node.md)).

- A node wakes only when a timer is due: the refresh waits for a node to go stale, and the peering check for a candidate while the node wants a peer, rather than looking every second.
- Every table is bounded ([Timers and limits](#timers-and-limits)).
- A node's ILP address is `<scheme>.das.<nodeId>`, where the allocation scheme is `test` or `g` by the node's realm. A peer is a connector endpoint at its own such address, which ILDCP tells it ([Connector](connector.md)).

## Node table

The table holds what a node knows about other nodes: per node, its key, its newest link state, why it is in the table, its registration with us, our peering with it, and the timers for refreshing and peering with it.

**Sources.** A node is in the table for one or more reasons (NET-29):

| Source | Meaning | Rank |
|---|---|---|
| self | this node | 5 |
| bootstrap | a configured bootstrap node | 4 |
| peer | peered with us, or being asked to peer | 3 |
| majority | on a majority of the bootstrap nodes' lists | 2 |
| registered | registered with us (on a bootstrap node) | 1 |

- A node enters the table only through one of these. A link state of a node not in the table is dropped.
- The table holds at most 1024 nodes. When it is full, a newcomer takes the place of the node whose strongest reason ranks lowest, if that is below the newcomer's reason; among equals, the one that came in last goes. Otherwise the newcomer is not added (a registration gets 503).
- **Removal** (NET-18): a node is removed 1 h after its last reason to be there is gone. Its link state goes with it. A removed node that still has a majority is added again.

## Link states

### The own link state

- A node without a valid URL issues no link state ([Peer protocol](peer-protocol.md)).
- It is issued at start, again when its neighbors, schemes, URL or alias change (NET-6), and every hour regardless (NET-5). It is never issued more than once per 5 s; a change waits for the rest of that period.
- **Sequence:** `max(now, last + 1)`, with the last value persisted (NET-7). If the persisted value is more than 10 min ahead of the clock, which no node with a right clock accepted, the sequence starts again from `now`.
- **Neighbors** are the advertised peers ([Lifecycle](#lifecycle)), in node-ID order. **Schemes** are the active settlement schemes, in the owner's order.
- A new own link state is flooded to peers at once, and sent directly to the bootstrap nodes we are not peered with, so they can answer refreshes for it (NET-20).

### Receiving a link state

A link state from `linkStateUpdate`, a `linkStateRequest` answer, a registration or a peering request is taken in this order, so that floods are cheap:

1. A state of this node itself is an echo and is ignored.
2. A state of a node not in the table is dropped.
3. A byte-identical copy of the state held counts toward the flooding suppression and is dropped. A state with an older or equal sequence is dropped; other copies with the same sequence do not count.
4. A state whose sequence is more than 10 min ahead of the receiver's clock is dropped (in a registration or a peering request it is refused with 400).
5. **Rate:** each subject may issue at most 8 states in a burst, then one per 5 s. The excess is neither stored nor forwarded. The rate is checked before the signature, but a token is spent only on a state that verifies, so forged states cannot use up the real node's allowance.
6. The state is verified ([Peer protocol](peer-protocol.md)). One that fails is dropped (a `linkStateUpdate` gets 400).
7. It replaces the state held, with the node's key, and is scheduled for flooding if it came by flooding.

**Freshness.** A link state is fresh while its sequence is within 3 h before now and 10 min after. Routing uses only fresh states, and stale ones are refreshed.

### Flooding

- A state taken from a `linkStateUpdate` is forwarded after a random delay of 1 to 500 ms to every peer except its subject and the peer it came from (NET-35).
- It is not forwarded if 3 or more copies of it, the first included, arrived before the delay ran out.
- Our own new state goes at once. States learned from refreshes, sweeps, registrations and peering requests are not forwarded.

### Heartbeats

- Every 5 to 20 s (uniformly random), a node sends its current link state, authenticated, to every peer it is not terminating with, advertised or not, so both sides can confirm each other.
- A heartbeat goes at once when a peering forms.
- On the receiving side, an authenticated `linkStateUpdate` whose sender is the subject of the state counts as a heartbeat. In fact any authenticated message from a peer, or authenticated response to one of our requests, marks the peer as heard from (NET-12), except from a banned node.

### Refresh

- Once per second, a node picks one node at random whose link state is missing or stale, and whose refresh is not backing off.
- It asks that node itself, at the URL of its last state. With probability 20%, or when it has no state of the node, it asks instead a random node with a fresh state, or a bootstrap node; such a request also carries up to 15 more stale nodes, 16 IDs in all.
- Each node asked about that does not come back with a newer state (old, dropped, invalid or missing) backs off: `10 s · 2^n` after its n-th failure in a row, at most 1 h ([Timers and limits](#timers-and-limits)).
- Only states of the nodes asked about are taken (NET-36).

### Anti-entropy sweep

Every 10 s, whether or not any link state is stale, a node asks a random peer (or, if it has none, a random bootstrap node other than itself) for the link states of the next 16 nodes in its table, in node-ID order, round and round. The node asked is left out of its own request. Whatever is newer replaces what is held; nothing else is done with the answer. Flooding alone loses an update for up to an hour when a message is lost, a node is down or the network splits; the sweep repairs it within one pass of the table.

## Membership

The network's members are the nodes that a majority of the bootstrap nodes list. Bootstrap nodes take registrations and serve lists of the nodes registered with them.

### Bootstrap nodes

- The bootstrap node list (BNL) is `DASSIE_BOOTSTRAP_NODES`, a JSON array of `{"id", "url", "publicKey"}` (`publicKey` in base64url). At most 64 entries. An entry whose URL is not a valid node URL, whose key does not decode, or whose ID is not the key's node ID stops the node at startup (NET-19), as does a node listed twice (by ID or URL).
- The default BNL is the public testnet's three nodes ([D15](../decisions/D15-bootstrap-network-for-v1.md)), each in its own region:

| Node | ID | Public key |
|---|---|---|
| `https://boot1.dassie.xyz` | `dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk` | `w25V-7IreZNDCPvwCrpiIv9Li5uwomzPSvqHot9ffLI` |
| `https://boot2.dassie.xyz` | `dE0GE90-9QCQI78S2_h5QaG81oo_cVKz1` | `s9xgWff_UZ26HQwHT_aPd0q0SwyKPGXS74fd2xQjh14` |
| `https://boot3.dassie.xyz` | `dtTay2e6AV4J940nFJ6_WPkVL-TyrVKGW` | `--P-gYMJ_ZZrFHa4GiNfVQ0x2jSu9_dr7z0mI_bcngY` |

- `DASSIE_BOOTSTRAP_NODES` replaces the list. `[]` makes a node run alone: it warns at startup and neither polls nor registers. Tests and the dev environment set their own list, so they stay off the testnet.
- Bootstrap nodes are always in the table, with their configured key, and always keep their replay state.
- A node takes registrations only when it is one of its own bootstrap nodes. So each testnet bootstrap node lists all three in its own `DASSIE_BOOTSTRAP_NODES`, itself included, through a systemd drop-in, which stays even though the default names the same three ([D15](../decisions/D15-bootstrap-network-for-v1.md)). Each runs `xrpl-testnet`, so a new node has someone to peer with.

### Node lists

- Every 60 s, a node asks each bootstrap node for its list's hash (`nodeListHashRequest`, 5 s timeout). If the hash differs from the one stored with the list it holds, it downloads the list (`nodeListRequest`); otherwise the list counts as fetched now.
- A downloaded list is accepted when its response is authenticated by the configured key, and is stored with the hash of the bytes received (NET-15).
- A bootstrap node's own list counts as fetched at every poll, without asking.
- **Served list:** a bootstrap node serves `unexpired registrations ∪ itself`, sorted by bytes (NET-32), at most 4096 IDs. It lists itself and registers with the other bootstrap nodes like any node. (It does not serve the majority back, which would keep a node listed forever once it had made it.)

### Majority

- A node is in the majority when strictly more than half of the *configured* bootstrap nodes list it (`votes · 2 > N`). With three bootstrap nodes that is two.
- A list not fetched for 24 h no longer votes.
- Majority nodes are added to the table. They lose their place only while more than half of the configured lists have been fetched within the last 10 min; otherwise membership freezes, so bootstrap nodes going offline do not empty every table in the network.
- The majority set survives a restart (the `majority` source is persisted).

### Registration

**Client side.** A node with a link state registers with every bootstrap node other than itself:

- once at start;
- again every 6 h, before its registration (24 h) lapses;
- again 5 list-poll periods after registering if that bootstrap node's list still does not have it;
- after a failure, again after `10 s · 2^n` for its n-th failure in a row, at most 1 h.

The owner can register with a bootstrap node at once ([RPC](rpc.md)). The answer says when the registration expires.

**Bootstrap side.** Registration is free and throttled; registration fees are not built ([roadmap](../roadmap.md#registration-fees-and-child-peers), [D4](../decisions/D04-network-membership-economics.md)):

- a registration lasts 24 h from its last renewal;
- new registrations are throttled globally: a bucket of 20, refilled at 60 per hour; a token is spent only when a registration succeeds, not on one whose probe fails;
- at most one registration per URL host and port (443 counts as no port), and at most 8 per /24 (IPv4) or /48 (IPv6) of the address the registration came from;
- at most 1024 registrations;
- before accepting a new registration, or one whose URL changed, the bootstrap node probes that URL: it asks it for the registrant's link state, with authentication, and must get back a verified state of that node at the registered sequence or later;
- a renewal is a registration from the URL that was probed, and is answered at once; one from another URL is probed again;
- an expired registration is dropped, and the node leaves the served list.

**Probes.** Registration and peering probe the requester's URL the same way:

- a `linkStateRequest` for the requester itself, authenticated, with a 5 s timeout;
- a probe of the same node or host is never made twice at once;
- probes are rate-limited: a bucket of 32, refilled at one per second;
- at most 32 inbound requests wait on probes; more get 503.

### How a node joins

1. It registers with each bootstrap node, which probes its URL and lists it.
2. Other nodes poll the lists; once a majority lists it, they add it to their tables.
3. They learn its link state from refreshes, sweeps or flooding.
4. Nodes that peer with it advertise it as a neighbor, which makes it routable.

## Peering

Each node looks for a few peers on its own (auto-peering). A peering is between two nodes on one settlement scheme, at one version of that scheme.

### Looking for peers

- **Target:** 2 peers. Only advertised peers count toward it ([Lifecycle](#lifecycle)), so a node whose peers went silent or flap looks for others.
- **Candidates:** nodes in the table, other than ourselves, with a fresh link state that lists a scheme we have active, not peered or being asked, not banned, not in backoff, and with room for a settlement channel. The scheme is the first one in the candidate's order that we also have.
- Candidates that our routes do not reach come first, then majority nodes; the RNG picks among the best.
- One attempt at a time, and only with a link state of our own and an active scheme. The check runs every second while the node wants a peer.
- After a restart with peers, the node waits 20 s (a heartbeat's time) before looking, so the peers it has can confirm themselves first.
- **Slot caps:** a node starts at most target + 2 = 4 outbound peerings, and holds at most 8 peerings. Inbound requests are accepted while there are fewer than 6 inbound peers, so requests alone cannot fill every slot.
- **Bridge rule:** every 60 s, if a majority node with a fresh link state has been unreachable by our routes for 2 min, the node starts one more outbound peering, with such a node, if it can. This joins separate islands of peers.

### Asking

1. The node marks the candidate as requesting and sends `peeringInfoRequest` for the scheme.
2. Its scheme builds the request data from the answer. The node sends `peeringRequest` with its own signed link state, the scheme's data, the scheme versions it speaks and `credit` 0.
3. On `accepted`, if it still has room and speaks the version named, its scheme finalizes the peering and opens the peer's accounts. The node is peered.
4. **Backoff** (NET-22): a failed attempt backs off from that node, `10 s · 2^n` after its n-th failure in a row, at most 1 h. A refusal for "unknown node" is retried after one list-poll period instead, and one for "concurrent request" after 10 s.

### Answering

The responder checks a `peeringRequest` in this order ([Peer protocol](peer-protocol.md) has the reasons):

1. the embedded link state names the sender, verifies, and is not from the future (400 otherwise);
2. the sender is not banned (8);
3. the scheme is active and has versions (2);
4. the sender is in the table (1);
5. we are not terminating with it, and no owner ended our peering with it in the last 24 h (9);
6. there is room for its settlement channel (4);
7. a peering with it already exists: on the same scheme, the request is a repeat; on another, it is refused (0);
8. a version is common to both (3);
9. if we are asking it too, the request from the lower node ID wins: ours (5), or we drop ours and go on;
10. there is an inbound slot and a peer slot (4).

Then:

- the responder takes in the requester's link state, and probes its URL as for registration; if it fails, the answer is "unreachable" (7);
- after the probe it checks again, and its scheme accepts or refuses (6), opening the peer's accounts;
- the peering is persisted before the acceptance is sent, and so before the requester uses it.

**Repeats and crossings.**

- A request from a node that is already peered on the same scheme (`peered`, not terminating) is answered at once, without a probe, with `accepted`, the stored peering's version and empty data. The peering does not change. This covers an acceptance that got lost.
- Crossed requests resolve in favour of the lower node ID (node IDs compare by bytes).

### The scheme's say

Peering asks the node's settlement schemes ([Settlement](settlement.md)), through `PeeringHooks`:

- which versions of a scheme it speaks (none: not active);
- its peering info, for `peeringInfoRequest`;
- the data for a request, from the peer's info;
- to accept a request, opening the peer's accounts and giving the response data (if a peering with the node formed while the probe was out, it is called again and changes nothing);
- to finalize our accepted request, opening the peer's accounts;
- whether nothing is in flight with a peer any more, in either direction.

### Roles

- The **junior** is the node whose peering request was accepted, the **senior** the node that accepted it. When requests cross, the lower node ID's request wins, so the lower ID is the junior.
- The junior extends the senior a default credit line, and the senior extends the junior none, at first. Credit then grows with what a peer earned us. The amounts, the thresholds and when peers settle are in [Settlement](settlement.md#credit) ([D20](../decisions/D20-trust-follows-what-a-peer-earned-us.md)).

## Lifecycle

A peering is `requesting`, `peered` or `terminating` ([D12](../decisions/D12-peering-lifecycle-and-liveness.md)). With no peering, a node is in state none. "Peered" in other documents covers `peered` and `terminating`; "active" means `peered`.

### Advertised peers

- A peer is **confirmed** while an authenticated message from it arrived within the last 60 s (`peer_confirm_ms`); its heartbeats keep it so. A banned node's messages do not confirm it.
- A peer is listed as a neighbor in our link state while it is `peered`, confirmed, not banned, and not held down.
- Each time it stops being advertised, it is held down for `30 s · 2^flaps`, at most 1 h. `flaps` resets after 1 h advertised without a break.
- A peer heard from again after its hold-down is listed at once.
- After a restart, the flag starts from the persisted time the peer was last heard from, without a hold-down: a restart does not drop our neighbors.
- **Withdrawn for room** ([D12](../decisions/D12-peering-lifecycle-and-liveness.md#refinement-a-peer-that-stays-short-of-room-2026-10-19)): a peer whose position, the room our books give it, has stayed below one of the largest packets for 2 min without settling is not advertised either.
  - The node counts it at every settlement check while its scheme sees its ledger, so that it can ask. Each settlement booked from the peer starts the 2 min again. That is longer than a routine settlement takes, so only a peer that is stuck is withdrawn.
  - Such a withdrawal is a flap like any other. The peer stays peered and is still asked to settle, but no packet crosses the edge either way, since both ends' routes drop it. Once its room is back, it is advertised again after its hold-down.
  - A node does not start with a grace shorter than two settlement request intervals. When it has been short since is not persisted.

### Termination

Termination starts when the owner asks, when the owner bans the node, when the peer has not been heard from for 24 h, counted from the later of the last message and the node's start (so a node that was down ends nothing for it), or when its room has stayed short of a packet, without a settlement, for 24 h (reason 4, [D12](../decisions/D12-peering-lifecycle-and-liveness.md#refinement-a-peer-that-stays-short-of-room-2026-10-19)).

1. The peer is no longer advertised, and takes no new Prepares to or from it: the connector skips it, and its Prepares get 409. Responses still flow.
2. The side that started sends `peeringTermination`, again 10 s after each failed try, until a 200 or a 409 arrives or 2 min have passed. The other side answers at once and terminates too. If both started, each one's message tells the other.
3. Once the exchange is over (acknowledged, or given up), no packet is in flight with the peer, and no response to it is waiting to be sent again, the peering ends. Its accounts stay, with their balances: they are the record of what is owed, and a later peering with the node uses them again without posting trust a second time. Its settlement channel stays too.
4. If the exchange of step 2 completed, each side then settles everything it owes, kept on the settlement channel across restarts until it is made ([Settlement](settlement.md#settling-at-a-peerings-end)). An unresponsive peer is not settled with; the debt stays on the books.
5. The node does not ask the peer again for 24 h after an owner's termination (a ban included), or after its usual peering backoff otherwise. A request from a node we are terminating with, or whose peering an owner ended in the last 24 h, is refused with reason 9. That cooldown does not survive a restart; a ban is the lasting way.

A termination under way survives a restart: a node still telling its peer goes on telling it.

### One-sided peerings

A node that holds no peering with a sender, and is not asking for one, answers an authenticated heartbeat that lists it as a neighbor with a `peeringTermination` of reason 3, at most once per 2 min (the termination timeout) for each such node. The sender must be in the table. The other side then ends its peering.

### Bans

- Bans are a list the owner keeps, of at most 1024 node IDs ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). Only the owner bans; a node never bans a peer for what it cost. A node cannot ban itself.
- A banned node is never asked, is refused with reason 8, and its messages do not confirm it as a peer. Its link states still flood.
- A ban ends a peering, and drops our pending request, as the owner's termination does, with the same wire reason (1).
- Lifting a ban only removes it from the list.
- Bans that older versions made automatically, for a grace limit, are lifted when the node upgrades.

## Settlement channels

Every peer gets a settlement channel, which belongs to the pair of nodes and outlives the peering ([Settlement](settlement.md#settlement-channels)). A node we hold one with keeps its replay state ([Peer protocol](peer-protocol.md#replay-protection)), and its Fulfills, Rejects and `settlement` messages are taken even with no peering; a `settlement` from any other node gets 409. Once all 512 channels are taken, a node without one is no candidate, and its peering requests are refused for capacity (reason 4).

## Routing

Routing runs a breadth-first search from this node over edges that both ends advertise and whose link states are fresh.

- An edge counts only if both ends list each other, so no node can claim a link the other end does not confirm. A node whose state went stale drops out.
- The route for each reachable node `<scheme>.das.<id>` lists up to 4 first hops on shortest paths, sorted, with the distance in hops.
- There is no route for the node's own address (NET-34). Destinations under our own address go only to our own endpoints, or get F02.
- A packet is never forwarded back to the endpoint it came from. Prefix matching and first-hop choice are the connector's ([Connector](connector.md)).
- Routes are recomputed when the graph may have changed, at most once per 250 ms, and at least once a minute as states go stale.
- Child (non-majority) peers routed beneath their uplink's address are not built ([roadmap](../roadmap.md#registration-fees-and-child-peers)).

## Persistence

Written in the same transaction as the ledger, on every change (NET-1, NET-2, ACC-18):

| Data | Kept |
|---|---|
| Nodes | ID, link state (the signed bytes, verified when received), sources, when it came in, registration (registered, renewed, expires, probed URL, network) |
| Peerings | scheme, version, the scheme's state for the peer, the credit we extend, since when, whether inbound, when last heard from, the replay floor |
| Terminations | since when, reason, whether we began it, whether acknowledged |
| Own link-state sequence | the highest issued (NET-7) |
| Replay state | per peer, the newest accepted message timestamp, written with the batch it took effect in; after a restart, the later of it and the start time is the floor |
| Settlement channels | as in [Settlement](settlement.md#settlement-channels); never removed, kept when the node leaves the table |
| Bans | the node IDs |

- On restart, link states are decoded, not verified again. A peering attempt cut short by the restart is dropped. A node no longer configured as a bootstrap node loses that source. Our own link state is issued anew.
- Not persisted: link-state flood counters, refresh and peering backoffs, the owner-termination cooldown, advertise hold-downs, bootstrap nodes' lists and hashes (fetched again), and replay state of non-peers.

## Timers and limits

All of these are configuration. The values in brackets are the dev settings (`DASSIE_DEV_TIMINGS`, [Node](node.md)), for what runs on a real clock: the dev environment and the end-to-end tests. The rest are the same in dev, and the simulations run the production values ([Testing](testing.md#deterministic-simulation)).

| What | Value |
|---|---|
| Hash poll and list download | 60 s (1 s) |
| List fresh, for the membership freeze | 10 min (10 s) |
| List stops voting | 24 h |
| Peer message timeout | 30 s; 5 s for hash polls and probes; a packet's own expiry if sooner |
| Heartbeat | 5–20 s, and once at once when a peering forms |
| Link-state re-issue | 1 h, debounced to one per 5 s (0.5 s) |
| Link state ahead of the clock | at most 10 min |
| Link-state staleness | 3 h |
| Link states per subject | 8 at once, then one per 5 s |
| Flood delay, suppression | 1–500 ms, after 3 copies |
| Refresh | one stale node per 1 s, 20% indirect, up to 16 IDs per request |
| Anti-entropy sweep | 16 nodes per 10 s (1 s) |
| Backoff of refreshes, registrations and peering attempts | `10 s · 2^n` after the n-th failure in a row, at most 1 h (`1 s · 2^n`, at most 30 s) |
| Registration lifetime and renewal | 24 h and 6 h (2 min and 30 s) |
| New registrations | 20 at once, 60 per hour |
| Registrations per /24 or /48 | 8 |
| Probes | 32 at once, one per second; 32 requests waiting |
| Removal grace | 1 h (10 s) |
| Peer target, most peers, inbound, outbound | 2, 8, 6, 4 |
| One-sided peering told again after | 2 min (20 s) |
| Peering check, bridge check | 1 s, 60 s (2 s) |
| Unreachable before a bridge | 2 min (4 s) |
| Peer confirmed by an authenticated message within ([confirmed](#advertised-peers)) | 60 s |
| Advertise hold-down | 30 s doubling to 1 h; reset after 1 h stable (5 s to 1 min; 5 min) |
| Withdrawn when short of room for | 2 min without a settlement (10 s); ended after 24 h (2 min) |
| Peer unresponsive after | 24 h (2 min) |
| Termination message retries | 10 s after each failed try, for 2 min (1 s, for 20 s) |
| No new peering after an owner's termination | 24 h (5 min) |
| Route recomputation | at most every 250 ms, at least every 60 s |
| Nodes, registrations, bans | 1024 each |
| Settlement channels | 512 |
| Own requests in flight | 8192 |
| Request body | 64 KiB |

The gossip parameters (suppression after 3 copies, 1–500 ms forward jitter, 5–20 s heartbeats, 3 h staleness, 1 h re-issue) are the original's, kept on purpose. The settlement timers are in [Settlement](settlement.md).

## Tests

The `peer` simulation (`dassie_peer::sim::chaos`) runs this protocol on a rough network with crashes, partitions, terminations and bans, then checks that link states and routes converge, peerings are two-sided, every termination finishes, and each settlement is applied exactly once ([Testing](testing.md#the-simulations)).

## Decisions

- [D2](../decisions/D02-peer-protocol-v0-or-v1.md): peering requests carry the full signed node info; termination messages exist; scheme versions are exchanged.
- [D4](../decisions/D04-network-membership-economics.md): removal and expiry, automatic registration, the served list, the membership freeze, authenticated lists, and the limits on free registration until fees.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): the peering lifecycle, confirmed peers, the bridge rule, slot caps, repeats and crossings, the advertise flag, termination, bans as a list, the 512-channel limit, and why child peers wait.
- [D15](../decisions/D15-bootstrap-network-for-v1.md): the testnet's bootstrap nodes as the default BNL.
- [D20](../decisions/D20-trust-follows-what-a-peer-earned-us.md): junior and senior.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): only the owner bans.

## Original

- [3 §9–§10](../original/03-peer-network.md): the node table, link-state generation, processing, flooding, heartbeats, broadcast and refresh.
- [3 §11](../original/03-peer-network.md): the BNL, node lists, majority (3 §11.4) and registration.
- [3 §12](../original/03-peer-network.md): the peering states, auto-peering, uplink addresses and the scheme contract.
- [3 §13](../original/03-peer-network.md): routing.
- [3 §15–§17](../original/03-peer-network.md): persistence, constants and timers.
- [2 §14](../original/02-node-runtime.md): the original's `GET /nodelist/meta`, which the rewrite does not serve; the hash poll is `nodeListHashRequest`.
