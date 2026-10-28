//! A payer over a hub sizes its fundings by how often it is asked
//! (spec/decisions/settle-by-request-rate.md,
//! spec/areas/settlement.md#settling-on-request): asked again soon after
//! each funding landed, it doubles the credit it counts, up to its bound;
//! not asked, it halves it; the hub's floor follows; its hold toward a peer
//! stays below the credit it counts, whatever the peer asks; and nothing is
//! settled twice. On one core over the stub with commitments, a downlink
//! asking it to settle, whose fundings land a hub's landing time after the
//! request, held back as a hub would hold them, and whose messages the
//! downlink acknowledges. The downlink takes nothing from the hold, so each
//! funding raises it.

use std::sync::Arc;

use dassie_identity::NodeId;
use dassie_ledger::Name;
use dassie_peer::Response;
use dassie_peer::wire::Kind;
use dassie_settlement::policy;

use super::super::Core;
use super::{
    NOW, core_on, hub_books, hub_core_with_downlink, hub_core_with_downlink_on, scheme_id,
};
use crate::engine::Output;
use crate::node::Policies;
use crate::store::Store;

/// How long a funding takes from the request to landing, as over the
/// testnet's hubs.
const LANDING_MS: u64 = 800;

/// The request interval, the longest a pace waits before it halves.
const ASK_EVERY_MS: u64 = Policies::DEFAULT.settlement_request_every_ms;

/// The seeds each test runs on.
const SEEDS: [u64; 3] = [121, 122, 123];

/// What one request has a payer fund toward a peer that took nothing of
/// its hold, `held`, counting `credit`: as if it extended that credit,
/// the hold paid in advance.
fn step_at(core: &Core, credit: u64, held: u64) -> u64 {
    policy::requested_amount(
        u128::from(credit.saturating_sub(held)),
        core.settle.trigger,
        core.settle.minimum,
    )
}

/// A payer over the stub with commitments and the downlink that asks it.
struct Payer {
    core: Core,
    peer: NodeId,
    /// The settlements prepared and not yet landed, held back as a hub
    /// holds them until it applied them.
    held: Vec<(Name, Name)>,
}

impl Payer {
    fn new(seed: u64) -> Self {
        Self::on(Arc::new(Store::open_in_memory().unwrap()), seed)
    }

    fn on(store: Arc<Store>, seed: u64) -> Self {
        let (mut core, peer) = hub_core_with_downlink_on(store, seed);
        core.begin_step(NOW + 1_500);
        core.stub_deposit(policy::hub_ceiling(core.settle.max_packet) * 10)
            .unwrap();
        core.finish_step(&mut Vec::new());
        Self {
            core,
            peer,
            held: Vec::new(),
        }
    }

    /// The peer asks us to settle at `at`: returns what is then on its way
    /// to it. What we prepare is held back until it lands.
    fn asked(&mut self, at: u64) -> u64 {
        self.core.begin_step(at);
        let on_its_way = self.core.settlement_requested(self.peer, scheme_id("stub"));
        self.held.append(&mut self.core.settle.to_execute);
        self.core.finish_step(&mut Vec::new());
        on_its_way
    }

    /// What we held back lands at `at`, and the peer acknowledges its
    /// notice.
    fn lands(&mut self, at: u64) {
        assert!(!self.held.is_empty(), "nothing to land");
        let mut outputs = Vec::new();
        self.core.begin_step(at);
        self.core.settle.to_execute.append(&mut self.held);
        self.core.finish_step(&mut outputs);
        let notice = outputs.into_iter().find_map(|output| match output {
            Output::PeerRequest(request)
                if request.to == self.peer && request.kind == Kind::Settlement =>
            {
                Some(request.id)
            }
            _ => None,
        });
        let id = notice.expect("a notice to acknowledge");
        self.core.begin_step(at);
        self.core.peer_response(
            id,
            Ok(Response {
                status: 200,
                body: Vec::new(),
            }),
        );
        self.core.finish_step(&mut Vec::new());
        assert!(self.core.peer.channel(&self.peer).unwrap().outbox.is_none());
    }

    /// One cycle: asked at `at`, landed `LANDING_MS` later if it funded
    /// anything. Returns what it funded, and when it landed.
    fn cycle(&mut self, at: u64) -> (u64, u64) {
        let funded = self.asked(at);
        if funded > 0 {
            self.lands(at + LANDING_MS);
        }
        (funded, at + LANDING_MS)
    }

    /// The credit we count for the peer now.
    fn counted(&self) -> u64 {
        self.core.counted_credit(self.peer, scheme_id("stub"))
    }

    /// Our hold toward the peer: what our books extend it, what the stub
    /// holds, and what is pending to it.
    fn books(&mut self) -> (u64, u64, u64) {
        hub_books(&mut self.core, self.peer)
    }

    /// The floor of the stub's pretend hub, for the peerings over it.
    fn floor(&self) -> u64 {
        let stub = dassie_ledger::name!("stub");
        self.core.hub_limits(stub, &self.core.hub_peerings(stub)).0
    }
}

// A payee that asks again within four landing times of each funding
// landing has its payer double the credit it counts each time, up to a
// quarter of the ceiling: 11 packets, 22, then 25 rather than 44. Each
// step is what the rule asks at that credit, the hold paid in advance, so
// a payee that spends nothing, asking all the same, gets a hold below the
// credit counted less the trigger, and nothing more once there. Each
// funding is settled once.
#[test]
fn steps_grow_under_sustained_asking_up_to_their_bound() {
    for seed in SEEDS {
        let mut payer = Payer::new(seed);
        let credit = Policies::DEFAULT.peer_credit;
        let most = policy::most_counted_credit(payer.core.settle.max_packet);
        let trigger = payer.core.settle.trigger;
        let soon = policy::GROW_WITHIN_LANDINGS * LANDING_MS;
        let mut at = NOW + 2_000;
        let (mut counted, mut sum) = (Vec::new(), 0);
        for round in 0..12 {
            let (held, _, _) = payer.books();
            let expected = match round {
                0 => credit,
                1 => 2 * credit,
                _ => most,
            };
            let (amount, landed) = payer.cycle(at);
            counted.push(payer.counted());
            assert_eq!(amount, step_at(&payer.core, expected, held), "seed {seed}");
            sum += amount;
            assert_eq!(payer.books(), (sum, sum, 0), "seed {seed}");
            assert!(sum <= expected - trigger, "seed {seed}");
            at = landed + soon;
            payer.core.check_invariants();
        }
        assert_eq!(counted[..3], [credit, 2 * credit, most], "seed {seed}");
        assert_eq!(most, 25 * payer.core.settle.max_packet);
        assert!(counted.iter().all(|c| *c <= most));
        // Past the bound, a payee spending nothing gets nothing more.
        assert_eq!(payer.asked(at), 0, "seed {seed}");
        assert_eq!(payer.books(), (sum, sum, 0));
    }
}

// Asked again later than four landing times after each funding landed,
// a payer keeps counting the default credit: a payee that spends a step
// slowly gets the usual step.
#[test]
fn steps_stay_while_asked_slowly() {
    for seed in SEEDS {
        let mut payer = Payer::new(seed);
        let credit = Policies::DEFAULT.peer_credit;
        let late = policy::GROW_WITHIN_LANDINGS * LANDING_MS + 1;
        let mut at = NOW + 2_000;
        for _ in 0..4 {
            let (held, _, _) = payer.books();
            let (amount, landed) = payer.cycle(at);
            assert_eq!(amount, step_at(&payer.core, credit, held), "seed {seed}");
            assert_eq!(payer.counted(), credit);
            at = landed + late;
        }
    }
}

/// A payer whose pace grew to its bound, and when its last funding
/// landed.
fn grown(seed: u64) -> (Payer, u64) {
    let mut payer = Payer::new(seed);
    let soon = policy::GROW_WITHIN_LANDINGS * LANDING_MS;
    let mut at = NOW + 2_000;
    let mut landed = at;
    for _ in 0..3 {
        (_, landed) = payer.cycle(at);
        at = landed + soon;
    }
    let most = policy::most_counted_credit(payer.core.settle.max_packet);
    assert_eq!(payer.counted(), most);
    (payer, landed)
}

// Not asked, the payer halves the credit it counts for each sixteen
// landing times: from 44 packets, capped at 25, to 22 after one, and the
// default after two. The next request after a pause gets that, and a pace
// back at the default is forgotten at the next check.
#[test]
fn steps_shrink_when_idle() {
    for seed in SEEDS {
        let (mut payer, landed) = grown(seed);
        let credit = Policies::DEFAULT.peer_credit;
        let most = policy::most_counted_credit(payer.core.settle.max_packet);
        let span = policy::IDLE_LANDINGS * LANDING_MS;
        payer.core.begin_step(landed + span - 1);
        assert_eq!(payer.counted(), most, "seed {seed}");
        payer.core.finish_step(&mut Vec::new());
        payer.core.begin_step(landed + span);
        assert_eq!(payer.counted(), 2 * credit, "seed {seed}");
        payer.core.finish_step(&mut Vec::new());
        // Asked after that span, it funds by 22 packets, and grows no
        // more: the request came long after the landing.
        let (held, _, _) = payer.books();
        let (amount, landed) = payer.cycle(landed + span);
        assert_eq!(
            amount,
            step_at(&payer.core, 2 * credit, held),
            "seed {seed}"
        );
        assert_eq!(payer.counted(), 2 * credit);
        // Never asked again: back to the default after another span, and
        // forgotten at the next check.
        payer.core.begin_step(landed + span);
        assert_eq!(payer.counted(), credit, "seed {seed}");
        payer.core.finish_step(&mut Vec::new());
        let quiet = landed + ASK_EVERY_MS;
        payer.core.begin_step(quiet);
        payer.core.settle.at_ms = quiet;
        payer.core.finish_step(&mut Vec::new());
        assert!(payer.core.settle.pace.is_empty(), "seed {seed}");
        payer.core.check_invariants();
    }
}

// The floor of the hub follows the step: per peering, what one request
// has the payer fund with nothing held at the credit it counts for that
// peer, so a hub that funds a peer often keeps that much ready, and lets
// it go once the peer is idle.
#[test]
fn the_floor_follows_the_step() {
    for seed in SEEDS {
        let payer = Payer::new(seed);
        let p = payer.core.settle.max_packet;
        let default = policy::hub_floor(p, Policies::DEFAULT.peer_credit, 1);
        assert_eq!(payer.floor(), default);
        let (mut payer, landed) = grown(seed);
        let most = policy::most_counted_credit(p);
        assert_eq!(payer.floor(), step_at(&payer.core, most, 0), "seed {seed}");
        assert_eq!(payer.floor(), 1_175 * p / 100);
        payer.core.begin_step(landed + ASK_EVERY_MS);
        assert_eq!(payer.floor(), default, "seed {seed}");
        payer.core.finish_step(&mut Vec::new());
    }
}

// However often the peer asks while a funding is on its way, the payer
// answers with that one and prepares no other, and such a request doubles
// nothing: each landing counts once. Nothing is settled twice.
#[test]
fn nothing_settles_twice_under_sustained_asking() {
    for seed in SEEDS {
        let mut payer = Payer::new(seed);
        let soon = policy::GROW_WITHIN_LANDINGS * LANDING_MS;
        let mut at = NOW + 2_000;
        let mut total = 0;
        for _ in 0..4 {
            let amount = payer.asked(at);
            assert!(amount > 0);
            total += amount;
            for again in 1..10 {
                assert_eq!(payer.asked(at + again * 10), amount, "seed {seed}");
                assert_eq!(payer.core.ledger.pending_transfers().count(), 1);
            }
            payer.lands(at + LANDING_MS);
            assert_eq!(payer.books(), (total, total, 0), "seed {seed}");
            at += LANDING_MS + soon;
        }
        // One doubling per landing after the first: three, capped at two.
        let pace = payer.core.settle.pace[&(payer.peer, scheme_id("stub"))];
        assert_eq!(pace.doublings(at, ASK_EVERY_MS), policy::MOST_DOUBLINGS);
        payer.core.check_invariants();
    }
}

// The pace is in memory: a restart counts the default credit again, as the
// asker's requests and the requests kept for late answers are forgotten.
#[test]
fn a_restart_forgets_the_pace() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut payer = Payer::on(Arc::clone(&store), 124);
    let soon = policy::GROW_WITHIN_LANDINGS * LANDING_MS;
    let (_, landed) = payer.cycle(NOW + 2_000);
    payer.cycle(landed + soon);
    assert_eq!(payer.counted(), 2 * Policies::DEFAULT.peer_credit);
    let peer = payer.peer;
    payer.core.flush();
    drop(payer);
    let core = core_on(store, 125);
    assert!(core.settle.pace.is_empty());
    assert_eq!(
        core.counted_credit(peer, scheme_id("stub")),
        Policies::DEFAULT.peer_credit
    );
}

// A peering over a hub whose methods carry no pace yet, as a downlink that
// never asked: the payer counts the default credit, and keeps no pace.
#[test]
fn no_pace_until_asked() {
    let (core, peer) = hub_core_with_downlink(126);
    assert!(core.settle.pace.is_empty());
    assert_eq!(
        core.counted_credit(peer, scheme_id("stub")),
        Policies::DEFAULT.peer_credit
    );
}
