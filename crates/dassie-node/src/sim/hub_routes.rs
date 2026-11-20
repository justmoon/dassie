//! The node simulation's run of settlement methods and random next hops
//! over two hubs (`node-hub-routes`, spec/areas/testing.md#over-a-settlement-hub,
//! spec/decisions/settlement-methods.md, spec/decisions/weighted-next-hops.md):
//! five nodes, each a member of the XRP hub and of the DSUSD hub, two hubs
//! of 4 to 7 honest validators each run in the simulation on the one fake
//! XRP Ledger, as in `node-hubs`. Bans make a square with a tail: node 0
//! peers with node 1 alone, node 1 with nodes 2 and 3, and those two with
//! node 4, so that node 1 has two first hops to node 4, and draws one per
//! packet, weighted by the fees each earned it. Nodes 1 and 3 charge for
//! what passes, node 2 passes it for free.
//!
//! - **No swaps.** Every owner turns peer swaps off: a swap converts on
//!   purpose (spec/decisions/peer-swaps.md), and its packets cross the
//!   DSUSD hub, so that it would fail the checks that routed packets keep
//!   their ledger. No simulation checks a swap, nor that swaps refill a hop
//!   short of room: `node-hubs` and `node-hub-currencies` leave them on but
//!   script none and check none. The core's tests
//!   (`engine/core/tests/swapping.rs`) and the daemons'
//!   (`tests/peer_swaps.rs`) test them.
//! - **Methods, and a hop that refuses.** Node 3 first peers with node 1
//!   alone and funds its hold toward it in the XRP hub; then its owner
//!   takes out of the XRP hub all its account can spend, so that node 3 can
//!   fund no hold toward node 4, and only then may node 3 peer with node
//!   4. What node 3 holds toward node 1 keeps node 4 a slot in the XRP hub
//!   (spec/decisions/slots-by-liquidity.md). Node 0 pays node 4 in XRP and
//!   node 4 pays node 0 in dollars, each payment's two ends on one ledger:
//!   no node on the way converts, though every one is in both hubs, which
//!   each one's books show (no FX booked anywhere, node 1's fees booked on
//!   each payment's ledger). Node 1 draws node 3 for some packets in XRP,
//!   which node 3 refuses (T04), node 0 paying again until it has; the XRP
//!   payments complete through node 2, and node 3 earns nothing in XRP:
//!   neither a fee of its own nor any weight in node 1's draws.
//! - **Weight costs fees.** Whatever the payments, the weight a node gives
//!   its next hops on a ledger never exceeds the fees it earned there: so
//!   raising a hop's weight costs the node's fee on every unit pushed
//!   through it.
//! - **Rejects count for nothing.** Once node 3's owner moved funds back,
//!   node 0, an attacker with node 4, sends itself packets through node 1
//!   that node 4 refuses: node 1's weights are, at the end, exactly what
//!   their decay alone makes of them.
//! - **A hop short of room keeps its weight.** Node 2's owner takes out of
//!   the XRP hub all its account can spend, so that node 2 cannot fund its
//!   hold toward node 4 again: node 1 still draws it by its weight, and,
//!   once node 0 has paid past what that hold had left, it answers T04,
//!   its packets moving neither to its DSUSD method nor anywhere else. Its
//!   owner moves funds back in, and the payments complete once node 2's
//!   hold is funded again, or through node 3 on an exploring draw; at
//!   every step of a window longer than that settlement takes, node 2's
//!   weight has lost only what decay takes: until its first T04, of what
//!   it had earned when its owner took the funds out, and from then on, of
//!   what it had earned by then. Then it carries packets again.
//!
//! Once the network is calm, every check of `node-hubs` holds: each node
//! by its own books in each hub, and what each peer can cost bounded on
//! each method by the holds on its hub, so summed over the methods.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use dassie_identity::NodeId;
use dassie_ilp::{Address, ErrorCode, Packet, Prepare, Timestamp};
use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_ledger::{AccountName, LedgerId};
use dassie_oer::{Decode, Encode};
use dassie_peer::wire::Content;
use dassie_xrpl::{DROPS_PER_XRP, UNITS_PER_DROP};

use super::checks::{check_across, check_events, first_failure};
use super::hub::{PREPARES_OVER_DSUSD, PREPARES_OVER_XRP};
use super::hub_checks::{ledger_of, status_in};
use super::hub_pairs::{
    JOINED_WITHIN_MS, OWNER_DEPOSIT_XRP, check_hubs, join_hubs, owner_deposits, settle_down_in,
};
use super::hubbed::{paid, routed};
use super::scripted::{READY_WITHIN_MS, ended};
use super::{
    Delivery, HUB, HUB_DSUSD, LEDGER_CLOSE_MS, LOSS_PER_MILLE, Settling, Sim, ilp_packet, key,
};
use crate::node::Policies;
use crate::store::PeerSettings;

/// The node whose client pays, and the attacker's sender.
const SENDER: usize = 0;
/// The node that charges, with two first hops to [`PAYEE`].
const HOP: usize = 1;
/// One of [`HOP`]'s first hops: free, and in the end short of room.
const LEFT: usize = 2;
/// The other: charging, and short of room from the start.
const RIGHT: usize = 3;
/// The node whose receiver is paid, and the attacker's receiver.
const PAYEE: usize = 4;
/// How many nodes the run has.
const NODES: usize = 5;

/// The pairs that never peer: what is left is the square of
/// [`HOP`], [`LEFT`], [`RIGHT`] and [`PAYEE`], and [`SENDER`] behind
/// [`HOP`].
const BANS: [(usize, usize); 5] = [
    (SENDER, LEFT),
    (SENDER, RIGHT),
    (SENDER, PAYEE),
    (HOP, PAYEE),
    (LEFT, RIGHT),
];

/// The pair that may peer only once [`RIGHT`] holds toward [`HOP`] and its
/// owner took its funds out of the XRP hub.
const LATE: (usize, usize) = (RIGHT, PAYEE);

/// What node 3 charges for what passes, as node 1 does: a 1% fee.
const RIGHT_CHARGES: PeerSettings = PeerSettings {
    connector_fee_ppm: 10_000,
    ..PeerSettings::DEFAULT
};

/// How many more payments node 0 makes, at most, for node 1 to draw node
/// 3, which it draws only when exploring once node 2 earned it a fee, and
/// see its T04. A tenth of node 1's draws explore, split evenly over its
/// two XRP hops, one fresh draw per Prepare (`explore_ppm`). A Prepare to
/// node 3 lost on its way fails as T01, and a T04 lost on its way back
/// shows nothing either, each one message in a hundred
/// ([`LOSS_PER_MILLE`]). Each packet then shows a T04 with a chance of
/// 0.05 × 0.99², and the at least 280 packets of 28 payments all miss in
/// fewer than one run in a million (0.951^280 ≈ 7.8 × 10⁻⁷). A model:
/// node 3's weight stays nothing while it refuses, and losses are
/// independent.
const AGAIN: u64 = 28;

// The bound holds as long as node 1 explores at least as often, the run
// loses at least as few messages, and a payment sends at least as many
// packets: each of those only makes a T04 from node 3 likelier.
const _: () = assert!(
    Policies::DEFAULT.connector.explore_ppm >= 100_000
        && LOSS_PER_MILLE <= 10
        && AGAIN * LEAST_PACKETS >= 280
);

/// The fewest packets of the largest a payment of the run sends: each
/// sends from this many to twice as many, less one.
const LEAST_PACKETS: u64 = 10;

/// How long a peering may take to gain the method of every hub both nodes
/// are in, once it is made: a request and its answer, retried a few times.
const METHODS_WITHIN_MS: u64 = 5 * 60_000;

/// How long a payment may take, at production timings.
const PAID_WITHIN_MS: u64 = 10 * 60_000;

/// How long the attacker's packets live.
const PACKET_EXPIRY_MS: u64 = 30_000;

/// How long the attacker's packets have to cross and be refused.
const ATTACK_MS: u64 = 2 * PACKET_EXPIRY_MS;

/// How many packets the attacker sends itself.
const ATTACK_PACKETS: u32 = 20;

/// The window over which a hop short of room must keep its weight: the
/// payment it refuses, its owner's move back into the hub, within the wait
/// of a move, and the settlement that funds its hold again, asked once a
/// minute.
const SHORT_WINDOW_MS: u64 =
    PAID_WITHIN_MS + JOINED_WITHIN_MS + 2 * Policies::DEFAULT.settlement_request_every_ms;

// The window is shorter than a half-life of the weights: decay takes at
// most half of what a hop earned while it is short of room.
const _: () = assert!(SHORT_WINDOW_MS < Policies::DEFAULT.connector.earnings_half_life_ms);

/// What an owner moves back into the XRP hub once it took its funds out,
/// in XRP: half its deposit.
const BACK_IN_XRP: u64 = OWNER_DEPOSIT_XRP / 2;

/// What a run counts of the T04s a hop short of room answered itself.
pub const SHORT_OF_ROOM: &str = "packets refused by a hop short of room (T04)";

/// What a run counts of the attacker's packets that its receiver refused.
pub const ATTACK_REFUSED: &str = "attacker's packets refused by its receiver";

/// The salt of the run's generator: "hubroute".
const SALT: u64 = 0x6875_6272_6f75_7465;

/// What a seed draws.
struct Setup {
    validators: usize,
}

fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ SALT);
    let validators = 4 + usize::try_from(rng.below(4)).unwrap();
    (rng, Setup { validators })
}

/// What a seed of `node-hub-routes` draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    format!(
        "{NODES} members of an XRP hub and a DSUSD hub of {} validators each, node 1 with two \
         first hops to node 4, each in turn short of room",
        setup.validators
    )
}

/// A run of settlement methods and next hops over two hubs.
#[must_use]
pub fn run(seed: u64) -> Run {
    let (mut rng, setup) = setup(seed);
    let mut sim = Sim::with(
        seed,
        NODES,
        0,
        BTreeMap::new(),
        Settling::Hubs(setup.validators),
    );
    let result = script(&mut sim, &mut rng);
    ended(sim, result)
}

/// What the run saw cross the network, each peer message as it came to be
/// delivered next.
#[derive(Default)]
struct Seen {
    /// The messages looked at that are still to be delivered.
    looked: BTreeSet<(u64, u64)>,
    /// Each node's ILP address.
    addresses: Vec<String>,
    /// Rejects by their sender, their receiver, the node that refused the
    /// packet and its code.
    rejects: BTreeMap<(usize, usize, usize, ErrorCode), u64>,
    /// Prepares on a method, by their sender and the method's scheme.
    prepares: BTreeMap<(usize, String), u64>,
    /// Fulfills by their sender and their receiver.
    fulfills: BTreeMap<(usize, usize), u64>,
    /// A hop's weight, watched.
    watch: Option<Watch>,
}

/// [`HOP`]'s weight for a first hop on the XRP hub, as of the end of a
/// window, watched at every step of the network: a weight cut on a
/// refusal shows in the lowest at the next step, before a Fulfill the hop
/// carries once its room is back could make up for it. Earnings decay by
/// whole halvings and a fee only adds to them, so nothing else lowers it.
struct Watch {
    /// The first hop.
    hop: usize,
    /// When the window ends.
    until: u64,
    /// The lowest weight seen, as of `until`.
    lowest: u64,
}

impl Seen {
    fn new(sim: &Sim) -> Self {
        Self {
            addresses: (0..sim.nodes.len())
                .map(|index| sim.core(index).own_address().to_owned())
                .collect(),
            ..Self::default()
        }
    }

    /// The node at or below whose address `address` is.
    fn node_of(&self, address: &str) -> Option<usize> {
        self.addresses.iter().position(|own| {
            address
                .strip_prefix(own.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    }

    /// Looks at the message to be delivered next, once.
    fn look(&mut self, sim: &Sim) {
        self.watch_weight(sim);
        // What is not in the queue any more was delivered or dropped.
        self.looked = self.looked.split_off(&(sim.now, 0));
        let Some((&at, delivery)) = sim.queue.iter().next() else {
            return;
        };
        if !self.looked.insert(at) {
            return;
        }
        let Delivery::Request { from, to, body, .. } = delivery else {
            return;
        };
        if let Ok(Content::MethodPacket {
            scheme, commitment, ..
        }) = Content::decode_exact(body)
            && !commitment.is_empty()
        {
            *self
                .prepares
                .entry((*from, scheme.as_str().to_owned()))
                .or_default() += 1;
            return;
        }
        let reject = match ilp_packet(body) {
            Some((_, Packet::Reject(reject))) => reject,
            Some((_, Packet::Fulfill(_))) => {
                *self.fulfills.entry((*from, *to)).or_default() += 1;
                return;
            }
            _ => return,
        };
        if let Some(by) = reject
            .triggered_by
            .and_then(|address| self.node_of(address.as_str()))
        {
            *self
                .rejects
                .entry((*from, *to, by, reject.code))
                .or_default() += 1;
        }
    }

    /// Takes in the watched hop's weight.
    fn watch_weight(&mut self, sim: &Sim) {
        if let Some(watch) = self.watch.as_mut() {
            let now = weight(sim, HOP, watch.hop, HUB, watch.until);
            watch.lowest = watch.lowest.min(now);
        }
    }

    /// The Rejects `from` sent `to` of packets `by` refused with `code`.
    fn rejected(&self, (from, to): (usize, usize), by: usize, code: ErrorCode) -> u64 {
        self.rejects
            .get(&(from, to, by, code))
            .copied()
            .unwrap_or(0)
    }

    /// The Fulfills `from` sent `to`.
    fn fulfilled(&self, from: usize, to: usize) -> u64 {
        self.fulfills.get(&(from, to)).copied().unwrap_or(0)
    }

    /// The Prepares `from` sent on its method on `ledger`.
    fn prepared(&self, from: usize, ledger: LedgerId) -> u64 {
        self.prepares
            .get(&(from, Settling::scheme(ledger).0.to_owned()))
            .copied()
            .unwrap_or(0)
    }
}

/// Runs until `until` or until `stop` holds, looking at each peer message
/// as it comes to be delivered.
fn run_seeing(
    sim: &mut Sim,
    seen: &RefCell<Seen>,
    until: u64,
    stop: impl Fn(&Sim) -> bool,
) -> bool {
    run_sliced(sim, until, |sim| {
        seen.borrow_mut().look(sim);
        stop(sim)
    })
}

/// Runs until `until` or until `stop` holds, in slices: two hubs'
/// validators make a busy network, and the simulation bounds the steps of
/// one run.
fn run_sliced(sim: &mut Sim, until: u64, stop: impl Fn(&Sim) -> bool) -> bool {
    while sim.now < until {
        if sim.run_until_or(until.min(sim.now + SLICE_MS), &stop) {
            return true;
        }
    }
    stop(sim)
}

/// How much of the network's time [`run_sliced`] runs at once.
const SLICE_MS: u64 = 60_000;

/// The run's hubs, by their places.
fn places(sim: &Sim) -> Vec<usize> {
    sim.hubs().map(|hub| hub.spec.place).collect()
}

/// The place of the run's hub of `ledger`.
fn place_of(sim: &Sim, ledger: LedgerId) -> usize {
    sim.hubs()
        .find(|hub| ledger_of(hub) == ledger)
        .map(|hub| hub.spec.place)
        .unwrap()
}

/// What node `index` booked on its account `name` on `ledger`.
fn booked(sim: &Sim, index: usize, ledger: LedgerId, name: AccountName) -> i128 {
    sim.balances_on(index, ledger)
        .get(&name.to_string())
        .copied()
        .unwrap_or(0)
        .abs()
}

/// The weight node `index` gives `peer` as a next hop on `ledger`, as of
/// `at_ms` on the network's clock: what its earnings now decay to by then,
/// on the node's own clock, by which it stamps them.
fn weight(sim: &Sim, index: usize, peer: usize, ledger: LedgerId, at_ms: u64) -> u64 {
    let local = at_ms.saturating_add_signed(sim.nodes[index].skew_ms);
    sim.core(index)
        .next_hop_weight(&key(peer).node_id(), ledger, local)
        .unwrap_or(0)
}

/// Whether the network is as the run needs it: every node routed to
/// every other, node 1 with both first hops to node 4, and every peering
/// with a method on each hub.
fn formed(sim: &Sim) -> Result<(), String> {
    if !routed(sim) {
        return Err("not every node has a route to every other".into());
    }
    let payee = key(PAYEE).node_id();
    let hops: BTreeSet<NodeId> = sim
        .core(HOP)
        .network()
        .nodes
        .into_iter()
        .find(|node| node.id == payee)
        .map(|node| node.first_hops.into_iter().collect())
        .unwrap_or_default();
    let both: BTreeSet<NodeId> = [key(LEFT).node_id(), key(RIGHT).node_id()].into();
    if hops != both {
        return Err(format!(
            "node 1's first hops to node 4 are {hops:?}, not nodes 2 and 3"
        ));
    }
    let schemes = [Settling::scheme(HUB).0, Settling::scheme(HUB_DSUSD).0];
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            let has: Vec<String> = (peer.peering.schemes())
                .map(|scheme| scheme.as_str().to_owned())
                .collect();
            if let Some(missing) = schemes
                .iter()
                .find(|scheme| !has.iter().any(|held| held == *scheme))
            {
                return Err(format!(
                    "node {index}'s peering with {} has no method on {missing}: {has:?}",
                    peer.id
                ));
            }
        }
    }
    Ok(())
}

/// Starts a payment of `packets` of the largest from `from`'s client to
/// `to`'s receiver, both on `ledger`, and says which it is.
fn start(
    sim: &mut Sim,
    (from, to): (usize, usize),
    ledger: LedgerId,
    packets: u64,
) -> Result<usize, Outcome> {
    let index = sim.payments.len();
    let amount = packets * super::MAX_PACKET;
    if sim.pay_in(from, (ledger, ledger), to, amount, None, None) {
        Ok(index)
    } else {
        Err(Outcome::liveness(
            "paid",
            format!("a payment from node {from} to node {to} could not start"),
        ))
    }
}

/// The payments from `first` on finished, each having sent something, and
/// arrived with what was sent less what the hops charged.
fn check_paid(sim: &Sim, first: usize, what: &str) -> Result<(), Outcome> {
    for payment in &sim.payments[first..] {
        let (sent, got) = (super::sent(payment), super::received(sim, payment));
        let least = super::least_across(payment.ledger, payment.ledger, sent);
        if !payment.done || sent == 0 {
            return Err(Outcome::liveness(
                "paid",
                format!(
                    "{what}: the payment from node {} to node {} did not finish: sent {sent} of \
                     {}, received {got}",
                    payment.from, payment.to, payment.amount
                ),
            ));
        }
        if got < least {
            return Err(Outcome::safety(
                "received",
                format!(
                    "{what}: node {} received {got} of node {}'s {sent}",
                    payment.to, payment.from
                ),
            ));
        }
    }
    Ok(())
}

/// No node converted anything, on either ledger: by its own books, every
/// packet left each node on the ledger it came in on, so on that ledger's
/// method.
fn check_unconverted(sim: &Sim) -> Result<(), String> {
    for index in 0..sim.nodes.len() {
        for ledger in [HUB, HUB_DSUSD] {
            let fx = booked(sim, index, ledger, AccountName::RevenueFx)
                + booked(sim, index, ledger, AccountName::ExpensesFx);
            if fx != 0 {
                return Err(format!(
                    "node {index} booked {fx} of conversions on {ledger}, where every payment \
                     stays on its own ledger"
                ));
            }
        }
    }
    Ok(())
}

/// The weight each node gives its next hops on a ledger never exceeds
/// the fees it earned there, a packet's fee counting toward the hop it went
/// to only if it kept its ledger: so a hop's weight costs the node's fee on
/// every unit pushed through it, which the packet's sender paid.
fn check_weights_cost_fees(sim: &Sim) -> Result<(), String> {
    for index in 0..sim.nodes.len() {
        let now = sim.local_now(index);
        let core = sim.core(index);
        let peers: Vec<NodeId> = core.network().peers.iter().map(|peer| peer.id).collect();
        for ledger in [HUB, HUB_DSUSD] {
            let weights: u128 = peers
                .iter()
                .filter_map(|peer| core.next_hop_weight(peer, ledger, now))
                .map(u128::from)
                .sum();
            let fees = booked(sim, index, ledger, AccountName::RevenueFees).unsigned_abs();
            if weights > fees {
                return Err(format!(
                    "node {index} weighs its next hops on {ledger} at {weights}, more than the \
                     {fees} it earned in fees there"
                ));
            }
        }
    }
    Ok(())
}

/// Both methods carried packets.
fn check_carried(sim: &Sim) -> Result<(), String> {
    let over = |what| sim.counts.get(what).copied().unwrap_or(0);
    let (xrp, dsusd) = (over(PREPARES_OVER_XRP), over(PREPARES_OVER_DSUSD));
    if xrp == 0 || dsusd == 0 {
        return Err(format!(
            "packets crossed the XRP hub {xrp} times and the DSUSD hub {dsusd} times"
        ));
    }
    Ok(())
}

/// Whether node `index` has a move into or out of the XRP hub on its way.
fn moving(sim: &Sim, index: usize) -> bool {
    status_in(sim, index, place_of(sim, HUB)).is_some_and(|status| status.moving.is_some())
}

/// Node `index`'s owner takes out of the XRP hub all its account can
/// spend, less the fees, once the hub pays it out: the node can then fund
/// no hold there. The owner's automatic moves are off since its deposit.
fn take_out(sim: &mut Sim, index: usize) -> Result<(), Outcome> {
    let place = place_of(sim, HUB);
    let scheme = sim.hub_at(place).scheme();
    let fees = sim.hub_at(place).rules.terms.fees;
    let deadline = sim.now + JOINED_WITHIN_MS;
    let mut taken: Result<(), String>;
    loop {
        let drops = status_in(sim, index, place)
            .map_or(0, |status| status.balance / UNITS_PER_DROP)
            .saturating_sub(fees.operation + fees.ledger);
        taken = Err("too little to take".into());
        if drops >= fees.least_payout {
            sim.step(index, |core| {
                taken = core
                    .hub_withdraw_from(scheme, drops * UNITS_PER_DROP)
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}"));
            });
        }
        if taken.is_ok() || sim.now >= deadline {
            break;
        }
        // All of it, never less: what a withdrawal leaves, at least the
        // hub's least payout, funds holds of many packets. An owner whose
        // node cannot take it now, its account reading up to a newer state
        // of the hub, or a settlement of its own on its way that the agreed
        // balance does not show yet, tries again a ledger later, until the
        // wait of a move is over, as in `owner_deposits`.
        run_sliced(sim, sim.now + LEDGER_CLOSE_MS, |_| false);
    }
    if let Err(error) = taken {
        return Err(Outcome::liveness(
            "taken-out",
            format!("node {index}'s owner could not take its funds out of the XRP hub: {error}"),
        ));
    }
    if !run_sliced(sim, sim.now + JOINED_WITHIN_MS, move |sim| {
        !moving(sim, index)
    }) {
        return Err(Outcome::liveness(
            "taken-out",
            format!("node {index}'s owner's withdrawal from the XRP hub was not done in time"),
        ));
    }
    // Less than an operation's fee left: the node can make no operation of
    // its own in the hub, so it neither funds a hold nor takes what its
    // peers' holds toward it owe it into its balance
    // (`dassie_hub::ledger::Ledger::apply_member`). An honest run leaves
    // nothing: the node takes a withdrawal of the whole balance only with
    // no operation of its own waiting or on its way, which would execute
    // first, so it executes on the balance it was sized from; after it,
    // only the node's own operations credit the account, a redeem of a
    // peer's claim among them, and the hub refuses each for its fee. No
    // node moves funds into another's account, and the owner's moves are
    // off.
    let left = status_in(sim, index, place).map_or(0, |status| status.balance / UNITS_PER_DROP);
    if left >= fees.operation {
        return Err(Outcome::liveness(
            "taken-out",
            format!(
                "node {index} kept {left} drops in the XRP hub once its owner took its funds \
                 out, enough to operate there"
            ),
        ));
    }
    sim.trace.push(format!(
        "{} node {index}'s owner took its funds out of the XRP hub",
        sim.now
    ));
    Ok(())
}

/// Node `index`'s owner moves [`BACK_IN_XRP`] back into the XRP hub, a
/// ledger later while its node cannot take the move yet, and the move is
/// done by `until`.
fn bring_back(
    sim: &mut Sim,
    seen: &RefCell<Seen>,
    index: usize,
    until: u64,
) -> Result<(), Outcome> {
    let place = place_of(sim, HUB);
    let scheme = sim.hub_at(place).scheme();
    let mut moved: Result<(), String> = Err("not yet".into());
    while sim.now < until {
        sim.step(index, |core| {
            moved = core
                .hub_deposit_to(scheme, BACK_IN_XRP * DROPS_PER_XRP * UNITS_PER_DROP)
                .map(|_| ())
                .map_err(|error| format!("{error:?}"));
        });
        if moved.is_ok() {
            break;
        }
        run_seeing(sim, seen, sim.now + LEDGER_CLOSE_MS, |_| false);
    }
    if let Err(error) = moved {
        return Err(Outcome::liveness(
            "brought-back",
            format!("node {index}'s owner could not move funds back into the XRP hub: {error}"),
        ));
    }
    sim.trace.push(format!(
        "{} node {index}'s owner moves funds back into the XRP hub",
        sim.now
    ));
    if !run_seeing(sim, seen, until, move |sim| !moving(sim, index)) {
        return Err(Outcome::liveness(
            "brought-back",
            format!("node {index}'s owner's move back into the XRP hub was not done in time"),
        ));
    }
    Ok(())
}

/// What node `from` holds toward node `to` in the XRP hub.
fn holding(sim: &Sim, from: usize, to: usize) -> u64 {
    let to = key(to).node_id();
    status_in(sim, from, place_of(sim, HUB)).map_or(0, |status| {
        status
            .paying
            .iter()
            .filter(|hold| hold.peer == Some(to))
            .map(|hold| hold.amount)
            .sum()
    })
}

fn script(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    join_hubs(sim)?;
    owner_deposits(sim)?;
    for index in 0..NODES {
        let settings = if index == RIGHT {
            RIGHT_CHARGES
        } else {
            sim.nodes[index].store.peer_settings().unwrap()
        };
        let settings = PeerSettings {
            offer_swaps: false,
            swapping: false,
            ..settings
        };
        sim.step(index, |core| core.set_peer_settings(settings).unwrap());
    }
    for (a, b) in BANS.into_iter().chain([LATE]) {
        sim.step(a, |core| core.ban_node(key(b).node_id()).unwrap());
        sim.step(b, |core| core.ban_node(key(a).node_id()).unwrap());
    }
    sim.join_network(1);
    // Node 3 holds toward node 1 at least a packet for each of its two
    // peerings over the XRP hub, which its owner cannot take out: with
    // nothing left to spend, it still has the liquidity to take node 4
    // over the hub, and none to fund a hold toward it.
    let least = 2 * super::MAX_PACKET;
    if !run_sliced(sim, sim.now + READY_WITHIN_MS + METHODS_WITHIN_MS, |sim| {
        holding(sim, RIGHT, HOP) >= least
    }) {
        return Err(Outcome::liveness(
            "holding",
            format!(
                "node 3 held {} toward node 1 in the XRP hub, less than {least}",
                holding(sim, RIGHT, HOP)
            ),
        ));
    }
    take_out(sim, RIGHT)?;
    let (a, b) = LATE;
    sim.step(a, |core| core.unban_node(&key(b).node_id()).unwrap());
    sim.step(b, |core| core.unban_node(&key(a).node_id()).unwrap());
    if !run_sliced(sim, sim.now + READY_WITHIN_MS + METHODS_WITHIN_MS, |sim| {
        formed(sim).is_ok()
    }) {
        return Err(Outcome::liveness("formed", formed(sim).unwrap_err()));
    }
    sim.count("routes with two first hops");
    let all = places(sim);
    let seen = RefCell::new(Seen::new(sim));
    methods(sim, rng, &seen)?;
    bring_back(sim, &seen, RIGHT, sim.now + JOINED_WITHIN_MS)?;
    settle_down_in(sim, &all);
    attack(sim, rng, &seen)?;
    short(sim, rng, &seen)?;
    settle_down_in(sim, &all);
    first_failure([
        check_events(sim).map_err(|problem| Outcome::safety("events", problem)),
        check_across(sim),
        check_hubs(sim, &all),
        check_unconverted(sim).map_err(|problem| Outcome::safety("converted", problem)),
        check_weights_cost_fees(sim).map_err(|problem| Outcome::safety("weights", problem)),
    ])
}

/// Node 0 pays node 4 in XRP and node 4 pays node 0 in dollars: no node
/// converts, each packet crossing on the method of its payment's ledger.
/// Node 3, which can fund no hold in the XRP hub, refuses the packets in
/// XRP drawn to it, and earns nothing there: the payment completes through
/// node 2.
fn methods(sim: &mut Sim, rng: &mut SeededRng, seen: &RefCell<Seen>) -> Result<(), Outcome> {
    let first = start(
        sim,
        (SENDER, PAYEE),
        HUB,
        LEAST_PACKETS + rng.below(LEAST_PACKETS),
    )?;
    start(
        sim,
        (PAYEE, SENDER),
        HUB_DSUSD,
        LEAST_PACKETS + rng.below(LEAST_PACKETS),
    )?;
    run_seeing(sim, seen, sim.now + PAID_WITHIN_MS, paid);
    check_paid(sim, first, "methods")?;
    let refused =
        |seen: &Seen| seen.rejected((RIGHT, HOP), RIGHT, ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
    // Once node 2 earned node 1 a fee, node 1 draws node 3 only when it
    // explores: node 0 pays node 4 again until node 3 answered T04.
    for _ in 0..AGAIN {
        if refused(&seen.borrow()) != 0 {
            break;
        }
        let again = start(
            sim,
            (SENDER, PAYEE),
            HUB,
            LEAST_PACKETS + rng.below(LEAST_PACKETS),
        )?;
        run_seeing(sim, seen, sim.now + PAID_WITHIN_MS, paid);
        check_paid(sim, again, "methods")?;
        sim.count("payments made again until node 3 answered T04");
    }
    first_failure([
        check_unconverted(sim).map_err(|problem| Outcome::safety("converted", problem)),
        check_weights_cost_fees(sim).map_err(|problem| Outcome::safety("weights", problem)),
        check_carried(sim).map_err(|problem| Outcome::liveness("methods-carried", problem)),
    ])?;
    for ledger in [HUB, HUB_DSUSD] {
        if booked(sim, HOP, ledger, AccountName::RevenueFees) == 0 {
            return Err(Outcome::liveness(
                "fees",
                format!("node 1 earned no fee on {ledger}, where a payment crossed it"),
            ));
        }
    }
    sim.count("payments kept their ledger across nodes in both hubs");
    let fees = booked(sim, RIGHT, HUB, AccountName::RevenueFees);
    let weighed = weight(sim, HOP, RIGHT, HUB, sim.now);
    if fees != 0 || weighed != 0 {
        return Err(Outcome::safety(
            "refusing-hop",
            format!(
                "node 3, which could fund no hold in the XRP hub, earned {fees} in fees there \
                 and a weight of {weighed} in node 1's draws"
            ),
        ));
    }
    // Node 3 was drawn, and refused: otherwise it earned nothing only for
    // never having been tried.
    let refused = refused(&seen.borrow());
    if refused == 0 {
        return Err(Outcome::liveness(
            "refusing-hop",
            format!(
                "node 3, which could fund no hold in the XRP hub, answered no T04 in {} \
                 payments",
                AGAIN + 1
            ),
        ));
    }
    *sim.counts.entry(SHORT_OF_ROOM).or_default() += refused;
    sim.count("payments completed past a hop that refuses");
    Ok(())
}

/// Node 0, an attacker with node 4, sends itself packets through node 1
/// that node 4 refuses: node 1's weights for its first hops come out as
/// their decay alone makes them, Rejects counting for nothing.
fn attack(sim: &mut Sim, rng: &mut SeededRng, seen: &RefCell<Seen>) -> Result<(), Outcome> {
    let until = sim.now + ATTACK_MS;
    let hops = [LEFT, RIGHT];
    let before = hops.map(|hop| weight(sim, HOP, hop, HUB, until));
    let refused = |seen: &Seen| {
        hops.iter()
            .map(|&hop| seen.rejected((hop, HOP), PAYEE, ErrorCode::F02_UNREACHABLE))
            .sum::<u64>()
    };
    let refused_before = refused(&seen.borrow());
    let destination = format!("{}.attack", sim.core(PAYEE).own_address());
    let mut link = None;
    sim.step(SENDER, |core| {
        let token = core
            .create_token("attacker".into(), HUB.as_str(), None)
            .unwrap();
        link = Some(core.btp_connect(&token.secret).unwrap().link);
    });
    let link = link.ok_or_else(|| Outcome::liveness("attack", "node 0 stopped"))?;
    let amount = super::MAX_PACKET / 10;
    for request in 1..=ATTACK_PACKETS {
        let mut fulfillment = [0; 32];
        for chunk in fulfillment.chunks_mut(8) {
            chunk.copy_from_slice(&rng.next_u64().to_be_bytes());
        }
        let packet = Prepare {
            amount,
            expires_at: Timestamp {
                unix_ms: sim.local_now(SENDER) + PACKET_EXPIRY_MS,
            },
            execution_condition: dassie_ilp::condition_for(&fulfillment),
            destination: Address::parse(&destination).unwrap(),
            data: &[],
        }
        .to_vec();
        sim.step(SENDER, |core| {
            core.btp_packet(link, request, &packet, false)
        });
    }
    sim.trace.push(format!(
        "{} node 0 sends itself {ATTACK_PACKETS} packets through node 1 that node 4 refuses",
        sim.now
    ));
    run_seeing(sim, seen, until, |_| false);
    let attacked = refused(&seen.borrow()) - refused_before;
    if attacked == 0 {
        return Err(Outcome::liveness(
            "attack",
            "none of the attacker's packets reached its receiver through node 1",
        ));
    }
    *sim.counts.entry(ATTACK_REFUSED).or_default() += attacked;
    let after = hops.map(|hop| weight(sim, HOP, hop, HUB, until));
    if after != before {
        return Err(Outcome::safety(
            "rejects-weigh",
            format!(
                "node 1's weights for nodes 2 and 3 moved from {before:?} to {after:?} on \
                 packets their receiver refused"
            ),
        ));
    }
    Ok(())
}

/// Node 2's owner takes out of the XRP hub all its account can spend,
/// and node 0 pays node 4, again until node 2 has spent what its hold
/// toward node 4 had left: node 1 still draws node 2 by its weight, and it
/// answers T04, its packets moving to no other method. Its owner moves
/// funds back in, its hold is funded again, and the payments complete;
/// at every step of a window longer than all that, node 2's weight in node
/// 1 has lost only what decay takes, of what it had earned by then. Then
/// it carries packets again.
fn short(sim: &mut Sim, rng: &mut SeededRng, seen: &RefCell<Seen>) -> Result<(), Outcome> {
    take_out(sim, LEFT)?;
    let until = sim.now + SHORT_WINDOW_MS;
    // What node 2 earned node 1 before, as decay alone leaves it at the
    // window's end, is the least its weight may be until node 2 refuses.
    let earned = weight(sim, HOP, LEFT, HUB, until);
    if earned == 0 {
        return Err(Outcome::liveness(
            "short",
            "node 2 had earned node 1 nothing by the time it was short of room",
        ));
    }
    seen.borrow_mut().watch = Some(Watch {
        hop: LEFT,
        until,
        lowest: earned,
    });
    let count = |seen: &Seen| {
        (
            seen.rejected((LEFT, HOP), LEFT, ErrorCode::T04_INSUFFICIENT_LIQUIDITY),
            seen.prepared(LEFT, HUB_DSUSD),
        )
    };
    let (refusals, sideways) = count(&seen.borrow());
    let over_dsusd = sim.counts.get(PREPARES_OVER_DSUSD).copied().unwrap_or(0);
    let payment = start(
        sim,
        (SENDER, PAYEE),
        HUB,
        LEAST_PACKETS + rng.below(LEAST_PACKETS),
    )?;
    let deadline = sim.now + PAID_WITHIN_MS;
    let refused = |sim: &Sim| count(&seen.borrow()).0 > refusals || paid(sim);
    loop {
        run_seeing(sim, seen, deadline, refused);
        if count(&seen.borrow()).0 > refusals || sim.now >= deadline {
            break;
        }
        // Paid with no T04: node 2's hold toward node 4, funded before its
        // owner took the funds out, carried what node 1 drew to node 2, and
        // node 3 the rest. Node 0 pays again, until that hold is spent.
        start(
            sim,
            (SENDER, PAYEE),
            HUB,
            LEAST_PACKETS + rng.below(LEAST_PACKETS),
        )?;
        sim.count("payments made again until node 2 was short of room");
    }
    let refused = count(&seen.borrow()).0 - refusals;
    if refused == 0 {
        return Err(Outcome::liveness(
            "short",
            "node 2, short of room in the XRP hub, never answered T04",
        ));
    }
    // Node 2's first T04 is on its way to node 1, which has not taken it
    // yet. Until now its weight may not fall below what it had earned;
    // from now on not below what it has earned, the fees of the payments
    // made again included, so that those fees cannot hide a cut on the
    // refusal.
    let kept = weight(sim, HOP, LEFT, HUB, until);
    let lowest = {
        let mut seen = seen.borrow_mut();
        let watch = seen.watch.as_mut().unwrap();
        std::mem::replace(&mut watch.lowest, kept)
    };
    if lowest < earned {
        return Err(Outcome::safety(
            "short-weight",
            format!(
                "node 1's weight for node 2 fell from {earned} to {lowest} once node 2 was short \
                 of room, before it refused anything, beyond what decay takes"
            ),
        ));
    }
    bring_back(sim, seen, LEFT, until)?;
    run_seeing(sim, seen, until, paid);
    let lowest = seen
        .borrow_mut()
        .watch
        .take()
        .map_or(kept, |watch| watch.lowest);
    check_paid(sim, payment, "short of room")?;
    let (refused, moved) = count(&seen.borrow());
    let (refused, moved) = (refused - refusals, moved - sideways);
    *sim.counts.entry(SHORT_OF_ROOM).or_default() += refused;
    let over_dsusd = sim.counts.get(PREPARES_OVER_DSUSD).copied().unwrap_or(0) - over_dsusd;
    if moved != 0 || over_dsusd != 0 {
        return Err(Outcome::safety(
            "method-moved",
            format!(
                "packets in XRP went over the DSUSD hub while node 2 was short of room in the \
                 XRP hub: {moved} from node 2, {over_dsusd} in all"
            ),
        ));
    }
    if lowest < kept {
        return Err(Outcome::safety(
            "short-weight",
            format!(
                "node 1's weight for node 2 fell from {kept} to {lowest} while node 2 was short \
                 of room, beyond what decay takes"
            ),
        ));
    }
    sim.count("hops short of room kept their weight");
    // Drawn by that weight, it carries packets again.
    let fulfilled = seen.borrow().fulfilled(LEFT, HOP);
    let payment = start(
        sim,
        (SENDER, PAYEE),
        HUB,
        LEAST_PACKETS + rng.below(LEAST_PACKETS),
    )?;
    run_seeing(sim, seen, sim.now + PAID_WITHIN_MS, paid);
    check_paid(sim, payment, "room back")?;
    if seen.borrow().fulfilled(LEFT, HOP) == fulfilled {
        return Err(Outcome::liveness(
            "room-back",
            "node 2 carried no packet once its room was back",
        ));
    }
    sim.count("hops carried again once their room was back");
    Ok(())
}
