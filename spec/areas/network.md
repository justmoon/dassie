# Network

How Dassie nodes find each other and stay connected: the node table, link states and how they spread, membership through bootstrap nodes, peering and its lifecycle, bans, and routing. The messages these use are in [Peer protocol](peer-protocol.md).

**Paths:** `crates/dassie-peer/src/node/`, `crates/dassie-peer/src/table.rs`, `crates/dassie-peer/src/routing.rs`, `crates/dassie-peer/src/channel.rs`, `crates/dassie-peer/src/persist.rs`, `crates/dassie-peer/src/config.rs` (every timer and bound, with the dev values), `crates/dassie-peer/src/hooks.rs`, `crates/dassie-node/src/env.rs` (bootstrap list)
**Read with:** [Peer protocol](peer-protocol.md), [Settlement](settlement.md), [Connector](connector.md), [Identity](identity.md), [Node](node.md)

## The peer node

`dassie-peer`'s `PeerNode` is a sans-IO state machine that `dassie-node` drives; its changes are written in the same transaction as the ledger's ([Node](node.md)). It wakes only when a timer is due. Every table it keeps is bounded, so no other node can grow its memory.

A node's ILP address is `<scheme>.das.<nodeId>`, with the allocation scheme set by the node's realm. A peer is a connector endpoint at its own such address, which ILDCP tells it ([Connector](connector.md)).

## Node table

The table holds what a node knows about other nodes: per node, its key, its newest link state, why it is in the table, its registration with us and our peering with it.

- **A node enters only for a reason:** it is this node, a configured bootstrap node, a peer (or being asked to peer), on the majority of bootstrap lists, or registered with us. A link state of a node not in the table is dropped, so gossip alone cannot fill the table.
- **When the table is full,** a newcomer takes the place of the node whose strongest reason ranks lowest, if that ranks below the newcomer's (self, bootstrap, peer, majority, registered, in that order). Registrations cannot push out the nodes a node depends on.
- A node is removed a grace period after its last reason is gone, with its link state.

## Link states

### The own link state

- A node without a valid URL issues no link state ([Peer protocol](peer-protocol.md#verification)).
- It is issued at start, when its neighbors, schemes, URL or alias change (debounced), and periodically regardless.
- **Sequence:** `max(now, last + 1)`, with the last value persisted, so it only rises across restarts. If the persisted value is further ahead of the clock than any node accepts, it was issued under a wrong clock and no node holds it; the sequence starts again from `now`.
- Its neighbors are the advertised peers ([Advertised peers](#advertised-peers)).
- A new own link state is flooded to peers at once, and sent directly to the bootstrap nodes we are not peered with, so they can answer refreshes for it.

### Receiving a link state

Cheap checks come first, so floods are cheap: an echo of our own state, a node not in the table, a sequence not newer than the one held, or a sequence too far in the future is dropped before the signature is checked.

- **Rate per subject.** Each node may issue only a bounded burst of states, then a steady rate; the excess is neither stored nor forwarded. The rate is checked before the signature, but a token is spent only on a state that verifies, so forged states cannot use up the real node's allowance.
- **Freshness.** A state is fresh while its sequence is within a window around now. Routing uses only fresh states, and stale ones are refreshed.

### How link states spread

The gossip defaults (copies before suppression, forward jitter, heartbeat interval, staleness, re-issue) are the original's, kept because they proved sound; the peering timings were set from the network simulation ([D12](../decisions/D12-peering-lifecycle-and-liveness.md)). All are in `crates/dassie-peer/src/config.rs`.

- **Flooding.** A state taken from a `linkStateUpdate` is forwarded after a random delay to every peer except its subject and the peer it came from, unless enough copies arrived meanwhile. Our own new state goes at once. States learned from refreshes, sweeps, registrations and peering requests are not forwarded.
- **Heartbeats.** A node sends its current state, authenticated, to every peer it is not terminating with, advertised or not, at random intervals and once when a peering forms, so both sides can confirm each other ([Advertised peers](#advertised-peers)).
- **Refresh.** A node picks stale or missing states one at a time and asks the node itself, at the URL of its last state; sometimes, or when it has no state of the node, it asks another node instead, together with other stale ones. A node asked about that brings back nothing newer backs off exponentially.
- **Anti-entropy sweep.** Periodically, whether or not anything is stale, a node asks a random peer (or a bootstrap node) for the states of the next few nodes in its table, round and round. Flooding alone loses an update until the next re-issue when a message is lost, a node is down or the network splits; the sweep repairs it within one pass of the table.

## Membership

The network's members are the nodes that a majority of the bootstrap nodes list. Bootstrap nodes take registrations and serve lists of the nodes registered with them. The design and its economics are [D4](../decisions/D04-network-membership-economics.md).

### Bootstrap nodes

- The bootstrap node list is `DASSIE_BOOTSTRAP_NODES` (format and validation in `crates/dassie-node/src/env.rs`). A bad entry stops the node at startup, as does a node listed twice, which would vote twice. The default is the public testnet's three nodes ([D15](../decisions/D15-bootstrap-network-for-v1.md)); tests and the dev environment set their own, so they stay off the testnet. An empty list makes a node run alone.
- Bootstrap nodes are always in the table, with their configured key, and always keep their replay state.
- A node takes registrations only when it is one of its own bootstrap nodes, so each bootstrap node lists itself in its own configuration. Each testnet bootstrap node runs `xrpl-testnet`, so a new node has someone to peer with.

### Node lists and the majority

- A node polls each bootstrap node for its list's hash, and downloads the list when the hash changed. A list is accepted only in a response authenticated under the bootstrap node's configured key ([Peer protocol](peer-protocol.md#message-set)). Polls and downloads only read, so they get through however full the bootstrap node's pool of replay state for strangers is ([Peer protocol](peer-protocol.md#replay-protection)).
- A bootstrap node serves its unexpired registrations and itself. It lists itself and registers with the other bootstrap nodes like any node. It does not serve the majority back, which would keep a node listed forever once it had made it.
- A node is in the majority when strictly more than half of the *configured* bootstrap nodes list it, not of those that answer, so silencing bootstrap nodes cannot shrink the quorum. A list not fetched for a long time no longer votes.
- **Membership freeze.** Majority nodes are added to the table, and lose their place only while more than half of the configured lists are fresh. Otherwise membership freezes, so bootstrap nodes going offline, or a node cut off from them, do not empty every table in the network. The majority set is persisted.

### Registration

**Client side.** A node with a link state registers with every bootstrap node other than itself at start, renews before its registration lapses, registers again if a bootstrap node's list still lacks it a while after, and backs off exponentially after failures. The owner can also register at once ([RPC](rpc.md)).

**Bootstrap side.** Registration is free, so it is throttled instead ([roadmap](../roadmap.md#registration-fees)):

- new registrations draw from a global bucket, spent only when a registration succeeds;
- one registration per URL host and port, and a cap per /24 (IPv4) or /48 (IPv6) of the address it came from;
- before accepting a new registration, or one whose URL changed, the bootstrap node probes that URL; a renewal from the probed URL is answered at once;
- a registration expires unless renewed, and the node leaves the served list;
- a registered node keeps its replay state while registered, so strangers filling the replay pool cannot turn its renewals away ([Peer protocol](peer-protocol.md#replay-protection)).

**Probes.** Registration and peering probe the requester's URL the same way: an authenticated `linkStateRequest` for the requester itself, which must return a verified state of that node at the registered sequence or later. That proves the URL serves the node's key. Since a requester picks the URL, probes are rate-limited, never made twice at once to the same node or host, and the requests waiting on them are bounded; a node cannot be used to hammer a third party.

Every probe spends a token of the node's probe bucket, whatever its outcome: failing probes are what it is there to stop. In front of it, each /24 (IPv4) or /48 (IPv6) the requests come from has a small share of its own. A probe holds a token of its network's share while it is in flight and gives it back if it succeeds, so only failing probes spend the share. One network's failing probes therefore cannot starve requesters from other networks: keeping the bucket empty takes failing probes from many networks at once. Nodes that share a network, as dev nodes on loopback do, spend nothing on probes that succeed, and wait only while a share's worth of them is in flight. The shares are bounded in number; when they are full, the network that has spent least loses its share first.

### How a node joins

1. It registers with each bootstrap node, which probes its URL and lists it.
2. Other nodes poll the lists; once a majority lists it, they add it to their tables.
3. They learn its link state from refreshes, sweeps or flooding.
4. Nodes that peer with it advertise it as a neighbor, which makes it routable.

## Peering

Each node looks for a few peers on its own (auto-peering). A peering is between two nodes on one settlement scheme, at one version of that scheme.

### Looking for peers

- **Target.** Only advertised peers count toward it ([Advertised peers](#advertised-peers)), so a node whose peers went silent, flap or are stuck looks for others.
- **Candidates** are table nodes with a fresh link state listing a scheme we have active, not peered or being asked, not banned, not in backoff, and with room for a settlement channel. Nodes our routes do not reach come first, then majority nodes. One attempt runs at a time.
- After a restart with peers, a node waits a heartbeat's time before looking, so the peers it has can confirm themselves first.
- **Slot caps.** A node starts a bounded number of outbound peerings and holds a bounded total; inbound requests are accepted only below a cap under that total, so requests alone cannot fill every slot.
- **Bridge rule.** If a majority node with a fresh link state has stayed unreachable by our routes for a while, the node starts one more outbound peering, with such a node, if it can. This joins separate islands of peers that each met their target.
- **Backoff.** A failed attempt backs off from that node exponentially. A refusal for "unknown node" is retried after one list poll instead, since the responder has not seen the majority list us yet; one for "concurrent request" after a short wait.

### Asking and answering

1. The asking node sends `peeringInfoRequest` for the scheme, and its scheme builds the request data from the answer.
2. It sends `peeringRequest` with its signed link state, the scheme's data and the scheme versions it speaks.
3. The responder checks the request (`crates/dassie-peer/src/node/peering.rs` has the order and the refusal reasons), takes in the requester's link state, and probes its URL as for registration. Its scheme then accepts, opening the peer's accounts. The peering is persisted before the acceptance is sent, and so before the requester uses it.
4. On acceptance, the requester's scheme finalizes the peering and opens the peer's accounts.

The settlement schemes have their say through `PeeringHooks` (`crates/dassie-peer/src/hooks.rs`, [Settlement](settlement.md)).

- **Repeats.** A request from a node already peered on the same scheme is answered at once, without a probe, with the stored peering's version. This covers an acceptance that got lost.
- **Crossings.** When both ask at once, the lower node ID's request wins.

### Roles

The **junior** is the node whose peering request was accepted, the **senior** the node that accepted it; when requests cross, the lower node ID is the junior. The junior extends the senior a default credit line, the senior extends the junior none at first, and credit then grows with what a peer earned us ([Settlement](settlement.md#credit), [D20](../decisions/D20-trust-follows-what-a-peer-earned-us.md)).

## Lifecycle

A peering is `requesting`, `peered` or `terminating` ([D12](../decisions/D12-peering-lifecycle-and-liveness.md)). "Peered" in other documents covers `peered` and `terminating`; "active" means `peered`.

**Withdrawing is prompt, ending is not.** A node takes a peer out of its link state moderately soon, weighing route flaps against stale routes that lose packets: a withdrawn peer stays peered, and comes back once it is healthy and its hold-down is over. Ending a peering is for good, and needs much stronger evidence: the owner's word, or a peer silent or short of room for as long as a silent peer is kept ([Termination](#termination)). Nothing that withdraws a peer ends it.

### Advertised peers

A node lists a peer as a neighbor in its link state, and routing uses the edge, only while the peer is `peered`, **confirmed** (an authenticated message from it arrived recently), not banned, not held down, and not withdrawn for room.

- **Hold-down.** Each time a peer stops being advertised it is held down, for a time that doubles with each flap and resets after a long stable stretch, so a flapping peer does not churn routes across the network.
- **Restarts** do not drop neighbors: confirmation starts from the persisted time the peer was last heard from, without a hold-down.
- **Withdrawal for room.** Withdrawing an edge removes it both ways for the whole network, turning a temporary T04 into a final F02, so neither a failed delivery nor a peer waiting seconds for a routine settlement withdraws it. A peer whose room in our books has stayed below one of the largest packets for the grace, without settling, is stuck: settling, not traffic, restores room, and a creditor asks over the peer protocol, which needs no route ([D12](../decisions/D12-peering-lifecycle-and-liveness.md)). It is withdrawn like a flap: still peered and asked to settle, but no packet crosses the edge either way until its room is back and its hold-down is over. The grace lets two requests to settle go unanswered and still gives the third a request's timeout to be answered, and its settlement time to be booked. A request that surely never reached the peer costs none of it: it is asked again at the next check ([Settlement](settlement.md#asking-a-peer-to-settle)). A node counts only the room it gives a peer, never the room the peer gives it.
- **Counting the time short.** Each settlement booked from the peer starts the grace again. The time short is a total, grown only while the node can ask (its scheme sees its ledger), by at most two check intervals per check, so neither the node's downtime, a stall of its host, nor an outage of its own ledger counts against the peer. It is persisted with the peering, so a restart neither forgives it nor starts it again.

### Termination

Termination starts when the owner asks, when the owner bans the node, when the peer has not been heard from for a long time (counted from the later of its last message and the node's start, so our own downtime ends nothing), or when its room has stayed short, without a settlement, for as long.

The node that ends a peering collects first, and tells the peer only then ([D28](../decisions/D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md)):

1. **It decides privately.** Nothing the peer can see changes: a peer it advertised stays advertised (unless it is banned, which withdraws it at once, [below](#bans)), and our own Prepares still go to it. Its owner sees it terminating. Its new Prepares are refused, as for want of liquidity ([Connector](connector.md#responses-and-expiry)), and its requests to settle are answered as ones that settle nothing.
2. **It waits for its own settlements to the peer under way,** so the peer counts them; then asks the peer to settle, once. The answer says what the peer has on its way ([Settlement](settlement.md#settling-on-request)), and the node waits until that is booked. Each wait lasts at most a time a settlement takes to reach the ledger and be announced (`depeer_wait_ms`). A peer that does not answer is waited out.
3. **Then it tells the peer:** it stops advertising it, takes no new Prepares to or from it, and sends `peeringTermination` until it is acknowledged or times out. Responses still flow.
4. **The other side ends at once** when the message arrives, as the node does that is told first when both decided: it stops advertising and routing to the peer, takes no new Prepares and settles nothing.
5. Once the exchange is over, no packet is in flight with the peer and no response to it waits to be sent again, the peering ends. Neither side settles anything for the end ([Settlement](settlement.md#settling-at-a-peerings-end)): what the books show stays owed, for a later peering to settle.
6. The node does not ask the peer again for a while after an owner's termination, and refuses its requests meanwhile. That cooldown does not survive a restart; a ban is the lasting way.

**Termination ends a peering, not a relationship.** The peer's accounts stay, with their balances, as the record of what is owed, and so does the settlement channel with its sequence ([Settlement channels](#settlement-channels)). A settlement that arrives after the peering ended is still credited once, and a later peering with the node reuses both without posting trust twice. A termination under way survives a restart, and so does a decision to end a peering; its steps start again after it.

### One-sided peerings

A node that holds no peering with a sender, and is not asking for one, answers an authenticated heartbeat that lists it as a neighbor with a `peeringTermination` of reason "not peered", at most once per termination timeout per node. The other side then ends its peering.

### Bans

- Bans are a bounded list the owner keeps. Only the owner bans; a node never bans a peer for what it cost ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)).
- A banned node is never asked, is refused, and its messages do not confirm it as a peer. A ban ends a peering as the owner's termination does. Its link states still flood: a ban is one node's choice, and other nodes route by both ends' word.
- A ban is a list, not a peering state, since a node can be banned while its peering is still terminating.

## Settlement channels

Every peer gets a settlement channel, which belongs to the pair of nodes and outlives the peering ([Settlement](settlement.md#settlement-channels)). A node we hold one with keeps its replay state ([Peer protocol](peer-protocol.md#replay-protection)), and its Fulfills, Rejects and settlements are taken even with no peering. Channels are never removed and are bounded in number; once all are taken, a node without one is no candidate and its peering requests are refused for capacity ([roadmap](../roadmap.md#not-built-yet)).

## Routing

Routing runs a breadth-first search from this node over edges that both ends advertise and whose link states are fresh.

- An edge counts only if both ends list each other, so no node can claim a link the other end does not confirm, and a node whose state went stale drops out.
- Each reachable node's address gets a few first hops on shortest paths. There is no route for the node's own address: destinations under it go only to its own endpoints. Prefix matching, first-hop choice and never sending a packet back where it came from are the connector's ([Connector](connector.md#routing)).
- Routes are recomputed when the graph may have changed, debounced, and periodically as states go stale.
- Child peers routed beneath their uplink's address, the periphery's downlinks, are not built ([roadmap](../roadmap.md#periphery)).

## Persistence

The peer node's state is written in the same transaction as the ledger, on every change (`crates/dassie-peer/src/persist.rs`, `crates/dassie-node/src/store/peer.rs`). What survives a restart is chosen so that a restart neither drops a peer nor forgives one: peerings with when each was last heard from and how long its room has been short, peerings being ended (decided or told), replay floors, the own sequence, the majority, settlement channels and bans. Link states were verified when received and are not verified again.

What is lost is either fetched again (bootstrap lists) or only paces the node's own attempts: backoffs, flood counters, hold-downs and the owner-termination cooldown. A peering attempt cut short by a restart is dropped.

## Tests

The `peer` simulation (`dassie_peer::sim::chaos`) runs this protocol on a rough network with crashes, partitions, terminations and bans, and checks that link states and routes converge, peerings are two-sided, every termination finishes, and each settlement is applied exactly once ([Testing](testing.md#the-simulations)).

## Decisions

- [D2](../decisions/D02-peer-protocol-v0-or-v1.md): v1 instead of v0: authenticated senders and responses, peering requests that carry the signed node info, termination, scheme versions.
- [D4](../decisions/D04-network-membership-economics.md): removal and expiry, automatic registration, the served list, the membership freeze, authenticated lists, and the limits on free registration until fees.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): the peering lifecycle, confirmed peers, the bridge rule, slot caps, repeats and crossings, the advertise flag and withdrawal for room, termination, bans as a list, the channel limit, and why child peers wait.
- [D15](../decisions/D15-bootstrap-network-for-v1.md): the testnet's bootstrap nodes as the default list.
- [D20](../decisions/D20-trust-follows-what-a-peer-earned-us.md): junior and senior.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): only the owner bans.
