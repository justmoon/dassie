# Testing

How Dassie is tested: what each level of tests is for, what the deterministic simulations judge and why, how to reproduce a failure, and the campaign that runs the simulations continuously.

**Paths:** `crates/*/tests/`, `crates/*/src/sim/`, `crates/dassie-sim/`, `scripts/sim-campaign.ts`, `scripts/sim-campaign/`, `crates/dassie-node-core/src/testing.rs`, `tests/e2e/`, `tests/interop/`, `testdata/`, `**/*_test.ts`
**Read with:** [Engineering](../engineering.md), [Release](release.md), [Website](website.md), [Dev environment](dev.md)

## Levels

Deterministic simulation is the main source of confidence ([tigerstyle](../decisions/tigerstyle.md)), and the other levels cover what a simulation cannot judge:

- golden vectors pin the wire formats and derivations that must never change;
- interop tests pin the standards against their reference implementations;
- end-to-end tests check the real IO layer that a simulation replaces.

Rules for every test:

- **Every decoder has a "never panics on arbitrary input" property test.**
- **Every simulation or property-test failure prints the seed that reproduces it.**
- **CI runs fixed seeds only, and no fuzzing** ([Engineering](../engineering.md#no-fuzzing-or-random-seeds-in-ci)).
- **The end-to-end tests skip what they lack outside CI, and fail on it in CI** (`CI=true`), so a missing tool never passes CI silently.

The install tests, the NixOS VM test and the static-binary check run only in the release workflow, before anything is signed ([Release](release.md#tests)).

## Golden vectors

The frozen surfaces of the [overview](../overview.md#frozen-surfaces) are pinned by vectors generated from the original implementation's code (`testdata/original/`, see its README), into `crates/<crate>/tests/data/`. Each file records its source commit, and regenerating must not change a committed vector. Where Dassie deliberately differs from the original (a bug fixed), the Rust test carries its own hand-written vector.

Other vectors come from their own reference: XRPL Payments, with and without `SendMax`, signed as xrpl.js signs them (`testdata/xrpl/`), Open Payments from the standards' own examples, releases from minisign with a test key (`testdata/release`). The rate feed has no other implementation, so its vectors come from its own generator with a fixed key (`testdata/feed`, [Rate feed](rate-feed.md#test-vectors)): they pin its bytes against change, not against a reference. The WASM builds run the same vectors, and must produce the bytes the native tests check, so the wallet derives exactly what the node derives.

## Property tests

Properties pin what no example can: every decoder never panics, only canonical encodings parse, no transfer breaks a ledger limit, balances equal the journal. Their regression files (`*.proptest-regressions`) are committed beside the tests. They run from one fixed seed, set in `.cargo/config.toml`, so a local `cargo test` runs what CI runs; a seed in `PROPTEST_RNG_SEED` explores others, outside CI ([random-seed-simulations](../decisions/random-seed-simulations.md)).

## Interop

Dassie runs against the reference implementations, which are the oracle for the standards: STREAM against `ilp-protocol-stream` in both directions, natively and in WASM; the BTP server against `ilp-plugin-btp`; Open Payments against `@interledger/open-payments`, which validates every response against the specs, with the node both as server and as a client of another node. The WASM build is also run in a headless browser, as the wallet and the website load it.

## Deterministic simulation

N nodes, or a crate's state machines, are wired in-process through the sans-IO crates, with a virtual clock and a seeded RNG (`dassie_node_core::testing::SeededRng`, reproducible, not cryptographic). Every simulation is library code in its crate's `src/sim/`, so `cargo test` (on fixed seeds) and the runner (on any seed) run the same code.

- Between them, the simulations inject latency, loss, reordering, duplication, partitions, clock skew, crashes and restarts, failed commits, and lying peers. What the `node` simulation does not inject yet is in the [roadmap](../roadmap.md#continuous-simulation).
- Every state machine's invariants are checked after each step.
- A run is reproducible from its seed: two runs with the same seed produce identical event traces, and a test checks it for each simulation.

### Simulation time follows production

The simulations run the production timings, not the dev ones: on the virtual clock a simulated day costs seconds, and the ledgers they settle on, like the fake XRP Ledger, run at their real speed. Dev timings, scaled down, are only for what runs on a real clock: the dev environment and the end-to-end tests.

Once a run heals, payments must complete, settlements arrive, routes converge and no node owe an honest peer past that peer's trigger, within a time derived from the production rules: for the `node` simulation, the longest wait a fault leaves behind (the backoff of a registration, a refresh, a peering attempt or a settlement, or a peer's hold-down), then as long as a network takes to be ready, then as long as a run's payments, and the settlements they call for, may take. A deadline is derived from the rules the node runs by, never chosen so that a run passes ([random-seed-simulations](../decisions/random-seed-simulations.md)). The one exception to payments completing is also a production rule: a node's payer gives up after a bounded number of reconnects, so an outgoing payment that failed in the chaos, or whose payer was already reconnecting when the run healed (since its node last started: a start gives the payer all its reconnects again), may end failed, since what is left of its reconnects can run out before the healed network routes it, for instance while a pair an owner's end split must peer anew. A payment started after the heal must complete. After a partition that outlasted the chaos it starts once the healed network is ready again, as the run's first payments do, since the network then forms anew for longer than a payer keeps trying; otherwise it starts at once, while the network re-forms after the chaos. Nor does the heal ask for routes or settlements across a pair that an owner's end or a ban keeps apart.

### Failed commits

Faults stay within what the node relies on. The store commits with `synchronous = FULL`, so a commit that returned is on disk, and a torn write is SQLite's to handle; the only fault injected at the store is a commit that fails, as on a full disk ([random-seed-simulations](../decisions/random-seed-simulations.md)).

- **It fails in SQLite,** through its commit hook, behind the `testing` feature, so SQLite rolls the transaction back as it would, and the engine meets the error its real commit returns.
- **The node stops as the daemon's engine does** ([Node](node.md#the-engine)): the core panics without handing over anything of the step, a peer request it was answering gets the daemon's 503, and the simulation treats the node as crashed until it restarts it from its last good commit.
- **It is aimed at the commits that matter:** one that books a packet's Fulfill, at a node with a payment under way; one that holds a settlement, ours prepared or executed or a peer's booked; or any. A later such commit may be the one that fails, so that a settlement can fail once it was executed.
- **What it checks** is that nothing the node sent rested on the lost commit: each node's books still hold against what its clients, receivers and peers saw, and a request answered 503 that is sent again is taken once.

### Long partitions

In some seeds the `node` simulation splits the network in two, cutting one node off from the rest or the network in halves. A split's length is drawn from the production timings that act on silence, never from what lets a run pass. Most splits are shorter than the silence that ends a peering, and spread over the timings below it: a peer's confirmation and hold-down, a stale link state, a registration's renewal. The rest outlast it, so that peerings end for silence, registrations and lists lapse and incoming payments expire before the network heals.

A split that outlasts the chaos keeps the network as the chaos left it, its cuts too, until the split ends; only a node that is down starts again, as its supervisor would start it, since held down for days it would leave the split no peering to part. A cut one way then lasts as long, as behind an asymmetric firewall: a node hears a peer it cannot reach.

- **When the split ends,** nothing has happened that must not happen even with money in flight, and every outgoing payment past its deadline has ended, completed or failed: a payment does not wait for the network to heal.
- **A client paying across the split** stops once its receiver's incoming payment has expired, with what arrived; its books are judged as any, but it need not have received all it asked for.
- **After the heal,** the production rules decide what must happen: peerings ended for silence form again through the bootstrap nodes and auto-peering, routes converge and settlements resume, within the [heal's deadline](#simulation-time-follows-production).

### Each node judged by its own books

Beyond the invariants, a simulation judges each node on its own, by its own books, and never compares two nodes' books or adds them up: each node keeps its own, and they may drift ([Engineering](../engineering.md#each-node-keeps-its-own-books), [own-books](../decisions/own-books.md)).

- **Against what really happened:** a node's books hold what its scheme holds, on a token exactly its line in whole units; its owner's funds moved by what its receivers took and its payers sent, and each token's by what its client sent. What an issuer did to a node's line counts as its owner's: what it minted a deposit, what it clawed back a withdrawal. The underlying ledgers neither make nor lose money, each asset apart in runs across currencies, a token's holders holding what its issuer minted less what came back to it.
- **Its decisions, against the liquidity its books show:** it refuses a Prepare for liquidity only by the limits its books set; it takes every settlement an honest peer sends; once the network is calm, it owes no peer past its own threshold for long; once it knows a peering is ending, it starts no settlement to that peer until they peer again; when its owner bans a peer, it tells the peer at once, asks it for nothing and never peers with it again; and when it ends one, it tells the peer only once what the peer last said it had on its way is booked, earlier bookings included, or once the wait for it ran out ([ending-a-peering](../decisions/ending-a-peering.md)).
- **Against adversaries:** whatever a lying peer does (never settle, spend the credit it was given and vanish), an honest node loses no more than the credit it extends ([own-fulfills](../decisions/own-fulfills.md)); one that vanishes loses its peering.
- **Against replays:** a copy of a request, sent again by someone who saw it arrive, is refused if its original was answered 200 or is still under way. Only a copy of a request that had no effect, refused or lost in a crash, may be taken once. A copy of a request that only reads may be taken too: a stranger's replay state does not record it, and taking it again changes nothing ([Peer protocol](peer-protocol.md#replay-protection)).
- **Liveness:** payments complete, and settlements arrive. A client pays an amount whatever its path delivers, as STREAM leaves to its user: it measures the rate again a while after a measurement failed, or after a dearer path stopped its money, as when a withdrawn route sends its packets through a hop that charges more ([STREAM](stream.md#responses)), and connects anew once its connection closed. What arrived must still be what was sent at the rates less what the hops charged. Not across a link that crashes drifted past what it survives, which the design does not save. The simulation tells how far each pair's books drifted from what crossed the network, never from the books: a Fulfill the peer booked and its sender never took, lost to a crash or a failed commit, or else refused, late or cut off. A network that did not settle down while crashes alone drifted a link that far is ruled out; drift from anything else excuses nothing ([crash-drift](../decisions/crash-drift.md)).

The peer protocol's own simulation checks what the protocol must make agree, which is not money: link states and routes converge, peerings are two-sided, and each settlement is applied exactly once.

### The simulations

Each simulation lives in its crate's `src/sim/`, and `dassie-sim list` names them all. The `peer` simulation runs the peer protocol alone; `connector` drives the connector with misbehaving clients; `stream` runs a payment over a lossy path; `hub` runs a settlement hub's consensus among faulty and colluding validators ([Settlement hubs](hubs.md#consensus)); `node` runs real engine cores with their own databases, settling on the stub, the stub with commitments or the fake XRP Ledger, through a seeded stretch of chaos and a heal, where owners also end peerings and ban peers, but never so that the pairs they keep apart split the network; the scripted node runs set up one situation each (crashes mid-payment, adversaries, packets over the stub with commitments, the live realm on the fake ledger's mainnet, runs across currencies, and RLUSD in the live realm on the fake ledger's mainnet, alone and beside XRP on one account, and DSUSD beside XRP on the testnet, while the token's issuer claws back and freezes lines); and `node-hub` runs members peered over a settlement hub whose validators run in the simulation ([below](#over-a-settlement-hub)). Seeds that draw their configuration print it before the run starts.

One run over the stub in four settles over the stub with commitments ([Settlement](settlement.md#the-stub-scheme)), so that its packets take the path over a settlement hub ([Settlement hubs](hubs.md#in-the-node)). After every step, no node's stub holds more of its packets to a peer, or of the peer's to it, than its connector holds pending with that peer: a promise kept for a packet the connector let go would never be resolved, and would take one of the peer's places for good. And the credit each node extends to each peer over the stub is its hold toward the peer and its settlements to it on their way ([Settlement hubs](hubs.md#in-the-node)).

### Over a settlement hub

The `node-hub` simulation runs a hub of 4 to 7 validators inside the node simulation, each the node's own validator role without its IO: the driver on a database of its own, and the watcher of the escrow on its own connection to the fake XRP Ledger ([Settlement hubs](hubs.md#the-validator-in-the-node)). The sponsor sets the escrow up on the fake ledger as the rules say; the validators' messages, and the members' requests to them and their answers, cross the simulated network in the region's delays, with its losses; the validators' clocks are off within the precision. The hub's terms are the design's starting values, with the timings derived from consensus's (the margin with f faulty leaders, and the allowance the rest of the connector's message window) and the stall the shortest the rules allow. Each seed draws:

- **the hub:** 4 to 7 validators, up to f of them faulty, as the `hub` simulation's roles are (silent, withholding their proposals, proposing what is invalid or to too few, equivocating), and in half the runs an honest validator that crashes and restarts from its database while payments are under way;
- **its members:** 3 or 4 nodes that join the hub by deposit from their XRP Ledger testnet accounts, then the network, so that they peer over the hub; two of them never peer, so payments cross a hop, both ways;
- **a lying member,** in three runs of four: a payer that sends no claims, a payee that keeps its Fulfills and claims on the hub only just before the commitments expire, or a payee that claims on the hub what its payer's claims cover, and each claim twice.

Once the network is calm (each account read the hub up to a recent block, each move of funds into or out of the hub is done, and each peer short of room settled once asked, all within the waits the production rules give), it checks each node by its own books ([Engineering](../engineering.md#each-node-keeps-its-own-books)): its books hold what its schemes hold, on the XRP Ledger its account's balance less the reserve, in the hub its account's balance and its peers' holds toward it as the honest validators have them, once it read the operations of its own they applied; its owner's funds on both ledgers moved by what its receivers took; the credit it extends to a peer over the hub never fell below its hold toward the peer and its settlements on their way; and what it booked as lost on a peer over the hub is never more than the holds between them ([Settlement hubs](hubs.md#packets-promises-and-claims)). The escrow holds what the hub owes and what the sponsor left, and the fake ledger made or lost no XRP. After every step of a validator, its invariants hold, no two honest validators finalized different states at one height, and no evidence names an honest one. Payments must complete, but those through a member that keeps its Fulfills or claims twice, of which only what must never happen is checked; and every honest member's account must read the hub again, and its moves be done.

Then the hub ends. In nine runs of ten it loses f + 1 validators at once, its faulty ones among them, and stalls: each validator still up declares it within the stall and a view, and signs the payout set of its last final block; the honest ones lost come back one by one, a stall apart at most, and sign in their turn. The simulation gathers the signatures over time from what each validator kept, as a member would, and submits the latest set a quorum signed, in sequence order. The escrow must then have paid each account with a disbursal address and enough to pay out its balance, its reserves and the holds it paid into, less the fee, worked out apart from the set; every transaction of the set applied; and the escrow kept what it held less what the set paid. In the tenth run, f + 1 validators collude and fork the chain: should honest validators finalize different states, evidence an honest one found must name the colluders, and only them.

An issuer's freeze stops what the nodes it touches pay or are paid, and lasts up to the grace a peer short of room has, drawn from the production timings. So while it holds, and for a grace after it is lifted, the check that routine settlement withdraws no honest peer accepts that the pairs it touches are withdrawn: that is the production rule at work ([Settlement](settlement.md#when-peers-cannot-pay-us)).

The `node` simulation counts what it reached (crashes mid-payment, cuts, settlements, withdrawals, …), and a test asserts that its fixed seeds between them reach every kind of fault and payment, so that a generator cannot quietly stop reaching a case. An outcome too rare for a few fixed seeds to be sure to reach, such as a settlement booked after its peering ended, has a targeted test instead: a minimum on it would fail whenever an unrelated change moved the seeds' runs.

### Outcomes

A run ends as `pass`, `safety` (something that must never happen did: an assertion, an invariant or a check of what the run did), `liveness` (something that must happen did not, once the network was calm), or `unsupported` (the run drew a situation no code could recover from, such as a majority of bootstrap nodes cut off, or a link that crashes drifted past what it survives; it neither passes nor fails) (`dassie_node_core::testing::Outcome`). Each of the node simulation's checks says which of the two failures it found. A run that broke both says safety: one that did not settle down is first checked for what must not happen even with money in flight, and one that did runs every check and reports a safety failure before any liveness failure.

Every outcome but `pass` also says where the run ended (`Finding`): the name of the check that failed or ruled the run out, or the file and line of a panic. It holds nothing a seed draws, so every seed that ends at the same place shares it; a check's name is words in lower case, so it cannot carry a number. With the simulation and the outcome, it is the run's **signature**, `<sim>/<outcome>/<where>`, which tells two failures apart whatever their seeds.

A check's name changes only when the check is renamed. A panic's file and line are what a panic offers, and cost two things: an edit above it in the same file moves its line, so one bug seen before and after such a commit gets two signatures; and a panic inside a dependency that does not pass on its caller's location puts every caller under one. The first splits an issue, the second may merge two, and the message, the trace and the replay tell them apart.

### The runner: `dassie-sim`

`dassie-sim` runs any simulation on any seed, to replay a failure, search a range for one, run a commit's own seed, or run random seeds as a campaign. It is a developer tool, never released.

**The campaign command.** `dassie-sim campaign` runs random seeds, drawn from the OS, on every core for a given time, and prints a line of JSON per finished run, then a summary. The lines are the runner's contract with the campaign's supervisor, set out in the runner's docs (`crates/dassie-sim/src/main.rs`).

- Each run's simulation is drawn by its weight. A weight counts runs, not time, so the simulations whose runs take milliseconds cost almost nothing, and the `node` simulation, with the most weight, gets most of the time.
- A record names its seed and signature; a failure's also holds what its seed drew and the end of its trace, enough to file it without running it again.
- Each run is a process of its own, so that runs share nothing and each takes as long as it does alone, on a core of its own. No run outlives its campaign: one that ends early stops and reaps its runs, and a run dies with a campaign that is killed, since nothing else would stop a stuck one.
- A panic is caught as a safety failure, and a run whose process ends without its record, such as one that overflows its stack, is one at `abort`, so that a run that panics or aborts cannot end the campaign. The supervisor's sandbox still stops the whole campaign when the kernel kills any of its processes for want of memory. Every simulation bounds its steps, so every run ends; a run that still takes far longer in wall time than any run should, stuck within a step, is stopped and reported as a liveness failure at `wall-time`.
- The campaign exits 0 when it ran to its end, whatever its runs found, since its records say that.

**The tripwire.** The `tripwire` simulation fails on purpose, for safety at one fixed place, on a small share of seeds, and passes the others at once. A campaign runs it among the others, so that a campaign whose tripwire stops failing shows that it has stopped. No test expects it to pass on such a seed.

**Reproducing a failure.** A test or run that fails names its simulation and seed.

- `dassie-sim run <sim> <seed>` replays it, and `dassie-sim explain <sim> <seed>` prints its trace, optionally only the lines that match.
- Build the runner with `--profile sim`, which is the test profile with assertions and overflow checks on, so a seed replays exactly as the test of the same seed.
- A seed reproduces a failure only at the commit it ran on, so a fix comes with a targeted test that sets up the situation directly.
- A panic is a safety failure that loses its trace; replay the seed to see what led to it.
- A git commit's full SHA is also a seed, so that a commit names its own run.

## The campaign

The campaign runs the simulations from random seeds on spot VMs, outside CI ([random-seed-simulations](../decisions/random-seed-simulations.md)). Its supervisor, `scripts/sim-campaign.ts`, runs one loop at a time on each VM: it builds a commit's `dassie-sim` in the `sim` profile, runs the runner's [campaign command](#the-runner-dassie-sim) on every core for a while, reports, and exits. systemd starts the next loop from a fresh checkout of `main`, so a change to the supervisor or the runner on `main` reaches every VM within a loop. The VMs' setup is in `scripts/sim-campaign/`.

- **What runs.** Most loops run `main`. Open pull requests labelled `sim` share some of the time; the label is the gate, since only people with triage access to the repository can set it.
- **It is not a check.** The campaign posts no commit status, on `main` or on a pull request: it reports in issues and comments only ([random-seed-simulations](../decisions/random-seed-simulations.md)).
- **A failure's [signature](#outcomes)** is its simulation, its outcome, and the assertion or check that failed: what tells two failures apart, whatever their seeds.
- **The [tripwire](#the-runner-dassie-sim),** a simulation that fails on purpose on a small share of seeds, shows that the campaign runs: while its issue keeps getting seeds, the campaign has not stopped. Its issue is never commented on.
- **A commit of `main`** gets one issue per failure signature, labelled `sim-failure`, or `sim-tripwire` for the tripwire, and found again by a marker in its body. The issue holds the commit, the configuration, the end of the trace and the commands that reproduce the seed that failed fastest. Later seeds update its counts and its fastest seed in place, and a comment says when it fails on a newer commit, a bounded number of times, so that no issue grows without end. A closed issue is never reopened: a signature seen again after its fix gets a new one.
- **A pull request** gets comments on itself, and nothing else: a summary of its latest loop, and one comment per failure signature, each edited in place. One that does not build gets the end of its build's log there instead.
- **A loop on `main` counts only if all of it was read.** A line the supervisor could not parse, a failure past what one loop keeps, or a count that disagrees with the runner's summary could each hide a failure. Such a loop files a failure under a signature of its own, `campaign/unreadable`, rather than pass over one nobody saw. A loop the runner did not finish is not failed for its last line, cut short: a runner stopped at its sandbox's limits leaves one, through no fault of the commit. Anything else it could not read still files one.
- **The campaign's own trouble** has an issue of its own, labelled `sim-campaign`. CI keeps `main` building, so a commit of `main` that the campaign cannot check out, fetch, build, or start the runner of almost always means the campaign itself is broken: its sandbox, its toolchain, a VM's disk, or its network. Such a loop files or updates that one issue, with the commit, the end of the step's log and the time, and exits with an error, so that its systemd unit shows as failed; systemd starts the next loop a minute later all the same, so that a fix on `main` or a network that comes back takes effect at once. Later occurrences update the issue in place, with a count and the latest time, and never comment, since the supervisor retries every minute. It is not a `sim-failure`, since nothing in it is the commit's fault. Without it, a campaign that cannot build would only go quiet, which nobody watches for.

### Isolating pull requests

A pull request's code is unreviewed, and the VMs run in the project that holds the bootstrap nodes' phrases. It must not be able to read the GitHub credentials, touch the issues, or change what a later loop on a trusted commit builds or reports.

- **The credentials live in the supervisor alone.** The VMs' service account has no role in the project and can read one secret, the GitHub App's key. The supervisor reads it into memory through the metadata server and mints short-lived tokens from it; it never writes it to disk or hands it to a child.
- **Every build and run is sandboxed** (`sandboxArgs` in the supervisor): as a user of its own, one for trusted commits and one for pull requests, with the file system read-only but for the step's own directories, no view of `/root`, `/home` or other users' processes, private `/tmp`, `/dev` and IPC, no privilege to gain, only a service's system calls, a share of memory and tasks that leaves the supervisor room to report, and no network, so no way to the metadata server, whose token reads the key. Build scripts and proc macros are the pull request's code too, and run inside.
- **Fetching dependencies needs the network,** and runs none of the commit's code: `cargo fetch --locked`, in a sandbox that reaches only public addresses through public resolvers, outside the commit's tree, so that no Cargo configuration of the pull request applies.
- **The Nix daemon is out of reach.** Its builds run outside the sandbox, with the network, so only root may use it and no sandbox can see its socket. The toolchain is `main`'s, from the flake's `campaign` shell: a pull request's flake is never evaluated, since its dev shell would run as root.
- **Nothing of a pull request outlives its loop.** It builds from nothing, in a directory removed before every loop and after its own, however it ends, so that no build of one can leave the disk full for `main`; its sandboxes stop with the loop; and trusted builds live in directories only the trusted user can write.
- **What it prints is untrusted data.** The supervisor parses it within bounds, uses it only for that pull request's comments, and puts its texts in fenced blocks they cannot close.

A pull request can still waste its own loop, fill the disk for the rest of it, and say what it likes in its own comments.

## End to end

The dev environment's scenarios run headless in CI on real binaries with the wallet built in: start several nodes, set them up through the HTTP API, add ledgers, peer, pay, settle, restart, and check that the balances are unchanged. They also cover what only real IO shows: a fresh install, socket-activated, getting its certificate from an ACME test server; reference clients paying through BTP and Open Payments; the wallet in a headless browser; settling on the fake XRP Ledger, which a test can switch to the public testnet by hand, in XRP and in DSUSD, whose issuer there freezes a line and claws back.

**Mainnet's schemes in the live realm** (`mainnet_test.ts`, on `xrpl-live-settlement` and `rlusd-live-settlement`): two nodes, set to the live realm through the dev switch, settle in XRP and in RLUSD on the fake ledger's mainnet at addresses under `g.`, offering only mainnet's schemes. Then RLUSD's pool is moved 5% off its peg: each node pauses its RLUSD scheme once the guard's hold has passed, on dev timers, and says so in its ledger, still books what the issuer sends it, and runs it again once the pool is back at a dollar for the recovery hold; drained below RLUSD's floor of XRP, the pool is no price, and the guard pauses the scheme until it reads one again ([XRPL](xrpl.md#the-peg-guard)).

**Passive rebalancing, the whole loop** (`market_maker_test.ts`, on the `market-maker` scenario): a core node short of DSUSD publishes its feed, and `dassie market-maker`, run from the same build for a bounded time, trades through it with its two pinned nodes and swaps back through the fake ledger's pool ([Rate feed](rate-feed.md#market-maker)). It reads what each party holds from its own books and the ledger: the core node's share of its value in DSUSD rose, toward its target; the market maker's value, at the pool's price after the run, grew; and every node's books are unchanged across a restart. With two targets (`market_maker_targets_test.ts`, on `market-maker-two-targets`), it trades through each: both core nodes take DSUSD in, and each one's share of its value in DSUSD rises. It waits on what the books and the pool show, each wait bounded, never on the market maker's log; the decisions themselves are the strategy's unit tests.

## Installer and updater

The install tests run the installer and the updater in containers with systemd as PID 1, against a mirror of signed test releases; [Release](release.md#tests) states what they check. Two rules for changing them:

- **Between them, the images must take every path the installer has:** both verifiers, both downloaders and both compressions. Each Dockerfile (`tests/e2e/install/`) says which path it takes; a change to an image must keep every path covered.
- **The installer runs under `umask 077`,** as on a hardened host, so what it installs must still be readable by the node's own user.

## Decisions

- [tigerstyle](../decisions/tigerstyle.md): why simulation is the main tool, and why the code is built for it from the start.
- [own-books](../decisions/own-books.md): why a simulation judges each node by its own books.
- [crash-drift](../decisions/crash-drift.md): why a run that crashes drifted a link past what it survives is ruled out, and how the drift is measured.
- [own-fulfills](../decisions/own-fulfills.md): the bound an adversary is held to.
- [random-seed-simulations](../decisions/random-seed-simulations.md): one runner, simulations from random seeds, the campaign, and why CI keeps fixed seeds.
