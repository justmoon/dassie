//! What a node simulation checks
//! (spec/areas/testing.md#each-node-judged-by-its-own-books): each node on its
//! own, by its own books, against what really happened
//! (spec/decisions/own-books.md). A check that fails says whether something
//! that must never happen did (safety), or something that must happen did not
//! (liveness).

use dassie_connector::{CapUsage, replay};
use dassie_kit::sim::Outcome;
use dassie_payments::amount::{paid_in_full, payment_tolerance};
use dassie_settlement::policy;

use dassie_ledger::LedgerId;

use super::{
    DSUSD, Drift, LIVE_XRP, Payment, RLUSD, START, Settling, Sim, TRIGGER, USD, Withdrawal, XRP,
    key, received, sent,
};
use crate::node::Policies;
use crate::store::{OutgoingState, PaymentId};

/// Every link's largest packet, which its thresholds are counted in
/// (spec/decisions/thresholds-in-packets.md).
const MAX_PACKET: u64 = Policies::DEFAULT.connector.max_packet_amount;

/// A capped token spent at most its cap plus what refilled, over every
/// period the journal shows, and its checkpoint is the journal's replay.
pub fn check_cap(sim: &Sim, payment: &Payment) -> Result<(), String> {
    let (id, Some(cap)) = payment.token else {
        return Ok(());
    };
    let store = &sim.nodes[payment.from].store;
    let row = store
        .btp_tokens()
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    let ledger = payment.ledger;
    let path = format!("{ledger}:equity/owner/btp/{id}");
    let mut spent: Vec<(u64, u64)> = Vec::new();
    let mut after = None;
    loop {
        let page = store.transfers_after(after, 1_000, None).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.sequence);
        spent.extend(
            page.iter()
                .filter(|row| row.debit_path == path)
                .filter(|row| {
                    !row.credit_path
                        .starts_with(&format!("{ledger}:equity/owner"))
                })
                .filter(|row| row.state == dassie_ledger::TransferState::Posted)
                .map(|row| (row.resolved_at_ms.unwrap(), row.transfer.amount)),
        );
    }
    spent.sort_unstable();
    let total: u64 = spent.iter().map(|(_, amount)| amount).sum();
    // Less is a payment still under way, or one whose receiver had it all
    // before its sender counted it (spec/decisions/own-fulfills.md): what the
    // token spent is held to what the client sent by the owner's books.
    if total > payment.amount {
        return Err(format!(
            "the capped token spent {total} of {}",
            payment.amount
        ));
    }
    let window = u128::from(cap.window_ms);
    for (i, &(start, _)) in spent.iter().enumerate() {
        let mut total = 0u128;
        for &(at, amount) in &spent[i..] {
            total += u128::from(amount);
            let allowed = u128::from(cap.amount) * (window + u128::from(at - start));
            if total * window > allowed {
                return Err(format!(
                    "the capped token spent {total} from {start} to {at}"
                ));
            }
        }
    }
    let expected = replay(cap, CapUsage::new(row.created_at_ms), spent.iter().copied());
    if row.cap_usage != expected {
        return Err(format!(
            "the cap's checkpoint {:?} is not the journal's {expected:?}",
            row.cap_usage
        ));
    }
    Ok(())
}

/// An incoming payment received at least what its sender sent, more only
/// what hops lost (checked over all payments), never more than it asked
/// for beyond its tolerance, and is complete once it has that, to within
/// its tolerance (spec/decisions/paid-within-a-tolerance.md). Less than
/// was sent, or more than it may take, is a safety failure; not yet all it
/// asked for, liveness.
pub fn check_incoming(sim: &Sim, payment: &Payment) -> Result<(), Outcome> {
    let row = sim.nodes[payment.to]
        .store
        .incoming_payment(payment.incoming)
        .unwrap()
        .ok_or_else(|| Outcome::safety("incoming", "an incoming payment vanished"))?;
    let sent = sent(payment);
    if row.received_amount < sent {
        return Err(Outcome::safety(
            "incoming",
            format!(
                "node {} received {} of the {sent} sent",
                payment.to, row.received_amount
            ),
        ));
    }
    // One that expired first got what it got, never more than its limit.
    let scale = row.ledger.asset().scale;
    let full = paid_in_full(payment.limit, row.received_amount, scale);
    let ceiling = payment
        .limit
        .map(|limit| limit + payment_tolerance(Some(limit), scale));
    if let (Some(limit), Some(ceiling)) = (payment.limit, ceiling)
        && (row.received_amount > ceiling
            || !payment.expired && (!full || row.completed_at_ms.is_none()))
    {
        let problem = format!(
            "an incoming payment of {limit} received {} (completed {:?})",
            row.received_amount, row.completed_at_ms
        );
        return Err(if row.received_amount > ceiling {
            Outcome::safety("incoming", problem)
        } else {
            Outcome::liveness("incoming", problem)
        });
    }
    Ok(())
}

/// Each owner's pool on the ledger the run pays over moved by exactly what
/// its incoming payments received, and each live client's token by what it
/// sent: each node's books against what its own clients and receivers
/// really moved.
pub fn check_owner_books(sim: &Sim) -> Result<(), String> {
    let all = || sim.payments.iter().chain(&sim.abandoned);
    for index in 0..sim.nodes.len() {
        let balances = sim.balances_on(index, sim.settling.ledger());
        let pool = balances["equity/owner"];
        let received: i128 = all()
            .filter(|payment| payment.to == index)
            .map(|payment| i128::from(received(sim, payment)))
            .sum::<i128>()
            + outgoing_flows(sim)
                .filter(|flow| flow.to == index)
                .map(|flow| i128::from(flow.received))
                .sum::<i128>();
        let deposit = sim.deposit(index);
        if pool - deposit != received {
            return Err(format!(
                "node {index}'s owner gained {} for {received} received",
                pool - deposit
            ));
        }
        for payment in sim.payments.iter().filter(|payment| payment.from == index) {
            let token = -balances[&format!("equity/owner/btp/{}", payment.token.0)];
            if token != i128::from(sent(payment)) {
                return Err(format!(
                    "node {index}'s token {} paid {token} for {} sent",
                    payment.token.0,
                    sent(payment)
                ));
            }
        }
        let paid: i128 = outgoing_flows(sim)
            .filter(|flow| flow.from == index)
            .map(|flow| i128::from(flow.sent))
            .sum();
        let payer = -balances.get("equity/owner/local/pay").copied().unwrap_or(0);
        if payer != paid {
            return Err(format!(
                "node {index}'s payer paid {payer} for {paid} its outgoing payments sent"
            ));
        }
    }
    Ok(())
}

/// What an outgoing payment sent, and what its receiver got.
pub struct Flow {
    /// The paying node.
    pub from: usize,
    /// The node paid.
    pub to: usize,
    /// What it sent.
    pub sent: u64,
    /// What its receiver got.
    pub received: u64,
}

/// Every outgoing payment's flow.
pub fn outgoing_flows(sim: &Sim) -> impl Iterator<Item = Flow> + '_ {
    sim.outgoing
        .iter()
        .map(|&(from, to, incoming, outgoing)| Flow {
            from,
            to,
            sent: sim.nodes[from]
                .store
                .outgoing_payment(outgoing)
                .unwrap()
                .map_or(0, |row| row.sent_amount),
            received: sim.nodes[to]
                .store
                .incoming_payment(incoming)
                .unwrap()
                .map_or(0, |row| row.received_amount),
        })
}

/// Once the network is calm, no node's books show an honest peer still at
/// its ask point on any method: asked, the peer settled
/// (spec/decisions/own-fulfills.md,
/// spec/areas/settlement.md#asking-a-peer-to-settle). Over a hub of XRP,
/// as the runs over a hub check it.
pub fn check_settled(sim: &Sim) -> Result<(), String> {
    if is_hub_run(sim) {
        return super::hub_checks::check_settled_with_honest(sim, None);
    }
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            if peer.termination.is_some() {
                continue;
            }
            for (method, owed) in peer.peering.methods.iter().zip(&peer.owed_by_method) {
                let Some(owed) = *owed else {
                    continue;
                };
                let credit = method.credit_out;
                let position = u128::try_from(owed + i128::from(credit)).unwrap();
                if policy::ask(position, TRIGGER) {
                    return Err(format!(
                        "node {index}'s room for {} on {} is {position} of {credit}: it never \
                         settled",
                        peer.id, method.scheme,
                    ));
                }
            }
        }
    }
    Ok(())
}

/// A link whose books drifted, by what the simulation saw: its two nodes,
/// how far, and how far it survives.
struct Drifted {
    nodes: (usize, usize),
    drift: Drift,
    survives: u128,
    /// Whether one side's room for the other is at its trigger.
    short: bool,
}

impl Drifted {
    fn describe(&self) -> String {
        let (a, b) = self.nodes;
        format!(
            "crashes drifted the books of nodes {a} and {b} apart by {}, and other lost \
             Fulfills by {}; their credit survives {}",
            self.drift.crashes, self.drift.others, self.survives
        )
    }
}

/// Every pair of peered nodes whose books drifted at all.
fn drifted(sim: &mut Sim) -> Vec<Drifted> {
    let count = sim.nodes.len();
    let mut links = Vec::new();
    for a in 0..count {
        for b in a + 1..count {
            let drift = sim.drift(a, b);
            if drift == Drift::default() {
                continue;
            }
            let (Some((credit_a, room_a)), Some((credit_b, room_b))) =
                (room(sim, a, b), room(sim, b, a))
            else {
                continue;
            };
            let (short_a, short_b) = (policy::ask(room_a, TRIGGER), policy::ask(room_b, TRIGGER));
            let survives = match (short_a, short_b) {
                (true, false) => survives(sim, credit_a, b, credit_b),
                (false, true) => survives(sim, credit_b, a, credit_a),
                _ => survives(sim, credit_a, b, credit_b).min(survives(sim, credit_b, a, credit_a)),
            };
            links.push(Drifted {
                nodes: (a, b),
                drift,
                survives,
                short: short_a || short_b,
            });
        }
    }
    links
}

/// How far a link survives drift when the side that extends `asker` asks
/// node `payer`, which extends `credit`, to settle: asked at the asker's
/// trigger, the payer settles only if its books leave it its own trigger
/// plus twice the least settlement, and over a hub it answers as if it
/// extended its default credit (spec/decisions/crash-drift.md).
fn survives(sim: &Sim, asker: u64, payer: usize, credit: u64) -> u128 {
    let over_hub =
        matches!(sim.settling, Settling::StubHub(_)) || sim.settling.hub_validators().is_some();
    let answers = if over_hub {
        sim.credits
            .get(&payer)
            .copied()
            .unwrap_or(Policies::DEFAULT.peer_credit)
    } else {
        credit
    };
    let settles = 2 * u128::from(TRIGGER) + 2 * u128::from(policy::minimum(MAX_PACKET));
    (u128::from(asker) + u128::from(answers)).saturating_sub(settles)
}

/// A link that crashes drifted past what it survives, stuck as the
/// decisions accept it: one side's room for the other is at its trigger,
/// and the other's books leave it nothing to settle
/// (spec/decisions/crash-drift.md). Only drift the simulation saw counts,
/// and only what crashes lost: a link that refused, late or cut-off
/// Fulfills drifted is never excused (spec/decisions/unreachable-peers.md).
pub fn drifted_past(sim: &mut Sim) -> Option<String> {
    drifted(sim)
        .into_iter()
        .find(|link| link.short && link.drift.crashes > link.survives)
        .map(|link| link.describe())
}

/// The credit node `index` extends node `other` on their peering's first
/// method, and its room for it there, while they are peered.
fn room(sim: &Sim, index: usize, other: usize) -> Option<(u64, u128)> {
    let id = key(other).node_id();
    let peer = sim.nodes[index]
        .core
        .as_ref()?
        .network()
        .peers
        .into_iter()
        .find(|peer| peer.id == id)?;
    let owed = peer.owed?;
    let credit = peer.peering.first().credit_out;
    Some((
        credit,
        u128::try_from(owed + i128::from(credit)).unwrap_or(0),
    ))
}

/// What a healed network that did not settle down within `seconds` found
/// (spec/areas/testing.md#outcomes): a run whose crashes drifted a link
/// past what it survives drew what the design gives up on, which no code
/// recovers from; anything else is a liveness failure, which names any
/// link that drifted past what it survives for other reasons.
pub fn not_settled_down(sim: &mut Sim, seconds: u64, what: &str) -> Outcome {
    let stuck = format!("healed, the network did not settle down in {seconds} s: {what}");
    if let Some(drifted) = drifted_past(sim) {
        sim.count("links drifted past what they survive");
        return Outcome::unsupported(
            "drifted-past-what-it-survives",
            format!("{stuck}; {drifted}"),
        );
    }
    let past: Vec<String> = drifted(sim)
        .iter()
        .filter(|link| link.drift.crashes + link.drift.others > link.survives)
        .map(Drifted::describe)
        .collect();
    if past.is_empty() {
        Outcome::liveness("settle-down", stuck)
    } else {
        Outcome::liveness("settle-down", format!("{stuck}; {}", past.join("; ")))
    }
}

/// Routine settlement never withdraws an honest peer for room: only a peer
/// that stays short, settling nothing, past the grace
/// (spec/decisions/peering-lifecycle.md). An issuer's freeze of its token
/// stops what the nodes it touches pay or are paid in it, so while it
/// holds, and for a grace after it is lifted, a pair it touches that
/// settles in the token may be withdrawn; a pair that settles in XRP may not
/// (spec/areas/settlement.md#when-peers-cannot-pay-us). So does a stretch in
/// which the ledger's servers or validators fail a node, or every node: a
/// node that cannot see its ledger settles nothing, honestly, so a pair it
/// is in may be withdrawn, until a grace after the stretch.
pub fn check_withdrawn(sim: &Sim) -> Result<(), String> {
    let grace = sim.peer.advertise_short_grace_ms;
    let token = sim
        .settling
        .token()
        .map(|(ledger, _)| Settling::scheme(ledger).0);
    let id = |index: usize| key(index).node_id();
    let frozen = |withdrawal: &Withdrawal| {
        let on_token = withdrawal
            .scheme
            .is_some_and(|scheme| Some(scheme.as_str()) == token);
        on_token
            && sim.freezes.iter().any(|&(freeze, from, until)| {
                let touched = (0..sim.nodes.len()).any(|index| {
                    freeze.stops(index) && (index == withdrawal.by || id(index) == withdrawal.peer)
                });
                touched
                    && withdrawal.at >= from
                    && until.is_none_or(|until| withdrawal.at <= until + grace)
            })
    };
    let unseen = |withdrawal: &Withdrawal| {
        sim.outages.iter().any(|&(node, from, until)| {
            node.is_none_or(|index| index == withdrawal.by || id(index) == withdrawal.peer)
                && withdrawal.at >= from
                && withdrawal.at <= until + grace
        })
    };
    match (sim.withdrawn.iter()).find(|withdrawal| !frozen(withdrawal) && !unseen(withdrawal)) {
        Some(withdrawal) => Err(format!(
            "node {} withdrew {} for room at {}",
            withdrawal.by,
            withdrawal.peer,
            withdrawal.at - START
        )),
        None => Ok(()),
    }
}

/// No node reported an error, nor lost an event.
pub fn check_events(sim: &Sim) -> Result<(), String> {
    match sim.errors.as_slice() {
        [] => Ok(()),
        errors => Err(format!(
            "{} error events, the first: {}",
            errors.len(),
            errors
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

/// What must never happen, even with packets in flight: a capped token
/// spent past its cap, a payment received more than it asked for, or a
/// payment vanished. A run that did not settle down checks these, and
/// fails for safety, before it fails for liveness.
pub fn check_mid_flight(sim: &Sim) -> Result<(), Outcome> {
    check_hub_packets(sim).map_err(|problem| Outcome::safety("hub-packets", problem))?;
    check_hub_credit(sim).map_err(|problem| Outcome::safety("hub-credit", problem))?;
    for payment in &sim.payments {
        check_cap(sim, payment).map_err(|problem| Outcome::safety("cap", problem))?;
        let row = sim.nodes[payment.to]
            .store
            .incoming_payment(payment.incoming)
            .unwrap()
            .ok_or_else(|| Outcome::safety("mid-flight", "an incoming payment vanished"))?;
        if let Some(limit) = payment.limit
            && row.received_amount
                > limit + payment_tolerance(Some(limit), row.ledger.asset().scale)
        {
            return Err(Outcome::safety(
                "mid-flight",
                format!(
                    "an incoming payment of {limit} received {}",
                    row.received_amount
                ),
            ));
        }
    }
    for &(from, to, incoming, outgoing) in &sim.outgoing {
        let row = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .ok_or_else(|| Outcome::safety("mid-flight", "an outgoing payment vanished"))?;
        let received = sim.nodes[to]
            .store
            .incoming_payment(incoming)
            .unwrap()
            .map_or(0, |row| row.received_amount);
        let asked = &row.receive_amount;
        if received > asked.value + payment_tolerance(Some(asked.value), asked.asset_scale) {
            return Err(Outcome::safety(
                "mid-flight",
                format!(
                    "node {to} received {received}, more than the {} it asked for",
                    row.receive_amount.value
                ),
            ));
        }
    }
    Ok(())
}

/// No node's stub with commitments held, after any step, a packet in flight
/// that its connector no longer held pending with that peer: each packet
/// it committed to or took a commitment for was resolved in the scheme
/// when the connector let it go, answered, decided or never sent
/// (spec/areas/hubs.md#in-the-node).
pub fn check_hub_packets(sim: &Sim) -> Result<(), String> {
    match sim.hub_leaks.as_slice() {
        [] => Ok(()),
        leaks => Err(format!(
            "stubs held packets past their connectors with {} peers: {}",
            leaks.len(),
            leaks.join("; ")
        )),
    }
}

/// No node's credit to a peer over the stub with commitments was, after
/// any step, other than its hold toward the peer and its settlements to it
/// still on their way (spec/areas/hubs.md#in-the-node).
pub fn check_hub_credit(sim: &Sim) -> Result<(), String> {
    match sim.hub_credit_off.as_slice() {
        [] => Ok(()),
        off => Err(format!(
            "credit over a hub was apart from the holds with {} peers: {}",
            off.len(),
            off.join("; ")
        )),
    }
}

/// The first safety failure among `results`, or else the first other
/// failure: a run that broke something that must never happen says so,
/// whatever else it failed to do.
pub fn first_failure(
    results: impl IntoIterator<Item = Result<(), Outcome>>,
) -> Result<(), Outcome> {
    let mut other = None;
    for result in results {
        match result {
            Ok(()) => {}
            Err(failure @ Outcome::Safety(_)) => return Err(failure),
            Err(failure) => {
                other.get_or_insert(failure);
            }
        }
    }
    other.map_or(Ok(()), Err)
}

/// Everything a run over one ledger checks once the network is calm. Every
/// check runs, and a run that broke both says safety.
pub fn check(sim: &Sim) -> Result<(), Outcome> {
    let caps = sim
        .payments
        .iter()
        .map(|payment| check_cap(sim, payment).map_err(|problem| Outcome::safety("cap", problem)));
    let incoming = sim
        .payments
        .iter()
        .map(|payment| check_incoming(sim, payment));
    first_failure(
        [
            check_events(sim).map_err(|problem| Outcome::safety("events", problem)),
            check_hub_packets(sim).map_err(|problem| Outcome::safety("hub-packets", problem)),
            check_hub_credit(sim).map_err(|problem| Outcome::safety("hub-credit", problem)),
        ]
        .into_iter()
        .chain(caps)
        .chain([
            check_books(sim),
            check_finished(sim).map_err(|problem| Outcome::liveness("finished", problem)),
            check_outgoing(sim),
        ])
        .chain(incoming)
        .collect::<Vec<_>>(),
    )
}

/// Every payment's client sent all it meant to.
pub fn check_finished(sim: &Sim) -> Result<(), String> {
    let Some(payment) = sim.payments.iter().find(|payment| !payment.done) else {
        return Ok(());
    };
    Err(format!(
        "a payment from node {} to {} did not finish: sent {} of {} (limit {:?}), {:?}",
        payment.from,
        payment.to,
        sent(payment),
        payment.amount,
        payment.limit,
        payment.connection.stats(),
    ) + &format!(
        ", received {:?}, remote received {:?}, closed {}",
        sim.nodes[payment.to]
            .store
            .incoming_payment(payment.incoming)
            .unwrap()
            .map(|row| row.received_amount),
        payment
            .stream
            .and_then(|id| payment.connection.stream(id))
            .map(|stream| stream.remote_received()),
        payment.connection.is_closed(),
    ))
}

/// Each outgoing payment completed, having sent its debit amount or what
/// its receiver still lacked when a hop lost a packet, and its receiver
/// got what it asked for. A receiver that got less than was sent, or more
/// than it asked for, is a safety failure; a payment that did not complete,
/// liveness, unless its payer gave up while the network was faulty.
pub fn check_outgoing(sim: &Sim) -> Result<(), Outcome> {
    first_failure(
        sim.outgoing
            .iter()
            .map(|&(from, to, incoming, outgoing)| {
                check_outgoing_payment(sim, from, to, incoming, outgoing)
            })
            .collect::<Vec<_>>(),
    )
}

/// One outgoing payment, as [`check_outgoing`] checks it.
fn check_outgoing_payment(
    sim: &Sim,
    from: usize,
    to: usize,
    incoming: PaymentId,
    outgoing: PaymentId,
) -> Result<(), Outcome> {
    let row = sim.nodes[from]
        .store
        .outgoing_payment(outgoing)
        .unwrap()
        .ok_or_else(|| Outcome::safety("outgoing", "an outgoing payment vanished"))?;
    let received = sim.nodes[to]
        .store
        .incoming_payment(incoming)
        .unwrap()
        .map_or(0, |row| row.received_amount);
    if received < row.sent_amount {
        return Err(Outcome::safety(
            "outgoing",
            format!(
                "node {to} received {received} of the {} node {from} sent",
                row.sent_amount
            ),
        ));
    }
    // A receiver may take up to the tolerance more, as when a Fulfill lost
    // to a crash is sent again (spec/decisions/paid-within-a-tolerance.md).
    let asked = &row.receive_amount;
    let tolerance = payment_tolerance(Some(asked.value), asked.asset_scale);
    if received > asked.value + tolerance {
        return Err(Outcome::safety(
            "outgoing",
            format!(
                "node {to} received {received}, more than the {} it asked for",
                row.receive_amount.value
            ),
        ));
    }
    // One whose payer gave up while the network was faulty may have
    // failed; what it sent still arrived.
    if row.state == OutgoingState::Failed && sim.given_up.contains(&outgoing) {
        return Ok(());
    }
    if row.state != OutgoingState::Completed {
        let incoming_row = sim.nodes[to].store.incoming_payment(incoming).unwrap();
        return Err(Outcome::liveness(
            "outgoing",
            format!(
                "an outgoing payment from {from} to {to} ended {:?} ({:?}) having sent {} of {}; its receiver: {incoming_row:?}",
                row.state, row.failure, row.sent_amount, row.debit_amount
            ),
        ));
    }
    // Completed, it has all it asked for, to within the tolerance.
    if !paid_in_full(Some(asked.value), received, asked.asset_scale) {
        return Err(Outcome::liveness(
            "outgoing",
            format!(
                "node {to} received {received} of the {} node {from} sent, for {}",
                row.sent_amount, row.receive_amount.value
            ),
        ));
    }
    Ok(())
}

/// Whether the run is over a hub of XRP run in the simulation, or two hubs,
/// whose members fund their accounts in them from their owner books on the
/// XRP Ledger: its books are the hubs' to check.
fn is_hub_run(sim: &Sim) -> bool {
    matches!(sim.settling, Settling::Hub(_) | Settling::Hubs(_))
}

/// Each node's books, on their own: they hold what its scheme holds, and
/// it settled what it owed. The underlying ledgers neither made nor lost
/// money. No check compares two nodes' books, or adds them up: each keeps
/// its own, and they may drift (spec/decisions/own-books.md). Over a hub of
/// XRP, as the runs over a hub check them, in the hub and on the XRP Ledger
/// account each member funds its hub account from.
pub fn check_books(sim: &Sim) -> Result<(), Outcome> {
    if is_hub_run(sim) {
        return super::hub_checks::check_books(sim);
    }
    first_failure([
        check_owner_books(sim).map_err(|problem| Outcome::safety("owner-books", problem)),
        check_ledgers(sim).map_err(|problem| Outcome::safety("ledgers", problem)),
        check_true(sim).map_err(|problem| Outcome::safety("proven", problem)),
        check_funded(sim).map_err(|problem| Outcome::liveness("funded", problem)),
        check_anchored(sim).map_err(|problem| Outcome::liveness("anchored", problem)),
        check_pegged(sim).map_err(|problem| Outcome::liveness("pegged", problem)),
        check_settled(sim).map_err(|problem| Outcome::liveness("settled", problem)),
        check_delivered(sim).map_err(|problem| Outcome::liveness("delivered", problem)),
    ])
}

/// Each node's books hold to the fake ledger's truth, whatever its servers
/// lied (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation):
/// nothing booked that the fake did not apply, no settlement cancelled that
/// it applied, and nothing written off, since the fake never resets in a
/// simulation. Each would leave the books apart from the account, which the
/// scheme could only reconcile without a reason it can name: so no node
/// reconciled anything but a change of the reserve.
pub fn check_true(sim: &Sim) -> Result<(), String> {
    match sim.unexplained.first() {
        Some(first) => Err(format!(
            "a node's books left its ledger, unexplained: {first}"
        )),
        None => Ok(()),
    }
}

/// Every node on the XRP Ledger anchored: its account took a ledger its
/// UNL validated, so it proved what it booked rather than taking a
/// server's word (spec/designs/xrpl-validation.md#1-the-rule).
pub fn check_anchored(sim: &Sim) -> Result<(), String> {
    if !sim.settling.on_xrpl() {
        return Ok(());
    }
    for index in 0..sim.nodes.len() {
        let Some(core) = sim.nodes[index].core.as_ref() else {
            continue;
        };
        if core
            .xrpl_accounts()
            .any(|account| account.anchor().is_none())
        {
            return Err(format!("node {index}'s XRPL account never anchored"));
        }
    }
    Ok(())
}

/// Whether node `index` books `ledger`: one it pays over, or one of its
/// owner books, in a run of them or one a hub's member funds its account in
/// the hub from, and pays the escrow's gas from.
fn books(sim: &Sim, index: usize, ledger: LedgerId) -> bool {
    sim.settling.books(index, ledger)
}

/// On the XRP Ledger, the faucet funded every node, and on a token the
/// issuer minted to every node: an owner's books that match a deposit of
/// nothing would pass the other checks.
pub fn check_funded(sim: &Sim) -> Result<(), String> {
    // A node on either XRPL scheme has an account the faucet funds.
    let on_xrpl = |index: &usize| {
        [XRP, LIVE_XRP, RLUSD, DSUSD]
            .into_iter()
            .any(|ledger| books(sim, *index, ledger))
    };
    if let Some(index) = (0..sim.nodes.len())
        .filter(on_xrpl)
        .find(|index| !sim.fundings.contains_key(index))
    {
        return Err(format!("node {index} was never funded"));
    }
    let Some((token, _)) = sim.settling.token() else {
        return Ok(());
    };
    let minted = |index: &usize| sim.issued.get(index).is_some_and(|(minted, _)| *minted > 0);
    match (0..sim.nodes.len())
        .filter(|index| books(sim, *index, token))
        .find(|index| !minted(index))
    {
        Some(index) => Err(format!("node {index} was never minted on {token}")),
        None => Ok(()),
    }
}

/// On a token, every node's peg guard read the token's pool, which stays at
/// a dollar at the run's rates, and lets its scheme run
/// (spec/decisions/issued-assets.md).
pub fn check_pegged(sim: &Sim) -> Result<(), String> {
    let Some((token, _)) = sim.settling.token() else {
        return Ok(());
    };
    for index in (0..sim.nodes.len()).filter(|index| books(sim, *index, token)) {
        let peg = sim
            .core(index)
            .ledger_views()
            .into_iter()
            .find(|view| view.id == token)
            .and_then(|view| view.peg)
            .ok_or_else(|| format!("node {index} has no peg guard on {token}"))?;
        if peg.paused.is_some() || peg.last.map(|(price, _)| price) != Some(1_000_000) {
            return Err(format!("node {index}'s peg guard on {token} says {peg:?}"));
        }
    }
    Ok(())
}

/// No settlement is still on its way.
pub fn check_delivered(sim: &Sim) -> Result<(), String> {
    for index in 0..sim.nodes.len() {
        let network = sim.core(index).network();
        if let Some((peer, _)) = network
            .channels
            .iter()
            .find(|(_, channel)| channel.outbox.is_some())
        {
            return Err(format!(
                "node {index}'s settlement to {peer} is still on its way"
            ));
        }
    }
    Ok(())
}

/// Node `index`'s books on `ledger` hold what its scheme holds, on the XRP
/// Ledger exactly its account's balance less the reserve, and on a token
/// exactly its line in whole units; returns what they hold.
pub fn check_held(sim: &Sim, index: usize, ledger: LedgerId) -> Result<i128, String> {
    // The books follow the scheme's balance: what settled was posted.
    let view = sim
        .core(index)
        .ledger_views()
        .into_iter()
        .find(|view| view.id == ledger)
        .ok_or_else(|| format!("node {index} has no {ledger}"))?;
    let booked = -sim.balances_on(index, ledger)["assets/settlement"];
    if booked != i128::from(view.on_ledger_balance) {
        return Err(format!(
            "node {index} books {booked} in settlement on {ledger}, its scheme holds {}",
            view.on_ledger_balance
        ));
    }
    match ledger {
        XRP | LIVE_XRP => {
            let drops = sim.ledger.balance(sim.xrpl_account(index)).unwrap_or(0);
            let bookable = i128::from(drops) * 1_000 - i128::from(view.reserve);
            if booked != bookable.max(0) {
                return Err(format!(
                    "node {index} books {booked}; its account holds {drops} drops"
                ));
            }
        }
        RLUSD | DSUSD => {
            let line = sim.line(index).unwrap_or(0).max(0);
            if booked != line {
                return Err(format!(
                    "node {index} books {booked} on {ledger}; its line holds {line}"
                ));
            }
        }
        _ => {}
    }
    Ok(booked)
}

/// Neither XRP nor a token was made or lost on the fake ledger: its XRP
/// is all there is, and a token's holders hold what its issuer minted, less
/// what came back to it.
fn check_fake_ledger(sim: &Sim) -> Result<(), String> {
    if sim.ledger.total() != dassie_xrpl::MAX_DROPS {
        return Err("the ledger made or lost XRP".into());
    }
    if !sim.ledger.tokens_conserved() {
        return Err("the ledger made or lost a token".into());
    }
    Ok(())
}

/// Each node books in settlement what its scheme holds, on the XRP Ledger
/// exactly its account's balance less the reserve, on a token exactly its
/// line, and the underlying ledger neither made nor lost money. On the stub
/// with unlimited trust nothing settles, so each node's stub holds what it
/// started with.
pub fn check_ledgers(sim: &Sim) -> Result<(), String> {
    let count = sim.nodes.len();
    let ledger = sim.settling.ledger();
    let trusting = matches!(sim.settling, Settling::Trusting | Settling::OwnerBooks);
    let mut stubs = 0i128;
    for index in 0..count {
        let held = check_held(sim, index, ledger)?;
        if trusting && held != Settling::deposit(USD) {
            return Err(format!(
                "node {index}'s stub holds {held} on unlimited trust, which never settles"
            ));
        }
        stubs += held;
    }
    if ledger == USD {
        let total = Settling::deposit(USD) * i128::try_from(count).unwrap();
        if stubs != total {
            return Err(format!("the stubs hold {stubs}, not {total}"));
        }
    }
    if sim.settling.on_xrpl() {
        check_fake_ledger(sim)?;
    }
    Ok(())
}

/// A run across currencies: every payment finished, each node settled
/// what it owed and took every settlement, and no ledger made or lost an
/// asset. Each node is judged by its own books (spec/decisions/own-books.md).
pub fn check_across(sim: &Sim) -> Result<(), Outcome> {
    let outgoing = sim.outgoing.iter().map(|&(from, to, _, outgoing)| {
        let row = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .ok_or_else(|| Outcome::safety("outgoing", "an outgoing payment vanished"))?;
        if row.state != OutgoingState::Completed || row.sent_amount != row.debit_amount {
            return Err(Outcome::liveness("outgoing", format!(
                "an outgoing payment from {from} to {to} ended {:?} ({:?}) having sent {} of {}",
                row.state, row.failure, row.sent_amount, row.debit_amount
            )));
        }
        Ok(())
    });
    first_failure(
        [
            check_events(sim).map_err(|problem| Outcome::safety("events", problem)),
            check_received_across(sim)
                .map_err(|problem| Outcome::safety("received-across", problem)),
            check_prices(sim).map_err(|problem| Outcome::safety("prices", problem)),
            check_assets(sim).map_err(|problem| Outcome::safety("assets", problem)),
            check_true(sim).map_err(|problem| Outcome::safety("proven", problem)),
            check_funded(sim).map_err(|problem| Outcome::liveness("funded", problem)),
            check_anchored(sim).map_err(|problem| Outcome::liveness("anchored", problem)),
            check_pegged(sim).map_err(|problem| Outcome::liveness("pegged", problem)),
            check_finished(sim).map_err(|problem| Outcome::liveness("finished", problem)),
            check_settled(sim).map_err(|problem| Outcome::liveness("settled", problem)),
        ]
        .into_iter()
        .chain(outgoing)
        .collect::<Vec<_>>(),
    )
}

/// What arrived is what was sent at the rates, moved by the converting
/// node's curve, less what the hops charged; more only if a hop lost a
/// Fulfill to a crash.
fn check_received_across(sim: &Sim) -> Result<(), String> {
    for payment in &sim.payments {
        let into = sim.nodes[payment.to]
            .store
            .incoming_payment(payment.incoming)
            .unwrap()
            .map_or(*sim.settling.ledgers(payment.to).last().unwrap(), |row| {
                row.ledger
            });
        let (got, sent) = (
            u128::from(received(sim, payment)),
            u128::from(sent(payment)),
        );
        let least = u128::from(super::least_across(
            payment.ledger,
            into,
            u64::try_from(sent).unwrap(),
        ));
        if got < least {
            return Err(format!(
                "node {} received {got} of node {}'s {sent}, {:?} to {into:?}",
                payment.to, payment.from, payment.ledger
            ));
        }
    }
    Ok(())
}

/// No node priced a conversion above spot by more than its curve allows,
/// out of a ledger its owner held nothing on, or better than at what its
/// owner had posted alone, after any step
/// (spec/decisions/passive-rebalancing.md).
fn check_prices(sim: &Sim) -> Result<(), String> {
    match sim.priced.as_slice() {
        [] => Ok(()),
        breaches => Err(format!(
            "{} prices beyond the curve, the first: {}",
            breaches.len(),
            breaches[0]
        )),
    }
}

/// No ledger made or lost an asset, each node's books on the fake ledger
/// hold what its account and its line hold, and node 1 charged for what
/// passed between its peers.
fn check_assets(sim: &Sim) -> Result<(), String> {
    let count = sim.nodes.len();
    let ledgers: &[LedgerId] = match sim.settling {
        Settling::HubAcross(_) | Settling::Hubs(_) => &[super::HUB_DSUSD, super::HUB],
        _ => &[USD, XRP],
    };
    for &ledger in ledgers {
        let (mut schemes, mut nodes) = (0i128, 0i128);
        for index in (0..count).filter(|i| sim.settling.ledgers(*i).contains(&ledger)) {
            nodes += 1;
            schemes += if ledger == USD {
                -sim.balances_on(index, ledger)["assets/settlement"]
            } else {
                check_held(sim, index, ledger)?
            };
        }
        if ledger == USD && schemes != Settling::deposit(USD) * nodes {
            return Err(format!(
                "{ledger} was made or lost: the schemes hold {schemes}"
            ));
        }
    }
    if sim.settling.on_xrpl() {
        check_fake_ledger(sim)?;
    }
    let fees: i128 = ledgers
        .iter()
        .filter_map(|ledger| sim.balances_on(1, *ledger).get("revenue/fees").copied())
        .sum();
    let through_one =
        sim.peers(0).contains(&key(1).node_id()) || sim.peers(2).contains(&key(1).node_id());
    if through_one && fees <= 0 && count == 3 {
        return Err("node 1 charged nothing for what passed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_safety_failure_ranks_before_an_earlier_liveness_failure() {
        let liveness = || Err(Outcome::liveness("settled", "late"));
        let safety = || Err(Outcome::safety("owner-books", "broken"));
        assert_eq!(first_failure([Ok(()), liveness(), safety()]), safety());
        assert_eq!(first_failure([liveness(), Ok(())]), liveness());
        assert_eq!(first_failure([Ok(()), Ok(())]), Ok(()));
    }
}
