//! What a node simulation checks
//! (spec/areas/testing.md#each-node-judged-by-its-own-books): each node on its
//! own, by its own books, against what really happened (D19). A check that fails
//! says whether something that must never happen did (safety), or something that
//! must happen did not (liveness).

use dassie_connector::{CapUsage, replay};
use dassie_node_core::testing::Outcome;
use dassie_settlement::policy;

use super::{Payment, Settling, Sim, TRIGGER, USD, XRP, key, received, sent, xrpl_account};
use crate::store::OutgoingState;

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
    let ledger = sim.settling.ledger();
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
    // Less is a payment still under way, which liveness judges.
    if total > payment.amount || (payment.done && total != payment.amount) {
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
/// for, and is complete once it has that. Less than was sent, or more than
/// it asked for, is a safety failure; not yet all it asked for, liveness.
pub fn check_incoming(sim: &Sim, payment: &Payment) -> Result<(), Outcome> {
    let row = sim.nodes[payment.to]
        .store
        .incoming_payment(payment.incoming)
        .unwrap()
        .ok_or_else(|| Outcome::Safety("an incoming payment vanished".into()))?;
    let sent = sent(payment);
    if row.received_amount < sent {
        return Err(Outcome::Safety(format!(
            "node {} received {} of the {sent} sent",
            payment.to, row.received_amount
        )));
    }
    if let Some(limit) = payment.limit
        && (row.received_amount != limit || row.completed_at_ms.is_none())
    {
        let problem = format!(
            "an incoming payment of {limit} received {} (completed {:?})",
            row.received_amount, row.completed_at_ms
        );
        return Err(if row.received_amount > limit {
            Outcome::Safety(problem)
        } else {
            Outcome::Liveness(problem)
        });
    }
    Ok(())
}

/// Each owner's pool moved by exactly what its incoming payments
/// received, and each live client's token by what it sent: each node's
/// books against what its own clients and receivers really moved.
pub fn check_owner_books(sim: &Sim) -> Result<(), String> {
    let all = || sim.payments.iter().chain(&sim.abandoned);
    for index in 0..sim.nodes.len() {
        let balances = sim.balances(index);
        let pool = balances["equity/owner"];
        let received: i128 = all()
            .filter(|payment| payment.to == index)
            .map(|payment| i128::from(received(sim, payment)))
            .sum::<i128>()
            + outgoing_flows(sim)
                .filter(|flow| flow.to == index)
                .map(|flow| i128::from(flow.received))
                .sum::<i128>();
        let deposit = Settling::deposit(sim.settling.ledger());
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
/// its ask point: asked, the peer settled (D23, spec/areas/settlement.md#asking-a-peer-to-settle).
pub fn check_settled(sim: &Sim) -> Result<(), String> {
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            let Some(owed) = peer.owed.filter(|_| peer.termination.is_none()) else {
                continue;
            };
            let credit = peer.peering.credit_out;
            let position = u128::try_from(owed + i128::from(credit)).unwrap();
            if policy::ask(position, TRIGGER) {
                return Err(format!(
                    "node {index}'s room for {} is {position} of {credit}: it never settled",
                    peer.id
                ));
            }
        }
    }
    Ok(())
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

/// Everything a run over one ledger checks once the network is calm: what
/// must never happen first, so that a run that broke both says safety.
pub fn check(sim: &Sim) -> Result<(), Outcome> {
    check_events(sim).map_err(Outcome::Safety)?;
    for payment in &sim.payments {
        check_cap(sim, payment).map_err(Outcome::Safety)?;
    }
    check_books(sim)?;
    check_finished(sim).map_err(Outcome::Liveness)?;
    for payment in &sim.payments {
        check_incoming(sim, payment)?;
    }
    check_outgoing(sim)
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
/// got what it asked for. A receiver that got less than was sent is a
/// safety failure; a payment that did not complete, liveness.
pub fn check_outgoing(sim: &Sim) -> Result<(), Outcome> {
    for &(from, to, incoming, outgoing) in &sim.outgoing {
        let row = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .ok_or_else(|| Outcome::Safety("an outgoing payment vanished".into()))?;
        let received = sim.nodes[to]
            .store
            .incoming_payment(incoming)
            .unwrap()
            .map_or(0, |row| row.received_amount);
        if received < row.sent_amount {
            return Err(Outcome::Safety(format!(
                "node {to} received {received} of the {} node {from} sent",
                row.sent_amount
            )));
        }
        if row.state != OutgoingState::Completed {
            let incoming_row = sim.nodes[to].store.incoming_payment(incoming).unwrap();
            return Err(Outcome::Liveness(format!(
                "an outgoing payment from {from} to {to} ended {:?} ({:?}) having sent {} of {}; its receiver: {incoming_row:?}",
                row.state, row.failure, row.sent_amount, row.debit_amount
            )));
        }
        if received != row.receive_amount.value {
            return Err(Outcome::Liveness(format!(
                "node {to} received {received} of the {} node {from} sent, for {}",
                row.sent_amount, row.receive_amount.value
            )));
        }
    }
    Ok(())
}

/// Each node's books, on their own: they hold what its scheme holds, and
/// it settled what it owed. The underlying ledgers neither made nor lost
/// money. No check compares two nodes' books, or adds them up: each keeps
/// its own, and they may drift (D19).
pub fn check_books(sim: &Sim) -> Result<(), Outcome> {
    check_owner_books(sim).map_err(Outcome::Safety)?;
    check_ledgers(sim).map_err(Outcome::Safety)?;
    check_settled(sim).map_err(Outcome::Liveness)?;
    check_delivered(sim).map_err(Outcome::Liveness)
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

/// Each node books in settlement what its scheme holds, on the XRP Ledger
/// exactly its account's balance less the reserve, and the underlying
/// ledger neither made nor lost money.
pub fn check_ledgers(sim: &Sim) -> Result<(), String> {
    let count = sim.nodes.len();
    let mut stubs = 0i128;
    for index in 0..count {
        let balances = sim.balances(index);
        // The books follow the scheme's balance: what settled was posted.
        let view = &sim.core(index).ledger_views()[0];
        let booked = -balances["assets/settlement"];
        if booked != i128::from(view.on_ledger_balance) {
            return Err(format!(
                "node {index} books {booked} in settlement, its scheme holds {}",
                view.on_ledger_balance
            ));
        }
        // On the XRP Ledger, that is the account's balance less its
        // reserve, exactly.
        if sim.settling == Settling::Xrpl {
            let drops = sim.ledger.balance(xrpl_account(index)).unwrap_or(0);
            let bookable = i128::from(drops) * 1_000 - i128::from(view.reserve);
            if booked != bookable.max(0) {
                return Err(format!(
                    "node {index} books {booked}; its account holds {drops} drops"
                ));
            }
        }
        stubs += booked;
    }
    match sim.settling {
        Settling::Stub => {
            let total = Settling::deposit(USD) * i128::try_from(count).unwrap();
            if stubs != total {
                return Err(format!("the stubs hold {stubs}, not {total}"));
            }
        }
        Settling::Xrpl => {
            if sim.ledger.total() != dassie_xrpl::MAX_DROPS {
                return Err("the ledger made or lost XRP".into());
            }
        }
        Settling::Cross => return Err("a run across currencies is checked apart".into()),
    }
    Ok(())
}

/// A run across currencies: every payment finished, each node settled
/// what it owed and took every settlement, and no ledger made or lost an
/// asset. Each node is judged by its own books (D19).
pub fn check_across(sim: &Sim) -> Result<(), Outcome> {
    check_events(sim).map_err(Outcome::Safety)?;
    check_received_across(sim).map_err(Outcome::Safety)?;
    check_assets(sim).map_err(Outcome::Safety)?;
    check_finished(sim).map_err(Outcome::Liveness)?;
    for &(from, to, _, outgoing) in &sim.outgoing {
        let row = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .ok_or_else(|| Outcome::Safety("an outgoing payment vanished".into()))?;
        if row.state != OutgoingState::Completed || row.sent_amount != row.debit_amount {
            return Err(Outcome::Liveness(format!(
                "an outgoing payment from {from} to {to} ended {:?} ({:?}) having sent {} of {}",
                row.state, row.failure, row.sent_amount, row.debit_amount
            )));
        }
    }
    check_settled(sim).map_err(Outcome::Liveness)
}

/// What arrived is what was sent at the rates, less what the hops charged;
/// more only if a hop lost a Fulfill to a crash.
fn check_received_across(sim: &Sim) -> Result<(), String> {
    for payment in &sim.payments {
        let into = *sim.settling.ledgers(payment.to).last().unwrap();
        let (got, sent) = (
            u128::from(received(sim, payment)),
            u128::from(sent(payment)),
        );
        let least = match (payment.ledger, into) {
            (USD, XRP) => sent * 19 / 10,
            (XRP, USD) => sent * 45 / 100,
            _ => sent * 95 / 100,
        };
        if got < least {
            return Err(format!(
                "node {} received {got} of node {}'s {sent}, {:?} to {into:?}",
                payment.to, payment.from, payment.ledger
            ));
        }
    }
    Ok(())
}

/// No ledger made or lost an asset, and node 1 charged for what passed
/// between its peers.
fn check_assets(sim: &Sim) -> Result<(), String> {
    let count = sim.nodes.len();
    for ledger in [USD, XRP] {
        let (mut schemes, mut nodes) = (0i128, 0i128);
        for index in (0..count).filter(|i| sim.settling.ledgers(*i).contains(&ledger)) {
            nodes += 1;
            schemes -= sim.balances_on(index, ledger)["assets/settlement"];
        }
        let made = match ledger {
            USD => schemes != Settling::deposit(USD) * nodes,
            _ => sim.ledger.total() != dassie_xrpl::MAX_DROPS,
        };
        if made {
            return Err(format!(
                "{ledger} was made or lost: the schemes hold {schemes}"
            ));
        }
    }
    let fees: i128 = [USD, XRP]
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
