# Roadmap

What is not built yet: the open areas of work, the gaps between the spec's contract and `main`, the conditions for real money, smaller open points, and what stays future work. Everything else in the spec describes what is on `main`; an area doc that meets one of these gaps says so in one line and links here.

**New work is an open area of work here, never a numbered milestone.** Areas proceed in parallel, and each says what done means. When an area is built, its behaviour moves into the area docs and its entry here goes.

## Open areas of work

### Settlement hubs

Another way to peer, beside bilateral peering: a ledger kept by n = 3f + 1 validators and backed by an escrow on the XRP Ledger, so that two nodes peered over a hub cannot walk away with what they owe each other. The design is [designs/settlement-hubs.md](designs/settlement-hubs.md); the reasons are [D22](decisions/D22-settlement-hubs.md).

- **Parts that can be built side by side:**
  - `dassie-hub`: the hub's ledger, its operations and Simplex consensus, in the simulation without an escrow;
  - the `hub` settlement scheme: holds, commitments and claims, peering over a hub, joining by being paid, against the hub's ledger in the simulation;
  - the escrow on the XRP Ledger testnet: founding, deposits, withdrawals, changing validators, shutdown and payout, against the fake XRP Ledger.
- **Done when:** in the simulation, with up to f faulty validators, members peered over a hub pay each other across hops without trusting each other: a payer that sends no claims, and a payee that claims twice or late, cost the other no more than its hold. f + 1 colluding validators leave evidence. A hub that loses a validator shuts down, and the escrow pays out exactly the final balances, with signatures collected over time.
- Hubs hold real money only once the conditions of [Real money](#real-money) are met.

### Continuous simulation

The simulations are to run continuously, outside CI, from random seeds; each seed draws its own configuration, and a failure becomes an issue that names its seed ([D27](decisions/D27-simulations-run-continuously-from-random-seeds.md)). The runner and the simulations exist ([Testing](areas/testing.md#the-simulations)); nothing runs them continuously yet, and the release workflow does not consult them. Not built, in parts that can be built side by side:

- **Swarm testing** in the peer, connector and STREAM simulations, as the `node` simulation has it: before a run starts, its seed draws the whole configuration (nodes, latency, loss, duplication, partitions, crashes biased toward payments in flight, clock skew, faults of the ledger link, the share of lying peers, the workload), and the run prints it first. Some seeds switch a fault off entirely, so rare combinations get their turn; a fixed realistic profile runs alongside. The `node` simulation does not yet draw duplication, clock skew, lying peers, owner terminations or runs across currencies.
- **Two phases** in the peer, connector and STREAM simulations, and in the `node` simulation the checks during the chaos (it checks the books once, after the heal). A run first has a stretch of chaos, with every invariant and every [per-node check](areas/testing.md#each-node-judged-by-its-own-books) checked throughout. Then it heals, and must [settle down in production time](areas/testing.md#simulation-time-follows-production). Lying peers are judged only by what they cost the honest ones.
- **New faults,** each on its own: duplication and clock skew in the `node` simulation, long partitions, and failed commits. Faults stay within what the node relies on: the store commits with `synchronous = FULL`, so a commit that returned is on disk, and a torn write is SQLite's to handle. The only fault injected at the store is a commit that fails; the node must stop, then restart from its last good commit.
- **Coverage** in the peer, connector and STREAM simulations, as the `node` simulation has it: each run counts what it reached, and a test in CI asserts a minimum for each count over fixed seeds, so that a generator cannot quietly stop reaching a case.
- **The campaign** on spot VMs. A loop on each VM fetches `main`, builds the runner, runs seeds on every core for a while, reports, and starts again, so changing the runner on `main` changes what every VM runs. Open PRs labelled `sim` get a share of the time. The VMs run unreviewed code from pull requests in the project that holds the bootstrap nodes' phrases, so they run as a service account of their own, with no role in the project, that can read one secret, the GitHub token, and nothing else.
- **Failures become issues.** A failure's signature is its simulation, its outcome, and the assertion or check that failed. Each signature seen on `main` gets one GitHub issue labelled `sim-failure`, holding the commit, the configuration, the end of the trace, and the commands that reproduce it with the seed that failed fastest; later seeds with the same signature are added to it. A failure on a pull request's branch is a comment on that pull request instead. A fix comes with a targeted test, because a seed reproduces a failure only at the commit it ran on.
- **The tripwire.** A `tripwire` simulation fails on purpose on a small share of seeds and files under its own label, `sim-tripwire`, so it never holds up a release. If its issue stops getting new seeds, the campaign has stopped.
- **The release check.** The release workflow's first step refuses to build, sign or publish a tagged commit on which an open `sim-failure` issue was seen, or on which the campaign has not yet run long enough. A refused tag is deleted, and the version is tagged again once the campaign has cleared a commit.

**Done when:** the campaign runs unattended on `main`, files one issue per failure signature with a command that reproduces it, and its tripwire keeps showing up. CI asserts the coverage minimums, and the release workflow refuses a commit the campaign has not cleared.

**Not yet decided:** coverage-guided fuzzing of the decoders that face untrusted input (outside CI, [Engineering](engineering.md#no-fuzzing-or-random-seeds-in-ci)), real binaries behind a proxy that injects faults, and shrinking failing runs.

### Ending a peering

Today both sides of a termination settle everything they owe once the exchange completes ([Settlement](areas/settlement.md#settling-at-a-peerings-end)). Under [D28](decisions/D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md), the node that ends a peering collects first and pays nothing, and the node being ended stops at once.

- **What it covers:**
  - **A private depeering state** on the side that ends a peering, kept across restarts. In it, the node refuses the peer's new Prepares (as for a peer short of room) and settles nothing the peer asks for, answering both as usual. It still advertises the peer and sends its own Prepares through it.
  - **The sequence before `peeringTermination`:**
    1. wait for its own settlements to the peer that are under way, for at most the same time as step 3;
    2. ask the peer to settle once;
    3. wait until what the peer has on its way is booked, or a time derived from how long a settlement takes to reach the ledger and be announced.
  - **An amount in the answer to `settlementRequest`:** what the peer has on its way, whether a settlement already under way or the one it makes now; 0 for nothing. An answer without one, from an older node, gets the whole wait.
  - **The node being ended** ends the peering on `peeringTermination` at once. It stops advertising and routing to the peer, takes no new Prepares and answers no settlement request. It lets Fulfills and Rejects of packets in flight finish both ways, and settles nothing.
  - **A settlement after the end** is credited to the peer's account once, as today, and its message is acknowledged.
  - **Both sides ending at once:** the first `peeringTermination` to arrive makes its recipient the node being ended.
  - **Every way a peering ends** follows this: the owner, a ban, silence, a peer short of room. Each step that needs the peer times out when it does not answer.
  - **The simulations' checks:**
    - the node that ended a peering may still owe at the end, and the node ended loses no more than the credit it extended;
    - in the scripted run that ends a peering, a settlement that crosses the end is not lost.
- **Done when:** a node ends a peering by collecting from the peer first and paying nothing. The peer, told only by `peeringTermination`, stops at once and pays nothing. The `node` simulation's runs and the scripted termination show each node's books as D28 says.

### Registration fees and child peers

Registration is free, and throttled instead ([Network](areas/network.md#registration)). The design calls for a paid registration whose fee grows with the size of the list ([D4](decisions/D04-network-membership-economics.md)).

- **What it covers:**
  - **The fee quote and payment,** as a v1.x peer-protocol addition: a quote message, and payment by ILP with a signed quote condition (the "naive" variant of the [token payment protocol](original/05-accounting-and-settlement.md)), to the cheapest 75% of bootstrap nodes.
  - **The fee schedule,** set when the quote and payment are built.
  - **Child peers** ([D12](decisions/D12-peering-lifecycle-and-liveness.md)): non-majority nodes peer as children, routed beneath their uplink's address, which touches the connector, BTP and the STREAM receiver. They matter only once registration costs something: until then any node with a URL registers and becomes a member within minutes. A child's Network graph shows the child itself and its links to its uplinks, although no route reaches it.
- **Done when:** a node registers by paying the quoted fee, and is refused without it. A non-majority node peers as a child, gets an address beneath its uplink, and pays and is paid through it.

## Not built yet

Gaps between what the spec's contract or decisions ask for and what `main` does. Each can be picked up on its own.

- **ILP-over-HTTP** (RFC 35, with its synchronous mode): the node serves no `/ilp` route ([Connector](areas/connector.md#ilp-over-http)). **Done when** an RFC 35 client, synchronous mode included, pays through the node over `/ilp`.
- **Hybrid post-quantum TLS.** Confidentiality against "harvest now, decrypt later" belongs to TLS with the hybrid key exchange X25519MLKEM768 ([D2](decisions/D02-peer-protocol-v0-or-v1.md)). The node's TLS uses rustls with ring as its only crypto provider, which offers no ML-KEM, so its handshakes use classical key exchange only ([HTTP](areas/http.md#certificates)). **Done when** the node's listeners and its clients (peer client, ACME) offer and prefer X25519MLKEM768, with a crypto provider that still builds the static musl binaries.
- **Per-ledger credit line and largest packet** ([D6](decisions/D06-owner-ledger-and-multi-currency-accounts.md)). Both, and with the largest packet every threshold counted in it, are one value for the whole node ([Connector](areas/connector.md#the-largest-packet), [Settlement](areas/settlement.md#credit)). **Done when** each ledger is configured with its own, and the thresholds and the junior's starting credit follow the ledger's.
- **Draining in-flight packets on shutdown.** On SIGTERM the engine stops at once; it does not wait for the packets in flight to be answered or to expire ([Node](areas/node.md#shutdown)). Their pending transfers were never on disk, so after the restart the books stand as if they had been rejected, and a Fulfill that came back is lost, as in a crash; the cap on money in flight with a peer bounds what that costs ([Connector](areas/connector.md#money-in-flight-with-a-peer)). **Done when** the node stops taking new Prepares on SIGTERM and waits, up to the latest expiry among them, for the ones in flight before it closes the database.
- **The channel limit.** A node never closes a peer's accounts or settlement channel, and keeps a bounded number of channels, one per node it has ever peered with; once they are taken, peering with a new node is refused for capacity ([Settlement](areas/settlement.md), [D12](decisions/D12-peering-lifecycle-and-liveness.md)). **Done when** a relationship whose peering has ended and whose balance is settled can be closed, freeing its channel, without a late settlement being lost or credited twice.
- **Child peers and registration fees:** see [Registration fees and child peers](#registration-fees-and-child-peers).
- **The rest of D27:** see [Continuous simulation](#continuous-simulation), including the release check.

## Real money

The `live` realm is refused, and the XRP Ledger mainnet has no scheme ([D8](decisions/D08-real-money.md), [XRP Ledger](areas/xrpl.md#real-money)). `live` is enabled only when all of these hold:

- the peer protocol's sender authentication is in place ([Peer protocol](areas/peer-protocol.md));
- the ledger is durable: journal and reconciliation ([Ledger](areas/ledger.md));
- XRP Ledger reserve and result handling is complete: the reserve is never booked, an account's creation is a deposit, whole drops with fees booked, results checked with a failure's fee booked and the sender checked ([XRP Ledger](areas/xrpl.md));
- releases are signed ([Release](areas/release.md));
- there has been an external review.

The first four are on `main`; the external review has not happened. Mainnet also needs a scheme of its own, with its own memo type, versioned with the peer protocol, and a mainnet bootstrap node list must exist before `live` makes sense. Until then, the testnet scheme stops unless its server reports the testnet's network ID, so a mainnet server never gets XRP booked or signed for with a testnet wallet. Settlement hubs hold real money only under the same conditions.

## Smaller open points

- **A later peer-protocol version** drops the `credit` field of `peeringRequest` and of its response (sent as 0 and ignored), and the one-byte body of the reply to a Fulfill (always `accepted`, kept for older nodes) ([D23](decisions/D23-each-node-books-its-own-fulfills.md)).
- **A post-quantum peer-protocol scheme.** The authentication choice reserves a tag for a scheme on the X-Wing hybrid KEM, and a link-state extension type for its key; neither is built ([Peer protocol](areas/peer-protocol.md#versions), [D2](decisions/D02-peer-protocol-v0-or-v1.md)). Signatures and node IDs stay Ed25519.
- **Credit growth** ([D20](decisions/D20-trust-follows-what-a-peer-earned-us.md)): its share and cap are node constants; owner settings may follow. The spread's gain is not booked per packet and does not count toward growth yet.
- **Importing a v0 node's config and BTP tokens** is not planned; it is cheap to add if anyone asks ([D3](decisions/D03-database-continuity.md)).

## Future work, design only

Designs that stay future work. The architecture must not preclude them.

- **The token payment protocol** ([original 5 §9](original/05-accounting-and-settlement.md)).
- **A private UDP peer transport** ([original 3 §18](original/03-peer-network.md)).
- **Signed or independent frontends,** so that the wallet need not trust the node it is served by ([original 2 §8.8](original/02-node-runtime.md), [D10](decisions/D10-secrets-at-rest.md)).

## Decisions

- [D2](decisions/D02-peer-protocol-v0-or-v1.md): the post-quantum path: X-Wing for the peer protocol, hybrid TLS for confidentiality.
- [D3](decisions/D03-database-continuity.md): no import of v0 data for now.
- [D4](decisions/D04-network-membership-economics.md): registration fees and why registration is free and throttled meanwhile.
- [D6](decisions/D06-owner-ledger-and-multi-currency-accounts.md): per-ledger credit line and largest packet.
- [D8](decisions/D08-real-money.md): the conditions for real money.
- [D10](decisions/D10-secrets-at-rest.md): frontend signing stays future work.
- [D12](decisions/D12-peering-lifecycle-and-liveness.md): child peers deferred; the channel limit.
- [D20](decisions/D20-trust-follows-what-a-peer-earned-us.md), [D23](decisions/D23-each-node-books-its-own-fulfills.md): credit growth's constants; the fields a later protocol version drops.
- [D22](decisions/D22-settlement-hubs.md): settlement hubs.
- [D27](decisions/D27-simulations-run-continuously-from-random-seeds.md): the simulation campaign.
- [D28](decisions/D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md): ending a peering by collecting first and paying nothing.
