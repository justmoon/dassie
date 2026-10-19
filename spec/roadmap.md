# Roadmap

What is not built yet: the open areas of work, the gaps between the spec's contract and `main`, the conditions for real money, smaller open points, and what stays future work. Everything else in the spec describes what is on `main`; an area doc that meets one of these gaps says so in one line and links here.

**New work is an open area of work here, never a new numbered milestone.** Areas proceed in parallel, and each says what done means. When an area is built, its behaviour moves into the area docs and its entry here goes.

## Open areas of work

### Settlement hubs

Another way to peer, beside bilateral peering: a ledger kept by n = 3f + 1 validators and backed by an escrow on the XRP Ledger, so that two nodes peered over a hub cannot walk away with what they owe each other. The design is [designs/settlement-hubs.md](designs/settlement-hubs.md); the reasons are [D22](decisions/D22-settlement-hubs.md).

- **What it covers,** in parts that can be built side by side:
  - `dassie-hub`: the hub's ledger, its operations and Simplex consensus, in the simulation without an escrow;
  - the `hub` settlement scheme: holds, commitments and claims, peering over a hub, joining by being paid, against the hub's ledger in the simulation;
  - the escrow on the XRP Ledger testnet: founding, deposits, withdrawals, changing validators, shutdown and payout, against the fake XRP Ledger.
- **Done when:** in the simulation, with up to f faulty validators, members peered over a hub pay each other across hops without trusting each other: a payer that sends no claims, and a payee that claims twice or late, cost the other no more than its hold. f + 1 colluding validators leave evidence. A hub that loses a validator shuts down, and the escrow pays out exactly the final balances, with signatures collected over time.
- Hubs hold real money only once the conditions of [Real money](#real-money) are met.

### Continuous simulation

The simulations run continuously, outside CI, from random seeds; each seed draws its own configuration, and a failure becomes an issue that names its seed ([D27](decisions/D27-simulations-run-continuously-from-random-seeds.md)). On `main` so far: the `dassie-sim` runner, with `list`, `run` and `explain`, and the node, peer, connector and STREAM simulations as library code in their crates' `sim` modules, run by `cargo test` on fixed seeds and by the runner on any ([Testing](areas/testing.md#the-runner-dassie-sim)). The node simulation has a run drawn from its seed, `node`: its nodes, settling, latency, loss, crashes (biased toward the ends of unfinished payments), cuts, ledger-link drops and payments, any fault off in some seeds; the network forms, goes through chaos, heals, and must settle down within a bound derived from the production timings; and it counts what it reached, with minimums over fixed seeds in CI ([Testing](areas/testing.md#the-simulations)). Nothing runs them continuously yet, and the release workflow does not consult them: any tag `v<version>` is built and published once its install tests pass ([Release](areas/release.md#publishing)). Not built, in parts that can be built side by side:

- **Swarm testing** in the peer, connector and STREAM simulations, and what the node simulation does not draw yet: duplication, clock skew, lying peers, owner terminations, runs across currencies, and a fixed realistic profile. Before a run starts, its seed draws the whole configuration, and the run prints it first:
  - the number of nodes;
  - latency, loss, duplication and partitions;
  - crashes, with a bias toward crashes while payments are in flight;
  - clock skew;
  - faults of the link to the ledger;
  - the share of lying peers;
  - the payment workload.

  Some seeds switch a fault off entirely, so rare combinations get their turn. A fixed realistic profile runs alongside.
- **Two phases** in the peer, connector and STREAM simulations, and in the node simulation the checks during the chaos (it checks the books once, after the heal). A run first has a stretch of chaos, with every invariant and every per-node check of [Testing](areas/testing.md#each-node-judged-by-its-own-books) checked throughout. Then it heals: crashed nodes restart, partitions close and faults stop. Within a time derived from the production timings, payments must then complete, settlements arrive, routes converge, and no node may owe an honest peer past that peer's trigger. Lying peers are judged only by what they cost the honest ones. A run that cannot converge whatever the code does is unsupported.
- **New faults,** each on its own: duplication and clock skew in the node simulation, long partitions, and failed commits. Faults stay within what the node relies on: the store commits with `synchronous = FULL`, so a commit that returned is on disk, and a torn write is SQLite's to handle. The only fault injected at the store is a commit that fails; the node must stop, then restart from its last good commit.
- **Coverage** in the peer, connector and STREAM simulations, as the node simulation has it. Each run counts what it reached: crashes during a payment, bans, settlements, withdrawals, partitions healed. A test in CI asserts a minimum for each count over fixed seeds, so that a generator cannot quietly stop reaching a case. CI otherwise keeps its fixed seeds.
- **The campaign** on spot VMs in the testnet's GCP project. A loop on each VM fetches `main`, builds the runner, runs seeds on every core for an hour, reports, and starts again; so changing the runner on `main` changes what every VM runs. Open PRs labelled `sim` get a share of the time. The VMs run unreviewed code from pull requests in the project that holds the bootstrap nodes' phrases, so they run as a service account of their own, with no role in the project, that can read one secret, the GitHub token, and nothing else.
- **Failures become issues.** A failure's signature is its simulation, its outcome, and the assertion or check that failed. Each signature seen on `main` gets one GitHub issue labelled `sim-failure`, holding the commit, the configuration, the end of the trace, and the commands that reproduce it with the seed that failed fastest. Later seeds with the same signature are added to that issue. A failure on a pull request's branch is a comment on that pull request instead. A fix comes with a targeted test, because a seed reproduces a failure only at the commit it ran on.
- **The tripwire.** A `tripwire` simulation fails on purpose once in a thousand seeds and files under its own label, `sim-tripwire`, so it never holds up a release. If its issue stops getting new seeds, the campaign has stopped.
- **The release check.** The release workflow's first step refuses to build, sign or publish a tagged commit on which an open `sim-failure` issue was seen, or on which the campaign has not yet run long enough. A refused tag is deleted, and the version is tagged again once the campaign has cleared a commit.

**Done when:** the campaign runs unattended on `main`, files one issue per failure signature with a command that reproduces it, and its tripwire keeps showing up. CI asserts the coverage minimums, and the release workflow refuses a commit the campaign has not cleared.

**Not yet decided:** coverage-guided fuzzing (cargo-fuzz) of the decoders that face untrusted input, real binaries behind a proxy that injects faults, and shrinking failing runs. Fuzzing never runs in CI.

### Registration fees and child peers

Registration is free today, and throttled instead ([Network](areas/network.md#registration)). The design calls for a paid registration whose fee grows with the size of the list ([D4](decisions/D04-network-membership-economics.md)).

- **What it covers:**
  - **The fee quote and payment,** as a v1.x peer-protocol addition: a quote message, and payment by ILP with a signed quote condition (the "naive" variant of the token payment protocol, [5 §9](original/05-accounting-and-settlement.md)), to the cheapest 75% of bootstrap nodes (the original's UI said 80%; the wallet follows the 75%).
  - **The fee schedule,** set when the quote and payment are built.
  - **Child peers** ([D12](decisions/D12-peering-lifecycle-and-liveness.md)): non-majority nodes peer as children, routed beneath their uplink's address, which touches the connector, BTP and the STREAM receiver. They matter only once registration costs something: today any node with a URL registers and becomes a member within minutes. A child's Network graph shows the child itself and its links to its uplinks, although no route reaches it.
- **Done when:** a node registers by paying the quoted fee, and is refused without it. A non-majority node peers as a child, gets an address beneath its uplink, and pays and is paid through it.

## Not built yet

Gaps between what the spec's contract or decisions ask for and what `main` does. Each is a known gap, not a milestone; each can be picked up on its own.

- **ILP-over-HTTP** (RFC 35, with its synchronous mode) is in the compatibility contract ([overview](overview.md#compatibility-with-the-original)), but the node serves no `/ilp` route: the HTTPS route table has none, so a `POST /ilp` gets 405 ([HTTP](areas/http.md#route-table), [Connector](areas/connector.md#ilp-over-http)). **Done when** an RFC 35 client, synchronous mode included, pays through the node over `/ilp`.
- **Hybrid post-quantum TLS.** Confidentiality against "harvest now, decrypt later" belongs to TLS with the hybrid key exchange X25519MLKEM768 ([D2](decisions/D02-peer-protocol-v0-or-v1.md)). The node's TLS (`crates/dassie-node/src/tls.rs`) uses rustls with ring as its only crypto provider, which offers no ML-KEM, so its handshakes use classical key exchange only (X25519 and the NIST curves) ([HTTP](areas/http.md#certificates)). **Done when** the node's listeners and its clients (peer client, ACME) offer and prefer X25519MLKEM768, with a crypto provider that still builds the static musl binaries.
- **Per-ledger credit line and largest packet** ([D6](decisions/D06-owner-ledger-and-multi-currency-accounts.md)). The default credit line (1.1 units) and the largest packet (0.1 units), and with it every threshold counted in it, are one value for the whole node ([Connector](areas/connector.md#bounds), [Settlement](areas/settlement.md#credit)). **Done when** each ledger is configured with its own, and the thresholds and the junior's starting credit follow the ledger's.
- **Draining in-flight packets on shutdown** (RT-7). On SIGTERM the engine stops at once when it is cancelled; it does not wait for the packets in flight to be answered or to expire. The other services get at most 15 s in all, and HTTP requests in progress at most 10 s ([Node](areas/node.md#shutdown)). A packet in flight at the stop gets no answer from the node. Its pending transfers were never on disk, so after the restart the books stand as if it had been rejected, and a Fulfill that came back is lost, as in a crash; the cap on money in flight with a peer bounds what that costs ([Connector](areas/connector.md#money-in-flight-with-a-peer)). **Done when** the node stops taking new Prepares on SIGTERM and waits, up to the latest expiry among them, for the ones in flight before it closes the database.
- **The 512-channel limit.** A node never closes a peer's accounts or settlement channel, and keeps at most 512 channels, one per node it has ever peered with; after that, peering with a new node is refused for capacity ([Network](areas/network.md#settlement-channels), [D12](decisions/D12-peering-lifecycle-and-liveness.md)). **Done when** a relationship whose peering has ended and whose balance is settled can be closed, freeing its channel, without a late settlement being lost or credited twice.
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

- **A later peer-protocol version** drops the `credit` field of `peeringRequest` and of its response (sent as 0 and ignored), and the one-byte body of the reply to a Fulfill (always `accepted`, for nodes of 0.1) ([D23](decisions/D23-each-node-books-its-own-fulfills.md)).
- **A post-quantum peer-protocol scheme.** The authentication choice reserves tag `[2]` for a scheme on the X-Wing hybrid KEM, and a link state's extension type 1 for its key; neither is built ([Peer protocol](areas/peer-protocol.md#envelope), [D2](decisions/D02-peer-protocol-v0-or-v1.md)). Signatures and node IDs stay Ed25519.
- **Credit growth** ([D20](decisions/D20-trust-follows-what-a-peer-earned-us.md)): the share k (10%) and the cap (a hundred of the largest packets) are node constants; owner settings may follow. The spread's gain is not booked per packet and does not count toward growth yet.
- **Importing a v0 node's config and BTP tokens** is not planned; it is cheap to add if anyone asks ([D3](decisions/D03-database-continuity.md)).

## Future work, design only

The original's designs that stay future work. The architecture must not preclude them.

- **The token payment protocol** ([5 §9](original/05-accounting-and-settlement.md)).
- **A private UDP peer transport** ([3 §18](original/03-peer-network.md)).
- **Signed or independent frontends,** so that the wallet need not trust the node it is served by ([2 §8.8](original/02-node-runtime.md), [D10](decisions/D10-secrets-at-rest.md)).

## How the rewrite was built

The rewrite was first built in this order. The milestones are history: commits, decisions and older discussions refer to them, and no new ones are added.

| # | What it delivered |
|---|---|
| M1 | The core crates: `dassie-oer`, `dassie-identity`, `dassie-ilp`, `dassie-stream`, and their WASM builds |
| M2 | The node skeleton: config, database, listeners, TLS, ACME, setup and login, the RPC contract, `dassie init`, `daemon` and `verify-install` |
| M3 | The ledger journal, the connector, local endpoints, ILDCP, BTP, the stub scheme, exchange-rate math |
| M4 | Peer protocol v1, membership and registration, peering, link states, routing (M4a); the settlement loop and the peering lifecycle (M4b) |
| M5 | The node's live streams and the wallet (M5a); the dev environment and dashboard (M5b) |
| M6 | Open Payments: wallet addresses and incoming payments (M6a); quotes, outgoing payments and consent (M6b); Send and payment pointers (M6c) |
| M7 | The owner's group taking in gains and losses (M7a); the XRPL testnet scheme (M7b); the rate fetcher, fees and spread (M7c) |
| M8 | Signed releases built by CI (M8a); the systemd units, installer and updater (M8b) |
| M9 | The website and STREAM toy |
| M10 | Junior and senior, thresholds in the largest packet (M10a); credit growing with earnings (M10b) |

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

## Original

- [2 §2.6, §20](original/02-node-runtime.md): the original's shutdown and its gap RT-7.
- [4 §8](original/04-interledger.md): the original's ILP-over-HTTP stub.
- [5 §9](original/05-accounting-and-settlement.md): the token payment protocol's design.
- [3 §18](original/03-peer-network.md): the membership and private-transport design notes.
