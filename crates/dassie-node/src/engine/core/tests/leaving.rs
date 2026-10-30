//! Leaving a settlement hub that ended, so that its scheme can be added
//! again with another hub's rules, and the books a move by hand left
//! behind (spec/areas/hubs.md#leaving-a-hub-that-ended,
//! spec/decisions/leaving-a-hub-that-ended.md): no deposit goes into a hub
//! that stopped, not even once its payout set returned the account's funds
//! to the owner book; a hub whose payout set a quorum signed is left, its
//! move on its way and all, its state let go and what the books held of
//! it written off, once; and books that hold what the account's checkpoint
//! does not explain are reconciled to it as the scheme starts, but never on
//! an ordinary join or restart.

use std::sync::Arc;

use dassie_hub::member::SCHEME_ID;
use dassie_ledger::{LedgerId, Name};
use dassie_node_core::config::{LiveRealm, Realm};
use dassie_xrpl::UNITS_PER_DROP;

use super::super::{Core, EngineError, Event, Output, SettlementEvent};
use super::{
    NOW, Store, Validators, core_taking_downlinks, deposits, follow_xrpl, funded_ledger,
    holding_core, hub_config, hub_rules, wait_and_close,
};
use crate::store::{Checkpoint, LedgerBatch};

/// What the stall of the tests' hub takes to be judged, and a moment.
const STALL_MS: u64 = 3_600_000 + 1_000;

/// The scheme over the tests' XRP hub.
fn hub_scheme() -> Name {
    Name::new(SCHEME_ID).unwrap()
}

/// What the owner holds on `ledger`, if its scheme runs.
fn owner_on(core: &Core, ledger: LedgerId) -> Option<i128> {
    core.ledger_views()
        .into_iter()
        .find(|view| view.id == ledger)
        .map(|view| view.owner_balance)
}

/// The settlement events among `outputs`.
fn settlement_events(outputs: &[Output]) -> Vec<SettlementEvent> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::Event(Event::Settlement(event)) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// The settlement events of a step taken now.
fn step_events(core: &mut Core) -> Vec<SettlementEvent> {
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    settlement_events(&outputs)
}

/// Whether `events` mend the books of a hub.
fn mended(events: &[SettlementEvent]) -> Option<(u64, u64)> {
    events.iter().find_map(|event| match event {
        SettlementEvent::HubBooksMended { held, kept, .. } => Some((*held, *kept)),
        _ => None,
    })
}

/// A node in the tests' XRP hub on `store`, its account opened and holding
/// a balance between the floor and the ceiling, then left below the floor
/// by a state heard after the hub went quiet: the node deposits nothing,
/// since the hub made no progress it heard. Its ledger, the hub's
/// validators, and the opening deposit.
fn quiet_low_hub(store: &Arc<Store>) -> (Core, dassie_xrpl::fake::FakeLedger, Validators, u64) {
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub) = holding_core(store, &mut ledger);
    let escrow = hub.escrow();
    wait_and_close(&mut core, &mut ledger, 1_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 1, "the opening deposit: {made:?}");
    let opening = made[0].amount;
    hub.account(&mut core, opening, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(deposits(&mut ledger, escrow).len(), 1, "near the target");

    // Quiet for longer than a hub that goes on ever is, then a state below
    // the floor, heard after the gap: no rise the account heard itself.
    wait_and_close(&mut core, &mut ledger, 6 * 60 * 1_000);
    hub.account(&mut core, 100_000, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        1,
        "no progress heard: no deposit"
    );
    (core, ledger, hub, opening)
}

// The incident of 2026-10-29: a hub stalled for good, its payout set was
// submitted, and member nodes deposited what it returned to their owner
// books into its escrow at once, through the floor's refill, never to be
// credited. A hub the account judges stalled takes no deposit, however low
// the account and however much the owner book holds: the payout is the
// owner's on the book, booked as it arrives.
#[test]
fn a_payout_landing_on_the_owner_book_goes_into_no_stalled_hub() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, mut ledger, mut hub, opening) = quiet_low_hub(&store);
    let escrow = hub.escrow();

    // The validators name the same block for a whole stall.
    wait_and_close(&mut core, &mut ledger, STALL_MS);
    hub.again(&mut core, 100_000, opening);
    let status = core.hub_status().unwrap();
    assert!(status.stopped, "judged stalled");
    assert!(!status.ended, "no payout set kept");
    assert_eq!(core.hub_refill(LedgerId::HubXrplTestnetXrp), 0);

    let book = owner_on(&core, LedgerId::XrplTestnetXrp).unwrap();
    let payout = 40_000_000;
    let own = super::super::schemes::own_xrpl_account(&core.key, &dassie_xrpl::network::TESTNET);
    ledger.deposit(own, payout);
    for _ in 0..3 {
        wait_and_close(&mut core, &mut ledger, 4_000);
    }
    assert_eq!(
        owner_on(&core, LedgerId::XrplTestnetXrp).unwrap() - book,
        i128::from(payout * UNITS_PER_DROP),
        "the payout is the owner's deposit on the book"
    );
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        1,
        "nothing goes back in"
    );
    assert!(core.hub_status().unwrap().moving.is_none());
    let refused = core.hub_deposit(10_000_000 * UNITS_PER_DROP);
    assert!(
        matches!(&refused, Err(EngineError::Precondition(why)) if why.contains("stalled")),
        "{refused:?}"
    );
}

// The hour after a restart: the account judges a stall again only after
// one, and a hub it only started to hear may be one that stopped. Until
// the hub shows a rise the account heard itself, no deposit goes in, not
// even with the payout on the owner book; and once the node keeps a
// quorum's payout set of the hub, which survives restarts, the hub is
// ended at once, and stays stopped across a further restart.
#[test]
fn after_a_restart_nothing_goes_into_a_hub_that_stopped() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, mut ledger, mut hub, opening) = quiet_low_hub(&store);
    let escrow = hub.escrow();
    core.flush();
    let now = core.now_ms() + 1_000;
    let own = super::super::schemes::own_xrpl_account(&core.key, &dassie_xrpl::network::TESTNET);
    drop(core);

    let mut core = core_taking_downlinks(Arc::clone(&store), Realm::Test, LiveRealm::Refused, now);
    follow_xrpl(&mut core, &mut ledger);
    hub.again(&mut core, 100_000, opening);
    ledger.deposit(own, 40_000_000);
    for _ in 0..3 {
        wait_and_close(&mut core, &mut ledger, 4_000);
    }
    assert!(!core.hub_status().unwrap().stopped, "not judged yet");
    assert_eq!(deposits(&mut ledger, escrow).len(), 1, "no rise heard");

    core.hub_paid_out(hub.rules.id());
    let events = step_events(&mut core);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SettlementEvent::HubEnded { .. })),
        "{events:?}"
    );
    let status = core.hub_status().unwrap();
    assert!(status.stopped && status.ended);
    // Even a hub that seems to go on: its payout set is signed.
    hub.account(&mut core, 100_000, opening);
    hub.account(&mut core, 100_000, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(deposits(&mut ledger, escrow).len(), 1, "ended: no deposit");

    // The scheme keeps that its hub stopped; the node tells it the payout
    // set again, which it lets the owner leave by.
    core.flush();
    let now = core.now_ms() + 1_000;
    drop(core);
    let mut core = core_taking_downlinks(Arc::clone(&store), Realm::Test, LiveRealm::Refused, now);
    follow_xrpl(&mut core, &mut ledger);
    let status = core.hub_status().unwrap();
    assert!(status.stopped, "kept with the scheme");
    assert!(!status.ended, "until the payout keeper tells it again");
    assert!(matches!(
        core.leave_hub(SCHEME_ID),
        Err(EngineError::Precondition(_))
    ));
    core.hub_paid_out(hub.rules.id());
    assert_eq!(core.leave_hub(SCHEME_ID), Ok(true));
    assert!(core.hub_status().is_none());
}

// A hub is left only once it ended for good, a quorum of its validators
// having signed its payout set: one that runs, or that the account only
// judges stalled, is refused, saying why, and nothing changes.
#[test]
fn a_hub_that_has_not_ended_is_not_left() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, mut ledger, mut hub, opening) = quiet_low_hub(&store);
    for scheme in ["", "stub", dassie_hub::member::DSUSD_SCHEME_ID] {
        assert!(core.leave_hub(scheme).is_err(), "{scheme:?}");
    }
    let refused = core.leave_hub(SCHEME_ID);
    assert!(
        matches!(&refused, Err(EngineError::Precondition(why)) if why.contains("has not ended")),
        "{refused:?}"
    );
    wait_and_close(&mut core, &mut ledger, STALL_MS);
    hub.again(&mut core, 100_000, opening);
    assert!(core.hub_status().unwrap().stopped);
    assert!(matches!(
        core.leave_hub(SCHEME_ID),
        Err(EngineError::Precondition(_))
    ));
    assert!(
        core.ledger_views()
            .iter()
            .any(|view| view.id == LedgerId::HubXrplTestnetXrp)
    );
    assert!(
        core.store
            .checkpoints()
            .unwrap()
            .iter()
            .any(|(id, _)| *id == hub_scheme())
    );
}

/// A node in the tests' XRP hub on `store`, its account credited, whose
/// owner's deposit the XRP Ledger applied and the hub never credited, as
/// the hub stopped, and of which the node then keeps a quorum's payout
/// set: its core, its ledger, its own XRPL account, the escrow, and what
/// its books hold of the account.
struct Ended {
    core: Core,
    ledger: dassie_xrpl::fake::FakeLedger,
    own: dassie_xrpl::AccountId,
    escrow: dassie_xrpl::AccountId,
    held: u64,
}

fn ended_with_a_stuck_deposit(store: &Arc<Store>) -> Ended {
    let (mut ledger, own) = funded_ledger();
    let (mut core, mut hub) = holding_core(store, &mut ledger);
    let escrow = hub.escrow();
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, opening, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let held = core.settlement_held(LedgerId::HubXrplTestnetXrp);
    assert!(held > 0);
    core.hub_deposit(5_000_000 * UNITS_PER_DROP).unwrap();
    for _ in 0..2 {
        wait_and_close(&mut core, &mut ledger, 4_000);
    }
    assert_eq!(deposits(&mut ledger, escrow).len(), 2);
    assert!(core.hub_status().unwrap().moving.is_some());
    core.hub_paid_out(hub.rules.id());
    assert!(core.hub_status().unwrap().ended);
    step_events(&mut core);
    Ended {
        core,
        ledger,
        own,
        escrow,
        held,
    }
}

// spec/areas/hubs.md#leaving-a-hub-that-ended: the owner leaves a hub that
// ended. A deposit into it stuck on its way, which keeps the scheme from
// being removed, never keeps the node in it: it is forgotten. The books'
// holding of the account is written off through suspense, once, its state
// goes with it, and the payout set's arrival on the owner book is the
// owner's deposit there, so nothing counts twice.
#[test]
fn a_hub_that_ended_is_left_with_its_stuck_deposit() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let Ended {
        mut core,
        mut ledger,
        own,
        escrow,
        held,
    } = ended_with_a_stuck_deposit(&store);
    assert!(
        matches!(core.remove_scheme(SCHEME_ID), Err(EngineError::Precondition(why)) if why.contains("move")),
        "the move keeps the scheme from being removed"
    );
    let book = owner_on(&core, LedgerId::XrplTestnetXrp).unwrap();

    assert_eq!(core.leave_hub(SCHEME_ID), Ok(true), "nothing peers on it");
    let events = step_events(&mut core);
    let written_off: Vec<u64> = events
        .iter()
        .filter_map(|event| match event {
            SettlementEvent::HubLeft { written_off, .. } => Some(*written_off),
            _ => None,
        })
        .collect();
    assert_eq!(written_off, [held], "{events:?}");
    assert_eq!(core.settlement_held(LedgerId::HubXrplTestnetXrp), 0);
    assert_eq!(owner_on(&core, LedgerId::HubXrplTestnetXrp), None);
    assert!(core.hub_status().is_none());
    assert!(core.leave_hub(SCHEME_ID).is_err(), "no hub over it now");
    core.flush();
    let scheme = hub_scheme();
    assert!(
        !core
            .store
            .checkpoints()
            .unwrap()
            .iter()
            .any(|(id, _)| *id == scheme)
    );
    assert!(
        !core
            .store
            .asset_checkpoints()
            .unwrap()
            .iter()
            .any(|(id, _)| id == SCHEME_ID)
    );
    assert!(!core.store.hub_fundings().unwrap().contains_key(&scheme));

    // The payout set pays the account out to the owner's own account: the
    // owner's deposit on the book, which the books held of the hub no more.
    let payout = 30_000_000;
    ledger.deposit(own, payout);
    for _ in 0..3 {
        wait_and_close(&mut core, &mut ledger, 4_000);
    }
    assert_eq!(
        owner_on(&core, LedgerId::XrplTestnetXrp).unwrap() - book,
        i128::from(payout * UNITS_PER_DROP)
    );
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        2,
        "nothing into the old hub"
    );
}

// spec/decisions/hub-named-when-added.md: a hub left is let go of, so the
// scheme is added again with another hub's rules, as one never added:
// nothing of the old hub is restored, nothing is reconciled, and it runs
// so after a restart.
#[test]
fn a_hub_left_is_joined_again_as_another() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let Ended { mut core, .. } = ended_with_a_stuck_deposit(&store);
    assert_eq!(core.leave_hub(SCHEME_ID), Ok(true));
    core.add_scheme(SCHEME_ID, hub_config(700, 300)).unwrap();
    let events = step_events(&mut core);
    assert_eq!(mended(&events), None, "{events:?}");
    assert_eq!(owner_on(&core, LedgerId::HubXrplTestnetXrp), Some(0));
    let joined = hub_rules(700, 300).id();
    let ids = |core: &Core| {
        core.hub_rules()
            .iter()
            .map(dassie_hub::Rules::id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&core), [joined]);
    let status = core.hub_status().unwrap();
    assert!(!status.stopped && !status.ended && status.moving.is_none());
    core.flush();

    let mut core = restarted(&store, core);
    assert_eq!(ids(&core), [joined]);
    assert_eq!(mended(&step_events(&mut core)), None);
}

// A leave that waits for its peerings' methods is kept with the removal: a
// restart carries both on, and the hub's state goes once the scheme does.
#[test]
fn a_leave_under_way_carries_on_after_a_restart() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub) = holding_core(&store, &mut ledger);
    let escrow = hub.escrow();
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, opening, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let held = core.settlement_held(LedgerId::HubXrplTestnetXrp);
    core.flush();
    let now = core.now_ms() + 1_000;
    drop(core);
    // What a node leaving while its methods wind down keeps.
    store
        .commit_ledger(&LedgerBatch {
            at_ms: now,
            schemes_removing: vec![(hub_scheme(), now)],
            schemes_leaving: vec![hub_scheme()],
            ..LedgerBatch::default()
        })
        .unwrap();

    let mut core = core_taking_downlinks(Arc::clone(&store), Realm::Test, LiveRealm::Refused, now);
    assert!(core.schemes[&hub_scheme()].leaving);
    // Leaving again succeeds, even before the payout keeper has told the
    // engine again that the hub ended.
    assert_eq!(core.leave_hub(SCHEME_ID), Ok(false));
    // The wait counts from the start, once no method is on the scheme.
    let wait = core.removal_wait_ms();
    core.begin_step(now + 1_000);
    let mut events = step_events(&mut core);
    assert!(core.hub_status().is_some(), "it waits");
    core.begin_step(now + wait + 2_000);
    events.extend(step_events(&mut core));
    assert!(
        events.iter().any(|event| matches!(
            event,
            SettlementEvent::HubLeft { written_off, .. } if *written_off == held
        )),
        "{events:?}"
    );
    assert!(core.hub_status().is_none());
    core.flush();
    assert!(
        !core
            .store
            .checkpoints()
            .unwrap()
            .iter()
            .any(|(id, _)| *id == hub_scheme())
    );
    assert!(core.store.removing_schemes().unwrap().is_empty());
}

/// A node in the tests' XRP hub on `store` whose account the hub
/// credited, its books holding it, kept: its core, and what its books
/// hold of the account.
fn credited_hub(store: &Arc<Store>) -> (Core, u64) {
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub) = holding_core(store, &mut ledger);
    let escrow = hub.escrow();
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, opening, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let held = core.settlement_held(LedgerId::HubXrplTestnetXrp);
    assert!(held > 0);
    core.flush();
    (core, held)
}

/// A core started again on `store`, a moment after `core` stopped.
fn restarted(store: &Arc<Store>, core: Core) -> Core {
    let now = core.now_ms() + 1_000;
    drop(core);
    core_taking_downlinks(Arc::clone(store), Realm::Test, LiveRealm::Refused, now)
}

/// What an operator who moves the hub scheme to another hub by hand
/// deletes: the scheme's row, its checkpoint, its account's and its hub's
/// funding, leaving the books of the hub's ledger as they are; then the
/// scheme added again with the rules of `config`, and, if the node ran it
/// before it updated, the checkpoint `kept` its first sync there wrote.
fn moved_by_hand(store: &Store, config: String, kept: Option<u64>) {
    store
        .commit_ledger(&LedgerBatch {
            at_ms: NOW,
            schemes_removed: vec![hub_scheme()],
            schemes_let_go: vec![(hub_scheme(), SCHEME_ID.into())],
            ..LedgerBatch::default()
        })
        .unwrap();
    store
        .commit_ledger(&LedgerBatch {
            at_ms: NOW,
            schemes_added: vec![(hub_scheme(), config)],
            checkpoints: kept
                .map(|balance| {
                    (
                        hub_scheme(),
                        Checkpoint {
                            balance,
                            state: Vec::new(),
                        },
                    )
                })
                .into_iter()
                .collect(),
            ..LedgerBatch::default()
        })
        .unwrap();
}

// An ordinary first join and a restart reconcile nothing: the books of a
// hub's ledger are what its account's checkpoint says.
#[test]
fn a_join_and_a_restart_mend_no_books() {
    let (mut ledger, _) = funded_ledger();
    let mut core = super::core_on(Arc::new(Store::open_in_memory().unwrap()), 9);
    super::add_xrpl(&mut core, "xrpl-testnet-xrp", super::SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    core.add_scheme(SCHEME_ID, hub_config(600, 300)).unwrap();
    assert_eq!(mended(&step_events(&mut core)), None, "a first join");
    drop(core);

    let store = Arc::new(Store::open_in_memory().unwrap());
    let (core, held) = credited_hub(&store);
    let mut core = restarted(&store, core);
    assert_eq!(mended(&step_events(&mut core)), None, "a restart");
    assert_eq!(core.settlement_held(LedgerId::HubXrplTestnetXrp), held);
}

// A node moved to another hub by hand, before leaving was built, kept the
// old hub's books under the new hub's account: what the payout returned to
// the owner book counted twice. As the scheme starts, the books are
// reconciled through suspense to what the account's checkpoint says,
// nothing for one with none, and the owner told: whether the node restarts
// before it ran the new hub, or after its first sync there wrote one.
#[test]
fn books_a_move_by_hand_left_behind_are_mended() {
    for kept in [None, Some(1_000)] {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let (core, held) = credited_hub(&store);
        moved_by_hand(&store, hub_config(700, 300), kept);
        let mut core = restarted(&store, core);
        let events = step_events(&mut core);
        assert_eq!(
            mended(&events),
            Some((held, kept.unwrap_or(0))),
            "{kept:?}: {events:?}"
        );
        assert_eq!(
            core.settlement_held(LedgerId::HubXrplTestnetXrp),
            kept.unwrap_or(0),
            "{kept:?}"
        );
        let owner = owner_on(&core, LedgerId::HubXrplTestnetXrp).unwrap();
        assert_eq!(
            owner,
            i128::from(kept.unwrap_or(0)),
            "{kept:?}: counted once"
        );
        let mut core = restarted(&store, core);
        assert_eq!(mended(&step_events(&mut core)), None, "mended once");
    }
}
