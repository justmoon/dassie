# Network

How Dassie nodes find each other and stay connected: the node table, link states and how they spread, membership through bootstrap nodes, names, peering and its lifecycle, bans, and routing. The messages these use are in [Peer protocol](peer-protocol.md).

**Paths:** `crates/dassie-peer/src/node/`, `crates/dassie-peer/src/names/registrar.rs`, `crates/dassie-peer/src/names/holders.rs`, `crates/dassie-peer/src/table.rs`, `crates/dassie-peer/src/routing.rs`, `crates/dassie-peer/src/channel.rs`, `crates/dassie-peer/src/persist.rs`, `crates/dassie-peer/src/config.rs` (every timer and bound, with the dev values), `crates/dassie-peer/src/hooks.rs`, `crates/dassie-node/src/env.rs` (bootstrap list)
**Read with:** [Peer protocol](peer-protocol.md), [Settlement](settlement.md), [Connector](connector.md), [Identity](identity.md), [Node](node.md)

## The peer node

`dassie-peer`'s `PeerNode` is a sans-IO state machine that `dassie-node` drives; its changes are written in the same transaction as the ledger's ([Node](node.md)). It wakes only when a timer is due. Every table it keeps is bounded, so no other node can grow its memory.

A node's ILP address is `<scheme>.das.<nodeId>`, with the allocation scheme set by the node's realm. A peer is a connector endpoint at its own such address, which ILDCP tells it ([Connector](connector.md)).

## Node table

The table holds what a node knows about other nodes: per node, its key, its newest link state, why it is in the table, its registration with us and our peering with it.

- **A node enters only for a reason:** it is this node, a configured bootstrap node, a peer (or being asked to peer), on the majority of bootstrap lists, or registered with us. A link state of a node not in the table is dropped, so gossip alone cannot fill the table.
- **When the table is full,** a newcomer takes the place of the node whose strongest reason ranks lowest, if that ranks below the newcomer's (self, bootstrap, peer, majority, registered, in that order). Registrations cannot push out the nodes a node depends on.
- A node is removed a grace period after its last reason is gone, with its link state.
- **Downlinks** ([below](#downlinks)) are in the table under a bound of their own: they take no other node's place, and none takes theirs.

## Link states

### The own link state

- A node without a valid URL issues no link state ([Peer protocol](peer-protocol.md#verification)), unless it is a periphery node, whose link state has none ([Periphery nodes](#periphery-nodes)).
- It is issued at start, when its neighbors, schemes, URL or alias change (debounced), and periodically regardless.
- **Sequence:** `max(now, last + 1)`, with the last value persisted, so it only rises across restarts. If the persisted value is further ahead of the clock than any node accepts, it was issued under a wrong clock and no node holds it; the sequence starts again from `now`.
- A core node's neighbors are its advertised peers ([Advertised peers](#advertised-peers)); a periphery node lists none ([Periphery nodes](#periphery-nodes)). Its schemes are those the node peers on: active, synced with their ledger at least once, and able to receive a peer's settlement ([below](#asking-and-answering)).
- **It always fits what every node decodes.** Its neighbors are bounded by the core peering slots and its schemes by `MAX_SCHEMES` (`crates/dassie-peer/src/node/mod.rs`), and the configuration is checked at start against the link state's bounds in entries and bytes ([Peer protocol](peer-protocol.md#link-states)), so a node never signs one that others refuse.
- A new own link state is flooded to peers at once, and sent directly to the bootstrap nodes we are not peered with, so they can answer refreshes for it.

### Receiving a link state

Cheap checks come first, so floods are cheap: an echo of our own state, a node not in the table, a sequence not newer than the one held, or a sequence too far in the future is dropped before the signature is checked.

- **Rate per subject.** Each node may issue only a bounded burst of states, then a steady rate; the excess is neither stored nor forwarded. The rate is checked before the signature, but a token is spent only on a state that verifies, so forged states cannot use up the real node's allowance.
- **Freshness.** A state is fresh while its sequence is within a window around now. Routing uses only fresh states, and stale ones are refreshed.

### How link states spread

The gossip defaults (copies before suppression, forward jitter, heartbeat interval, staleness, re-issue) are the original's, kept because they proved sound; the peering timings were set from the network simulation ([peering-lifecycle](../decisions/peering-lifecycle.md)). All are in `crates/dassie-peer/src/config.rs`.

- **Flooding.** A state taken from a `linkStateUpdate` is forwarded after a random delay to every peer except its subject and the peer it came from, unless enough copies arrived meanwhile. Our own new state goes at once. States learned from refreshes, sweeps, registrations and peering requests are not forwarded, nor is a periphery node's, and a periphery node forwards none. Nothing is forwarded to a downlink.
- **Heartbeats.** A node sends its current state, authenticated, to every peer it is not terminating with, advertised or not, at random intervals and once when a peering forms, so both sides can confirm each other ([Advertised peers](#advertised-peers)).
- **Refresh.** A node picks stale or missing states one at a time and asks the node itself, at the URL of its last state; sometimes, or when it has no state of the node, it asks another node instead, together with other stale ones. A node asked about that brings back nothing newer backs off exponentially.
- **Anti-entropy sweep.** Periodically, whether or not anything is stale, a node asks a random peer (or a bootstrap node) for the states of the next few nodes in its table, round and round. Flooding alone loses an update until the next re-issue when a message is lost, a node is down or the network splits; the sweep repairs it within one pass of the table. A peer is not asked for its own state, which its heartbeats bring. A bootstrap node that a node without peers asks is asked for its own state too, whenever the sweep's window holds its ID: once a pass of the table, which for a newcomer, whose table fits in one request, is every sweep. Where it is the only bootstrap node, or the others are out of reach, nothing else brings its state to that node. Without that, a newcomer that registered before its schemes synced, and whose new link state then got lost on the way, would not peer with its bootstrap node until its next re-issue: that holds the newcomer's state without the schemes, and the newcomer has none of the bootstrap node's.

## Membership

The network's members are the nodes that a majority of the bootstrap nodes list. Bootstrap nodes take registrations and serve lists of the nodes registered with them. The design and its economics are [membership-economics](../decisions/membership-economics.md).

### Bootstrap nodes

- The bootstrap node list is `DASSIE_BOOTSTRAP_NODES` (format and validation in `crates/dassie-node/src/env.rs`). A bad entry stops the node at startup, as does a node listed twice, which would vote twice. The default is the public testnet's four nodes ([bootstrap-network](../decisions/bootstrap-network.md)); tests and the dev environment set their own, so they stay off the testnet. An empty list makes a node run alone.
- Bootstrap nodes are always in the table, with their configured key, and always keep their replay state.
- A node takes registrations only when it is one of its own bootstrap nodes, so each bootstrap node lists itself in its own configuration. Each testnet bootstrap node runs `xrpl-testnet` and `xrpl-testnet-dsusd`, so a new node on either has someone to peer with.

### Node lists and the majority

- A node polls each bootstrap node for its list's hash, and downloads the list when the hash changed. A list is accepted only in a response authenticated under the bootstrap node's configured key ([Peer protocol](peer-protocol.md#message-set)). Polls and downloads only read, so they get through however full the bootstrap node's pool of replay state for strangers is ([Peer protocol](peer-protocol.md#replay-protection)).
- A bootstrap node serves its unexpired registrations and itself. It lists itself and registers with the other bootstrap nodes like any node. It does not serve the majority back, which would keep a node listed forever once it had made it.
- A node is in the majority when strictly more than half of the *configured* bootstrap nodes list it, not of those that answer, so silencing bootstrap nodes cannot shrink the quorum. A list not fetched for a long time no longer votes.
- **Membership freeze.** Majority nodes are added to the table, and lose their place only while more than half of the configured lists are fresh. Otherwise membership freezes, so bootstrap nodes going offline, or a node cut off from them, do not empty every table in the network. The majority set is persisted.

### Registration

**Client side.** A periphery node registers nowhere, and a bootstrap node refuses its registration. Any other node with a link state registers with every bootstrap node other than itself at start, renews before its registration lapses, registers again if a bootstrap node's list still lacks it a while after, and backs off exponentially after failures. The owner can also register at once ([RPC](rpc.md)).

**Bootstrap side.** Registration is free, so it is throttled instead ([roadmap](../roadmap.md#registration-fees)):

- new registrations draw from a global bucket, spent only when a registration succeeds;
- one registration per URL host and port, and a cap per /24 (IPv4) or /48 (IPv6) of the address it came from;
- before accepting a new registration, or one whose URL changed, the bootstrap node probes that URL; a renewal from the probed URL is answered at once;
- a registration expires unless renewed, and the node leaves the served list;
- a registered node keeps its replay state while registered, so strangers filling the replay pool cannot turn its renewals away ([Peer protocol](peer-protocol.md#replay-protection)).

**Probes.** Registration and peering probe the requester's URL the same way: an authenticated `linkStateRequest` for the requester itself, which must return a verified state of that node at the registered sequence or later. That proves the URL serves the node's key. Since a requester picks the URL, probes are rate-limited, never made twice at once to the same node or host, and the requests waiting on them are bounded; a node cannot be used to hammer a third party.

Every probe spends a token of the node's probe bucket, whatever its outcome: failing probes are what it is there to stop. In front of it, each /24 (IPv4) or /48 (IPv6) the requests come from has a small share of its own. A probe holds a token of its network's share while it is in flight and gives it back if it succeeds, so only failing probes spend the share. One network's failing probes therefore cannot starve requesters from other networks: keeping the bucket empty takes failing probes from many networks at once. Nodes that share a network spend nothing on probes that succeed, and wait only while a share's worth of them is in flight. Loopback is no network where private peers are allowed ([Peer protocol](peer-protocol.md#replay-protection)), so dev nodes draw only on the bucket. The shares are bounded in number; when they are full, the network that has spent least loses its share first.

### How a node joins

1. It registers with each bootstrap node, which probes its URL and lists it.
2. Other nodes poll the lists; once a majority lists it, they add it to their tables.
3. They learn its link state from refreshes, sweeps or flooding.
4. Nodes that peer with it advertise it as a neighbor, which makes it routable.

## Names

A node's owner can register a name for the node's ID, held by the bootstrap nodes as its registrars ([the design](../designs/names.md), [names-by-earliest-commitment](../decisions/names-by-earliest-commitment.md)); the messages are in [Peer protocol](peer-protocol.md#names). The registrar's side is built, releasing a split included: the rule in `crates/dassie-peer/src/names/registrar.rs`, who may ask what in `crates/dassie-peer/src/node/names.rs`. So is every node's view of who holds a name, in `crates/dassie-peer/src/names/holders.rs`, which needs nothing of the peer node, so that a front door can keep one too. Registrants are not built ([roadmap](../roadmap.md#names)).

### The registrar

A node is a registrar when it is one of its own bootstrap nodes, as it takes registrations only then ([Bootstrap nodes](#bootstrap-nodes)). Any other node refuses every name message with 409, the list requests too: only registrars' lists count. Everything a registrar decides is by its own log and clock; registrars never agree on time or order with each other.

- **The log.** A commitment is logged at the next position and at the registrar's time, `max(now, last + 1)`, both persisted, so both only rise across restarts, as the own link state's sequence does ([The own link state](#the-own-link-state)). Its window closes a set time after it was logged. It is on disk, in the batch that logs it, before its receipt goes out, so a crash loses no receipted commitment and never reorders the log. A commitment in the log sent again gets its receipt again, at no cost, since a registrant retries until it holds every receipt.
- **Unsealing.** An unsealing is valid if it opens a commitment in the log, sealed for its sender, before the commitment's window closes, for a name that is not reserved, and the name was neither listed nor in its grace period here when the commitment was logged. An invalid one is refused with 409. A valid one is answered alike whether it still contends for the name or has lost already, to a name granted since its commitment was logged, so the answer says nothing of commitments still sealed. One taken already is answered as taken for as long as its commitment is in the log, after its window closed too, so a registrant whose answer was lost and who retries late counts the registrar as having taken it, as it has.
- **Deciding.** As windows close, commitments are decided in the order of the log. One never unsealed lapses. One that contends is granted the name if no commitment earlier in the log won it, and only once every earlier window has closed, since until then an earlier commitment can still take the name; nothing about a name is shown before. The rest lose. So the commitment logged first among a name's valid unsealings wins, whenever it was unsealed, and a name waits a window before it is granted.
- **Holding.** A granted name is listed for a lifetime. Its holder alone renews it, once its expiry is near, to a lifetime past that expiry, or in its grace period, to a lifetime from now; a renewal before its expiry is near is answered with the expiry it has, so a holder cannot move the list's hash at will. Unrenewed, it leaves the list at its expiry, stays the holder's for its grace period, and is then free. What it was is kept until no commitment in the log was logged while it was held, so such a commitment stays void for it.
- **The list** is the names listed, with their holders and expiries, sorted by name, served by hash and download like the node list ([Node lists and the majority](#node-lists-and-the-majority)), and rebuilt only once a grant, a renewal or an expiry changed it. Its bound, `MAX_NAME_LIST`, holds the bound on names below.
- **Free, so throttled,** as registration is until there are fees ([roadmap](../roadmap.md#registration-fees)). New commitments draw from a global bucket, spent only on one logged; the log holds a bounded number from each /24 or /48, and from each node, which together with the names it holds stay under a cap of their own. That cap is also the holder's: an unsealing is refused with 409 once the node named in it holds that many names with its unsealings that contend, so commitments other nodes log for a node, which count against their senders, do not take it past the cap. The log is bounded, and so are the names with the commitments in the log, each of which may become one, so a grant never finds the names full. A bound refuses with 503, a cap with 409 and the bucket with 429, as for registration.
- **Replay state.** A node with a commitment in the log, an unsealing there or a name held keeps its replay state, so strangers filling the pool cannot turn its unsealing or renewal away, and lets it go once it has none ([Peer protocol](peer-protocol.md#replay-protection)).
- **Events in counts.** Each step of the registrar's timer reports what it decided, names granted, unsealings lost, commitments lapsed and names freed, in one event of counts, however many it decided; what it lists is read from its state. The node's event queue is bounded and drops its oldest, and a registrar back after a downtime past its windows or its names' grace may decide every commitment and free every name in one step, after the peering events of the same tick, which the node acts on, were queued.
- **Timers and bounds** are `RegistrarConfig` (`crates/dassie-peer/src/names/registrar.rs`). The window gives a commitment and its unsealing each a request's time and retries (`least_window_ms`); renewals and the grace period leave room for a renewal to be retried at the longest backoff; the wait before a split is released is longer than a registration takes to reach every list (`least_release_ms`, [below](#releasing-a-split)); dev values follow production's rules (`Config::DEV`).

### Holders

Every node, a registrar too, takes as a name's holder the node that a quorum of the configured registrars list for it ([the design](../designs/names.md#5-holding-a-name)).

- **Reading the lists.** A node polls each registrar's name-list hash with its node list, at the same interval, and downloads the list only when the hash changed. A list is taken only from a response authenticated under the registrar's configured key, as a node list is ([Node lists and the majority](#node-lists-and-the-majority)). A registrar's own list is its own, without asking.
- **The quorum** is n - f of the n *configured* registrars, not of those that answer, where f is the largest with n ≥ 3f + 1: three of four, four of five, and all of up to three. A quorum is more than two thirds, so it names at most one holder for a name, and any two quorums share an honest registrar, which lists one holder: a registrar that shows nodes different lists cannot lead two of them to different holders. A strict majority, as membership takes, would let one such registrar do that after an honest split ([names-by-earliest-commitment](../decisions/names-by-earliest-commitment.md)).
- **Votes and the freeze.** As for membership, a list not fetched for a long time no longer votes, and the holders are taken only while a quorum of the lists are fresh. Otherwise the view freezes: it keeps every holder it had, one whose name has expired since too, rather than lose names to registrars going down or to a node cut off from them. The view is persisted, so a freeze outlasts a restart; until a quorum of the lists are fetched after it, the node keeps what it had.
- **The expiry** a node shows for a name is the latest one a quorum of the lists give it, by the registrars' clocks.
- **Bounds.** The registrars are the bootstrap nodes, at most `MAX_REGISTRARS`, each list is bounded by `MAX_NAME_LIST`, and a holder needs more than two thirds of the lists, so a view holds at most `MAX_HOLDERS`, one and a half lists' worth (`crates/dassie-peer/src/names/holders.rs`).

### Releasing a split

Two registrants that commit to a name at nearly the same time may be logged in different orders by different registrars and split the vote, so that no holder has a quorum. A registrar releases a split rather than keep the name from everyone until it expires ([the design](../designs/names.md#5-holding-a-name)).

- **The rule.** A registrar drops a name it lists when more than f of the other registrars' fresh lists lack its holder for it, listing nobody or another node, for longer than a set wait (`release_after_ms`). The name is free at once there: a new commitment may have it, while one logged while it was held stays void for it.
- **Never a holder with a quorum.** A holder that a quorum of honest registrars list is missing from at most f lists, so it is never released, however late or seldom the lists arrive: a list not fresh counts for nothing, nor one not fetched.
- **Not for a registration on its way.** A registration's commitments are all logged within the first one's window, so every registrar grants it within a window of the first; a list fetched before that grant stays fresh for a while, and the next one comes a poll and a download later. The wait is longer than all of these together (`least_release_ms`), so lists that are merely late release nothing.
- **Checked at each poll** and after each list read, by the registrar's own view of the others' lists; a check that finds the name not split starts the wait again. The wait is not persisted: after a restart it starts again, which only delays a release.
- **A faulty registrar** can keep a split from being released, by showing each side its own holder confirmed, and can have a name released whose quorum needed its own list, by showing the registrars that list it another holder.

## Peering

Each node looks for a few peers on its own (auto-peering). A peering is between two nodes on one settlement scheme, at one version of that scheme. A peering that settles by every scheme both run is not built yet ([roadmap](../roadmap.md#settlement-methods)).

### Looking for peers

- **Target.** Only advertised peers count toward it ([Advertised peers](#advertised-peers)), so a node whose peers went silent, flap or are stuck looks for others. Downlinks do not count.
- **Candidates** are table nodes with a fresh link state listing a scheme we peer on, not peered or being asked, not banned, not in backoff, and with room for a settlement channel. Nodes our routes do not reach come first, then majority nodes. One attempt runs at a time.
- After a restart with peers, a node waits a heartbeat's time before looking, so the peers it has can confirm themselves first.
- **Slot caps.** A node starts a bounded number of outbound peerings and holds a bounded total; inbound requests are accepted only below a cap under that total, so requests alone cannot fill every slot. Downlinks take none of these slots ([Downlinks](#downlinks)).
- **Bridge rule.** If a majority node with a fresh link state has stayed unreachable by our routes for a while, the node starts one more outbound peering, with such a node, if it can. This joins separate islands of peers that each met their target.
- **Backoff.** A failed attempt backs off from that node exponentially. A refusal for "unknown node" is retried after one list poll instead, since the responder has not seen the majority list us yet; one for "concurrent request" after a short wait.

### Asking and answering

1. The asking node sends `peeringInfoRequest` for the scheme, and its scheme builds the request data from the answer.
2. It sends `peeringRequest` with its signed link state, the scheme's data and the scheme versions it speaks.
3. The responder checks the request (`crates/dassie-peer/src/node/peering.rs` has the order and the refusal reasons), takes in the requester's link state, and probes its URL as for registration; a periphery requester has none, and is not probed ([Downlinks](#downlinks)). Its scheme then accepts, opening the peer's accounts. The peering is persisted before the acceptance is sent, and so before the requester uses it.
4. On acceptance, the requester's scheme finalizes the peering and opens the peer's accounts.

The settlement schemes have their say through `PeeringHooks` (`crates/dassie-peer/src/hooks.rs`, [Settlement](settlement.md)).

- **Only on a scheme that has seen its ledger.** A node offers, asks for and accepts peerings on a scheme only once the scheme has synced with its ledger at least once: before that, a peer's settlement would be booked as the owner's deposit ([Settlement](settlement.md#the-scheme-interface), [synced-before-peering](../decisions/synced-before-peering.md)). A request on another scheme is refused as unsupported, and the requester backs off as from any refusal; once the scheme has synced, the node's link state lists it. Having synced once, a scheme stays peered on across restarts, its ledger reachable or not.
- **Not on a scheme peers cannot pay.** While a scheme's ledger stops what peers pay us, as while an issuer froze our line deep, a peer's settlement would only fail, so the node leaves the scheme out as before its first sync, until it can receive again; peerings already made on it stay ([Settlement](settlement.md#when-peers-cannot-pay-us)).
- **Not on a stopped scheme.** A scheme whose effects the books refused stops until the node restarts, and refuses a peer's settlement meanwhile ([Settlement](settlement.md#the-scheme-interface)). So the node leaves it out as before its first sync: its link state no longer lists it, it asks for no peering on it, and refuses a request on it as unsupported. Peerings already made on it stay, and a repeat request on one is still answered with the stored peering; a restart lists it again.

- **Repeats.** A request from a node already peered on the same scheme is answered at once, without a probe, with the stored peering's version. This covers an acceptance that got lost.
- **Crossings.** When both ask at once, the lower node ID's request wins.

### Roles

The **junior** is the node whose peering request was accepted, the **senior** the node that accepted it; when requests cross, the lower node ID is the junior. The junior extends the senior a default credit line, the senior extends the junior none at first, and credit then grows with what a peer earned us ([Settlement](settlement.md#credit), [earned-trust](../decisions/earned-trust.md)).

## Lifecycle

A peering is `requesting`, `peered` or `terminating` ([peering-lifecycle](../decisions/peering-lifecycle.md)). "Peered" in other documents covers `peered` and `terminating`; "active" means `peered`.

**Withdrawing is prompt, ending is not.** A node takes a peer out of its link state moderately soon, weighing route flaps against stale routes that lose packets: a withdrawn peer stays peered, and comes back once it is healthy and its hold-down is over. Ending a peering is for good, and needs much stronger evidence: the owner's word, a peer silent or short of room for as long as a silent peer is kept, or a scheme the node no longer runs ([Termination](#termination)). Nothing that withdraws a peer ends it.

### Advertised peers

A node lists a peer as a neighbor in its link state, and routing uses the edge, only while the peer is `peered`, **confirmed** (an authenticated message from it arrived recently), not banned, not held down, and not withdrawn for room.

- **Hold-down.** Each time a peer stops being advertised it is held down, for a time that doubles with each flap and resets after a long stable stretch, so a flapping peer does not churn routes across the network.
- **Restarts** do not drop neighbors: confirmation starts from the persisted time the peer was last heard from, without a hold-down.
- **Withdrawal for room.** Withdrawing an edge removes it both ways for the whole network, turning a temporary T04 into a final F02, so neither a failed delivery nor a peer waiting seconds for a routine settlement withdraws it. A peer whose room in our books has stayed below one of the largest packets for the grace, without settling, is stuck: settling, not traffic, restores room, and a creditor asks over the peer protocol, which needs no route ([peering-lifecycle](../decisions/peering-lifecycle.md)). It is withdrawn like a flap: still peered and asked to settle, but no packet crosses the edge either way until its room is back and its hold-down is over. The grace lets two requests to settle go unanswered and still gives the third a request's timeout to be answered, and its settlement time to be booked. A request that surely never reached the peer costs none of it: it is asked again at the next check ([Settlement](settlement.md#asking-a-peer-to-settle)). A node counts only the room it gives a peer, never the room the peer gives it.
- **Downlinks owing nothing are not withdrawn.** A downlink is in no link state, so withdrawing it would only stop the payments to it, which are what raise its room when it owes nothing. One whose debt in our books is zero is never withdrawn or ended for room, however short its room; one that owes and does not settle is, as any peer.
- **Counting the time short.** Each settlement booked from the peer starts the grace again. The time short is a total, grown only while the node can ask (its scheme sees its ledger), by at most two check intervals per check, so neither the node's downtime, a stall of its host, nor an outage of its own ledger counts against the peer. Only the part of it during which the peer could be reached ends the peering: time during which the node's last request to settle surely never reached the peer still withdraws it, since the link carries nothing, but does not count toward ending it for room, as the peer had no chance to settle ([peering-lifecycle](../decisions/peering-lifecycle.md)). Nor does time during which no settlement from the peer could reach us, as while an issuer froze our line deep: it withdraws the peer, never ends it ([Settlement](settlement.md#when-peers-cannot-pay-us)). Both are persisted with the peering, so a restart neither forgives them nor starts them again.

### Termination

Termination starts when the owner asks, when the owner bans the node, when the peer has not been heard from for a long time (counted from the later of its last message and the node's start, so our own downtime ends nothing), when its room has stayed short, without a settlement, for as long while it could be reached ([Advertised peers](#advertised-peers)), or when the node does not run the peering's scheme, as one disabled or of the other realm ([Settlement](settlement.md#schemes-and-ledgers)). A peer cut off from the node is ended for its silence, if at all, never for room. A peering on a scheme the node does not run collects nothing, since no settlement on that scheme could be booked: the node tells the peer at once, as in step 3, giving no reason, so no cooldown follows; a peering it was already ending keeps the reason it had.

The node that ends a peering collects first, and tells the peer only then, unless its owner banned the peer or the node does not run the peering's scheme ([ending-a-peering](../decisions/ending-a-peering.md)):

1. **It decides privately.** Nothing the peer can see changes: a peer it advertised stays advertised, and our own Prepares still go to it. Its owner sees it terminating. Its new Prepares are refused, as for want of liquidity ([Connector](connector.md#responses-and-expiry)), and its requests to settle are answered as ones that settle nothing.
2. **It waits for its own settlements to the peer under way,** so the peer counts them; then asks the peer to settle, once. The answer says what the peer has on its way ([Settlement](settlement.md#settling-on-request)), and the node waits until that is booked. Each wait lasts at most a time a settlement takes to reach the ledger and be announced (`depeer_wait_ms`). A peer that does not answer is waited out.
3. **Then it tells the peer:** it stops advertising it, takes no new Prepares to or from it, and sends `peeringTermination` until it is acknowledged or times out. Responses still flow.
4. **The other side ends at once** when the message arrives, as the node does that is told first when both decided: it stops advertising and routing to the peer, takes no new Prepares and settles nothing.
5. Once the exchange is over, no packet is in flight with the peer and no response to it waits to be sent again, the peering ends. Neither side settles anything for the end ([Settlement](settlement.md#settling-at-a-peerings-end)): what the books show stays owed, for a later peering to settle.
6. After an owner's termination, or one for room, the two do not peer again for a while: neither asks the other, and each refuses the other's requests meanwhile ([ended-for-room](../decisions/ended-for-room.md)). A node collecting for its owner, or for room, keeps its reason when the peer tells it first. That cooldown does not survive a restart; a ban is the lasting way. After any other end, asking again waits only a backoff.

**Termination ends a peering, not a relationship.** The peer's accounts stay, with their balances, as the record of what is owed, and so does the settlement channel with its sequence, open until it has been idle for as long as a silent peer is kept, then closed to disk ([Settlement channels](#settlement-channels)). A settlement that arrives after the peering ended is still credited once, and a later peering with the node reuses both without posting trust twice. The channel also remembers that the node ended a peering for room, so a later one raises the peer no credit ([Settlement](settlement.md#credit)). A termination under way survives a restart, and so does a decision to end a peering; its steps start again after it.

### One-sided peerings

A node that holds no peering with an authenticated sender, and is not asking for one, answers any request from it that presumes a peering with a `peeringTermination` of reason "not peered", at most once per termination timeout per node. The request itself gets the answer it would get anyway, a refusal for not being peered, so no status changes meaning. The other side then ends its peering at once, as the node being ended does ([Termination](#termination)).

This repairs a peering whose `peeringTermination` never arrived while it was sent, as when the two were cut off for the whole termination timeout. The side left over would otherwise keep it until the peer had been silent for as long as a silent peer is kept, and, if the peer is short of room in its books, ask it to settle at every check, since a refusal for not being peered is also what a peer that has not taken in our acceptance yet answers ([peering-lifecycle](../decisions/peering-lifecycle.md)).

- **What presumes a peering:** a Prepare, a request to settle, a scheme's message, another node's link state forwarded to us, since floods go only to peers, a core node's own link state that lists us as a neighbor, and a periphery node's own link state. A core node's own link state that does not list us presumes nothing: it also sends it to the bootstrap nodes it is not peered with ([How link states spread](#how-link-states-spread)), even to one it is asking for a peering. A periphery node's lists nobody, but it sends it only to its uplinks, in heartbeats and when it issues a new one; to a node it asks, it goes inside the peering request.
- **A former downlink is told too.** A downlink leaves the table when its peering ends ([Downlinks](#downlinks)), so the node tells it through the settlement channel it keeps with it, open for as long as the periphery node would keep a silent uplink ([Settlement channels](#settlement-channels)). Otherwise a periphery node whose uplink ended their peering while the two were cut off would keep it, unconfirmed, until the uplink had been silent for as long as a silent peer is kept, and never ask it again meanwhile. Who was told lately is a bounded list.
- **Not while asking.** A node whose own peering request is under way answers nothing: the peer may have accepted it, and hold the peering before the acceptance arrives.

### Bans

- Bans are a bounded list the owner keeps. Only the owner bans; a node never bans a peer for what it cost ([own-fulfills](../decisions/own-fulfills.md)).
- A banned node is never asked, is refused, and its messages do not confirm it as a peer. A ban ends a peering as the owner's termination does, with its cooldown, but collects nothing: the node withdraws the peer from its link state and tells it at once (step 3 of [Termination](#termination)), without waiting for its own settlements or asking the peer to settle. What the peer owes stays on the books. A ban is the owner's emphatic word, often about misbehaviour, and a peer being banned has little reason to settle. Its link states still flood: a ban is one node's choice, and other nodes route by both ends' word.
- A ban is a list, not a peering state, since a node can be banned while its peering is still terminating.

## Settlement channels

Every peer gets a settlement channel, which belongs to the pair of nodes and outlives the peering ([Settlement](settlement.md#settlement-channels)). A node we hold an open one with keeps its replay state ([Peer protocol](peer-protocol.md#replay-protection)), and its Fulfills, Rejects and settlements are taken even with no peering. Open channels are bounded in number; once all are taken, a node without one is no candidate and its peering requests are refused for capacity. A channel idle for as long as a silent peer is kept after its peering ended closes: it leaves memory and frees its place, and what stays of it on disk still takes the node's settlements once and opens again with its next peering ([closing-channels](../decisions/closing-channels.md)).

## Periphery nodes

A node without a public address joins as a periphery node ([core-and-periphery](../decisions/core-and-periphery.md)): it connects only to core nodes, its uplinks, is reached only over the connections it opened to them, and forwards nothing. What else the [design](../designs/periphery.md) holds is not built yet ([roadmap](../roadmap.md#periphery)).

- **Who is one.** A node with a QUIC endpoint and no hostname, so no URL. A node with a URL is a core node, and a core node that takes QUIC takes downlinks. The owner's hostname must make a URL peers take, with any port, so a node never turns periphery by a hostname too long ([Node](node.md#configuration)).
- **Changing kind.** A core node that loses its URL ends the peerings with its downlinks, since a periphery node takes none, and asks for uplinks. A peer whose link state turns from core to periphery, or back, is a peer of the other kind, with another address and other bounds: the peering made as the old kind ends, and it may ask anew.
- **Its link state** has no URL and the periphery extension ([Peer protocol](peer-protocol.md#link-states)), and lists no neighbors: only core nodes list theirs, since routing needs both ends of an edge and no node routes to a periphery node ([periphery-lists-no-neighbors](../decisions/periphery-lists-no-neighbors.md)). So its size does not grow with its uplinks. Only its uplinks see it, in its peering requests and heartbeats: it is never forwarded or served, nor registered, so it is in no node list, and in no table but its uplinks'.
- **Peering.** It asks core nodes from the node list whose link states say they take QUIC, as junior ([Roles](#roles)), until it has the usual target of confirmed peers, or only the nodes it is pinned to ([below](#pinned-uplinks)). It accepts no peering request, and is never asked: no core node asks a periphery node. It seeks no bridge.
- **Routes** go through its uplinks, which never list it: it reaches whatever they reach.
- **Its addresses** lie beneath its uplinks', and it forwards nothing ([Connector](connector.md#a-periphery-node)).

### Pinned uplinks

An owner may pin a periphery node's uplinks, by node ID (`env.rs`), when it must reach the network through particular core nodes: a market maker's nodes trade through the core node whose rates they watch, and a node that peered with any other would route some of its payments elsewhere.

- **Only those are asked,** each still under every rule of [Looking for peers](#looking-for-peers): fresh, taking QUIC, not banned, not in backoff. A pinned node the node does not know, or cannot ask, is waited for; no other is asked meanwhile, so a pin that names no reachable core node leaves the node without an uplink rather than with one the owner did not choose.
- **The target is as many as it names,** and only peerings with them count toward it. A peering made before with another node stays, as one on a scheme left out does, until the owner ends it: it is not counted, and takes none of the places for peerings the node starts. Its routes still serve, so an owner who wants every payment through the pinned nodes ends it.
- **Slots of their own.** A pinned node starts as many peerings as it pins, rather than the configured cap, still one attempt at a time, so every pinned node can be asked however many there are. Its core peers together, the pinned ones and those made before, stay within the larger of `MAX_UPLINKS` and the configured total; only a node pinned to nearly that many, with peerings from before, waits for the owner to end one before it asks the last. That bound is what every node's tables are sized for: its connector's addresses and packet table, its ledger and its settlement channels (`check_bounds` in `crates/dassie-node/src/engine/core/mod.rs`). An unpinned node, and every core node, keeps the configured slots.
- **Bounded** by `MAX_UPLINKS` in `crates/dassie-peer/src/node/mod.rs`, high enough that a market maker's node can trade through many core nodes: a longer list stops the node at start ([Node](node.md#startup)).
- **Only a periphery node uses the list.** A core node finds its peers itself, and warns at start that it ignores the list; one that turns periphery later uses it from then on, as a periphery node that turns core stops.

### Downlinks

A core node keeps its downlinks, the periphery nodes peered with it, apart from its peerings with core nodes.

- **Taken without a probe.** A periphery node's peering request is checked as any other's, but its URL is not probed: it has none, and the connection its request came on shows it is there. It is in no node list, so it enters the table by asking.
- **A bound of their own** (`max_downlinks` in `crates/dassie-peer/src/config.rs`) on their places in the table, their peerings and their settlement channels, apart from the core peering slots, the table's other nodes and the channels with core nodes. A channel closes once idle after its peering ended ([Settlement channels](#settlement-channels)), so it bounds the downlinks peered with, or not long ago. Past it, a new downlink is refused for capacity. A downlink took no place among the table's other nodes, so one that is refused, or whose peering ends, stays in the table only if it gets such a place: within their bound, or, if another reason keeps it, in place of a weaker node. Otherwise it leaves at once, and with it the wait before it may ask again after a termination; its channel stays, and with it its replay state. An owner who wants it kept away bans it.
- **In nobody's routes.** A downlink is never listed in our link state, so no route leads to it: its child address lies beneath ours, which the rest of the network routes to us, and the connector passes it on ([Connector](connector.md#addresses-and-endpoints)).
- **Reached only over its connection** ([Peer protocol](peer-protocol.md#quic)), each with its own bound on requests in flight rather than one for all nodes without a URL, and never withdrawn for room while it owes nothing ([Advertised peers](#advertised-peers)).
- **Places of their own for its connections.** A downlink's QUIC connections hold places among the downlinks', which have room for every downlink within the bound, apart from core peers' and everyone else's: however many downlinks a node keeps, its core peers and relayed streams keep their room ([Peer protocol](peer-protocol.md#quic)).

## Routing

Routing runs a breadth-first search from this node over edges that both ends advertise and whose link states are fresh.

- An edge counts only if both ends list each other, so no node can claim a link the other end does not confirm, and a node whose state went stale drops out.
- Each reachable node's address gets a few first hops on shortest paths. There is no route for the node's own address: destinations under it go only to its own endpoints. Prefix matching, first-hop choice and never sending a packet back where it came from are the connector's ([Connector](connector.md#routing)).
- Routes are recomputed when the graph may have changed, debounced, and periodically as states go stale.
- A periphery node routes through its advertised uplinks, whatever they list, and recomputes its routes when one is withdrawn or comes back; no route leads to a periphery node ([Periphery nodes](#periphery-nodes)).

## Persistence

The peer node's state is written in the same transaction as the ledger, on every change (`crates/dassie-peer/src/persist.rs`, `crates/dassie-node/src/store/peer.rs`). What survives a restart is chosen so that a restart neither drops a peer nor forgives one: peerings with when each was last heard from and how long its room has been short, peerings being ended (decided or told), replay floors, the own sequence, the majority, settlement channels (with whether a peering with the node was ended for room), open or closed, bans, at a registrar its log's head, the commitments in its log with their unsealings, and its names ([Names](#names)), and every node's view of who holds each name ([Holders](#holders)). Only open channels are loaded at start; a closed one is read when it is needed. Link states were verified when received and are not verified again.

What is lost is either fetched again (the bootstrap nodes' node and name lists) or only paces the node's own attempts: backoffs, flood counters, hold-downs, the cooldown after a termination, how long each channel has been idle, a registrar's bucket of new commitments, and how long each name it lists has been split. A peering attempt cut short by a restart is dropped.

## Tests

The `peer` simulation (`dassie_peer::sim::chaos`) runs this protocol on a rough network with crashes, partitions, terminations and bans, and checks that link states and routes converge, peerings are two-sided, every termination finishes, and each settlement is applied exactly once ([Testing](testing.md#the-simulations)).

A registrar takes commitments, unsealings and renewals over the protocol on the same simulated network in `crates/dassie-peer/tests/names_registrar.rs`: the commitment logged first wins, a receipted commitment survives a crash, names expire unless renewed, and commitments are throttled. In `crates/dassie-peer/tests/names_holders.rs`, nodes take holders from four or five registrars: a view freezes without a quorum of fresh lists, across a restart too; a faulty registrar that shows nodes different lists after an honest split never gives a name two holders; a split of two to two is released at all four, and no sooner than the wait; and neither a holder with a quorum, with one registrar faulty or out of reach, nor a registration whose lists are merely late, is released.

Periphery nodes peer with their uplinks on the same simulated network in `crates/dassie-peer/tests/periphery.rs`, pinned ones too, to more uplinks than a link state could list; `crates/dassie-node/tests/periphery.rs` has a periphery daemon peer with a core daemon over QUIC and get paid beneath it, one pinned to a core node ask only that one, and one pinned to two get paid beneath each, as its owner names it.

## Decisions

- [peer-protocol-v1](../decisions/peer-protocol-v1.md): v1 instead of v0: authenticated senders and responses, peering requests that carry the signed node info, termination, scheme versions.
- [membership-economics](../decisions/membership-economics.md): removal and expiry, automatic registration, the served list, the membership freeze, authenticated lists, and the limits on free registration until fees.
- [peering-lifecycle](../decisions/peering-lifecycle.md): the peering lifecycle, confirmed peers, the bridge rule, slot caps, repeats and crossings, the advertise flag and withdrawal for room, termination, bans as a list, the channel limit, and why child peers wait.
- [bootstrap-network](../decisions/bootstrap-network.md): the testnet's bootstrap nodes as the default list.
- [earned-trust](../decisions/earned-trust.md): junior and senior.
- [own-fulfills](../decisions/own-fulfills.md): only the owner bans.
- [synced-before-peering](../decisions/synced-before-peering.md): no peering on a scheme before its first sync.
- [ended-for-room](../decisions/ended-for-room.md): a pair parted for room stays apart for the cooldown, and earns no credit back.
- [closing-channels](../decisions/closing-channels.md): a settlement channel left idle closes to disk, freeing its place.
- [core-and-periphery](../decisions/core-and-periphery.md): core and periphery; why periphery nodes are in no node list and no link state.
- [places-by-proved-key](../decisions/places-by-proved-key.md): downlinks' connections hold places of their own.
- [periphery-lists-no-neighbors](../decisions/periphery-lists-no-neighbors.md): a periphery node's link state lists no neighbors, so it may keep many uplinks.
- [settlement-methods](../decisions/settlement-methods.md): a peering settling by every scheme both run (not built).
- [names-by-earliest-commitment](../decisions/names-by-earliest-commitment.md): a name goes to its earliest sealed commitment at each registrar, and is held by a quorum of their lists.
