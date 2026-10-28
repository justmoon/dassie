//! Fewer peer slots when a hub's pool is too short
//! (spec/decisions/slots-by-liquidity.md): on one engine core in the XRP
//! hub, its own moves off so that nothing refills its account, with
//! downlinks peered over it.

use std::sync::Arc;

use dassie_hub::member::SCHEME_ID;
use dassie_identity::NodeId;
use dassie_peer::wire::TerminationReason;
use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::fake::FakeLedger;

use dassie_oer::Decode;
use dassie_peer::wire::{Content, Kind};

use super::super::liquidity::grace_ms;
use super::super::{Core, Output};
use super::{
    Policies, SHARED_CONFIG, Validators, add_xrpl, deposits, downlink_request_on, follow_xrpl,
    funded_ledger, hub_config, wait_and_close,
};

/// The largest packet, in drops.
const P: u64 = Policies::DEFAULT.connector.max_packet_amount / UNITS_PER_DROP;

/// The core in the XRP hub, account 5, holding `balance` drops as last
/// agreed, its own moves off, and what its opening deposit credited.
fn hub_core(ledger: &mut FakeLedger, balance: u64) -> (Core, Validators, u64) {
    let mut core = super::core(9);
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(SCHEME_ID, hub_config(600, 300)).unwrap();
    follow_xrpl(&mut core, ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    wait_and_close(&mut core, ledger, 1_000);
    let opening = deposits(ledger, hub.escrow())[0].amount;
    core.set_hub_automatic(SCHEME_ID, false).unwrap();
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, ledger, 1_000);
    (core, hub, opening)
}

/// Downlink `n` asks the core to peer over the hub, as account `20 + n`,
/// once a check judged the pool: its ID, and whether it is peered.
fn ask(core: &mut Core, hub: &Validators, n: u8) -> (NodeId, bool) {
    let downlink = Arc::new(dassie_identity::TableKeygen::identity_key(&[20 + n; 32]));
    let key = dassie_hub::member::member_key(
        &dassie_identity::TableKeygen::identity_key(&[40 + n; 32]),
        &hub.rules.id(),
    )
    .public_key();
    let mut membership = hub.rules.id().0.as_bytes().to_vec();
    membership.extend_from_slice(&(20 + u32::from(n)).to_be_bytes());
    membership.extend_from_slice(key.as_bytes());
    let at = core.now_ms() + Policies::DEFAULT.settlement_interval_ms;
    core.begin_step(at);
    core.finish_step(&mut Vec::new());
    let body = downlink_request_on(&downlink, at, SCHEME_ID, &membership);
    core.peer_inbound(dassie_peer::InboundId(at), downlink.identity(), &body, None);
    core.finish_step(&mut Vec::new());
    let id = downlink.node_id();
    (id, core.peer.active_peers().any(|(peer, _)| *peer == id))
}

/// Moves the core on to `to`, a check at a time, the hub saying the
/// account holds `balance` at each, as its validators do every few
/// seconds: what the core handed over meanwhile.
fn run_to(
    core: &mut Core,
    hub: &mut Validators,
    to: u64,
    balance: u64,
    opening: u64,
) -> Vec<Output> {
    let every = Policies::DEFAULT.settlement_interval_ms;
    let mut outputs = Vec::new();
    while core.now_ms() < to {
        let now = (core.now_ms() + every).min(to);
        core.begin_step(now);
        hub.account(core, balance, opening);
        core.finish_step(&mut outputs);
    }
    outputs
}

/// The peer requests in `outputs` to `peer`, in order: their kind and
/// content.
fn requests_to(outputs: &[Output], peer: &NodeId) -> Vec<(Kind, Vec<u8>)> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::PeerRequest(request) if request.to == *peer => {
                Some((request.kind, request.body.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The core, ending its peering with `peer`, collects first: the peer is
/// still peered, and asked to settle once before it is told, with no
/// reason. Runs the core until it is told.
fn collected_then_told(core: &mut Core, hub: &mut Validators, peer: &NodeId, opening: u64) {
    assert!(core.peer.active_peers().any(|(id, _)| id == peer));
    let wait = Policies::DEFAULT.peer.depeer_wait_ms;
    let told_by = core.now_ms() + 2 * wait + 8_000;
    let outputs = run_to(core, hub, told_by, P, opening);
    let kinds: Vec<Kind> = (requests_to(&outputs, peer).into_iter())
        .filter(|(kind, _)| matches!(kind, Kind::SettlementRequest | Kind::PeeringTermination))
        .map(|(kind, _)| kind)
        .collect();
    assert_eq!(
        kinds.get(..2),
        Some(&[Kind::SettlementRequest, Kind::PeeringTermination][..])
    );
    let told = requests_to(&outputs, peer)
        .into_iter()
        .find(|(kind, _)| *kind == Kind::PeeringTermination)
        .unwrap();
    assert_eq!(
        Content::decode_exact(&told.1).unwrap(),
        Content::PeeringTermination(TerminationReason::Unspecified)
    );
}

/// Why the core ends its peering with `peer`, while it collects first.
fn ending(core: &Core, peer: &NodeId) -> Option<TerminationReason> {
    let peering = core.peer.peering_of(peer)?;
    peering.depeering.map(|depeering| depeering.reason)
}

// A pool that covers each downlink, and one more, takes it; once it no
// longer would, the next is refused for capacity at once, and nothing is
// ended. Short of its peerings past the grace, the node ends as many as
// bring the pool back to a packet each, those that never paid first, then
// the newest, collecting first, for its liquidity; the rest stay. Funded
// again with the margin, it takes a new downlink.
#[test]
fn a_short_pool_refuses_then_ends_the_newest_free_downlinks() {
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub, opening) = hub_core(&mut ledger, 3 * P);

    let (first, peered) = ask(&mut core, &hub, 1);
    assert!(peered);
    let (paying, peered) = ask(&mut core, &hub, 2);
    assert!(peered);
    let (newest, peered) = ask(&mut core, &hub, 3);
    assert!(peered);
    // The second paid us a largest packet: it is no free receiver.
    core.with_peer(|peer, hooks, now| peer.paid_by(now, hooks, &paying));
    let (_, peered) = ask(&mut core, &hub, 4);
    assert!(!peered, "three packets cover three peerings, not a fourth");
    let liquidity = core.hub_status().unwrap().liquidity.unwrap();
    assert!(liquidity.refusing_downlinks && !liquidity.refusing_core);
    assert_eq!((liquidity.peerings, liquidity.covered), (3, 3));
    assert_eq!(liquidity.short_since_ms, None, "not short: nothing ends");

    // The pool falls to a packet: short of two.
    let short_at = core.now_ms() + Policies::DEFAULT.settlement_interval_ms;
    run_to(&mut core, &mut hub, short_at, P, opening);
    let grace = grace_ms(Policies::DEFAULT.settlement_interval_ms);
    let liquidity = core.hub_status().unwrap().liquidity.unwrap();
    let since = liquidity.short_since_ms.unwrap();
    assert_eq!(liquidity.ends_from_ms, Some(since + grace));
    run_to(&mut core, &mut hub, since + grace - 1, P, opening);
    for peer in [&first, &paying, &newest] {
        assert_eq!(ending(&core, peer), None, "nothing ends within the grace");
    }
    run_to(&mut core, &mut hub, since + grace, P, opening);
    assert_eq!(
        ending(&core, &newest),
        Some(TerminationReason::ShortOfLiquidity)
    );
    assert_eq!(
        ending(&core, &first),
        Some(TerminationReason::ShortOfLiquidity),
        "free before paying, however old"
    );
    assert_eq!(ending(&core, &paying), None);
    collected_then_told(&mut core, &mut hub, &newest, opening);
    // Those being ended count no more at the next check.
    let next = core.now_ms() + 4_000;
    run_to(&mut core, &mut hub, next, P, opening);
    let liquidity = core.hub_status().unwrap().liquidity.unwrap();
    assert_eq!((liquidity.peerings, liquidity.covered), (1, 1));

    // Funded again, but within the margin: still no newcomer.
    let funded = 2 * P;
    let later = core.now_ms() + 8_000;
    run_to(&mut core, &mut hub, later, funded, opening);
    let (_, peered) = ask(&mut core, &hub, 5);
    assert!(!peered, "within the margin");
    // Past it, the caps come back.
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let target = dassie_settlement::policy::hub_target(p);
    let margin =
        dassie_settlement::policy::hub_spendable(p, Policies::DEFAULT.peer_credit, 1, target)
            / UNITS_PER_DROP;
    let later = core.now_ms() + 8_000;
    run_to(&mut core, &mut hub, later, funded + margin, opening);
    let liquidity = core.hub_status().unwrap().liquidity.unwrap();
    assert!(!liquidity.refusing_downlinks && liquidity.short_since_ms.is_none());
    let (_, peered) = ask(&mut core, &hub, 6);
    assert!(peered, "taken again");
}

// spec/decisions/slots-by-liquidity.md: a node whose own account can pay
// never refuses or ends a peer for want of pool. At an owner-set target of
// 25 largest packets, 20 of them the account's reserve, its pool at the
// target covers 5 peerings; it deposits past its ceiling a packet for each
// peering and one more instead, and takes every downlink.
#[test]
fn funds_on_its_own_account_are_deposited_not_refused() {
    let (mut ledger, _) = funded_ledger();
    let mut core = super::core(9);
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    core.add_scheme(SCHEME_ID, hub_config(600, 300)).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let escrow = hub.escrow();
    let mut deposited = deposits(&mut ledger, escrow)[0].amount;
    let target = 25 * Policies::DEFAULT.connector.max_packet_amount;
    core.set_hub_target(SCHEME_ID, Some(target)).unwrap();
    let reserve = hub.rules.terms.fees.account_reserve;
    assert_eq!(reserve, 20 * P);
    let mut balance = 25 * P - reserve;
    hub.account(&mut core, balance, deposited);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let mut seen = deposits(&mut ledger, escrow).len();

    for n in 1..=8 {
        let (_, peered) = ask(&mut core, &hub, n);
        assert!(peered, "downlink {n} is taken");
        wait_and_close(&mut core, &mut ledger, 4_000);
        // The hub credits what the node deposited meanwhile.
        let made = deposits(&mut ledger, escrow);
        for deposit in made.get(seen..).unwrap_or_default() {
            balance += deposit.amount;
            deposited += deposit.amount;
        }
        seen = made.len();
        hub.account(&mut core, balance, deposited);
        wait_and_close(&mut core, &mut ledger, 1_000);
    }
    let liquidity = core.hub_status().unwrap().liquidity.unwrap();
    assert_eq!(liquidity.peerings, 8);
    assert!(liquidity.covered >= 9, "a packet each and one more");
    assert!(!liquidity.refusing_downlinks && !liquidity.refusing_core);
    assert_eq!(liquidity.short_since_ms, None);
    assert!(seen > 1, "it deposited");
    assert!(balance + reserve > 27 * P, "past the ceiling");
}
