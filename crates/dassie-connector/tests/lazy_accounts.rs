//! A peer added without accounts: a downlink whose accounts open with the
//! first packet that moves money to it, with the packet's transfers or not
//! at all (spec/decisions/lazy-downlink-accounts.md).

#![allow(clippy::panic, clippy::unwrap_used)]

use dassie_connector::sim::*;
use dassie_connector::{Config, EndpointId, OpensAccounts, Outcome};
use dassie_ilp::ErrorCode;
use dassie_ledger::{AccountId, AccountName, AccountPath, Ledger, LedgerId, Name};

const ALICE: EndpointId = EndpointId::Btp(1);
const LAZY: EndpointId = EndpointId::Peer(7);
const OTHER: EndpointId = EndpointId::Peer(8);
const UNIT: u64 = 1_000_000_000;

/// Opens the downlink `dlazy`'s accounts while it has room, and counts what
/// it opened and took back.
#[derive(Default)]
struct Opener {
    room: bool,
    opened: usize,
    taken_back: usize,
}

fn lazy() -> Name {
    Name::new("dlazy").unwrap()
}

impl OpensAccounts for Opener {
    fn open(
        &mut self,
        now_ms: u64,
        ledger: &mut Ledger,
        endpoint: EndpointId,
        on: LedgerId,
    ) -> Option<AccountId> {
        assert_eq!(endpoint, LAZY);
        if !self.room {
            return None;
        }
        self.opened += 1;
        ledger
            .open_peer(now_ms, on, lazy(), 0)
            .ok()
            .map(|(id, _)| id)
    }

    fn take_back(&mut self, ledger: &mut Ledger, endpoint: EndpointId, on: LedgerId) {
        assert_eq!(endpoint, LAZY);
        self.taken_back += 1;
        ledger.unopen_peer(on, lazy());
    }
}

/// A harness with `funds`, the downlink `dlazy` without accounts, and the
/// downlink `dother` without accounts either.
fn harness(funds: u64) -> Harness {
    let mut h = Harness::new(Config::DEFAULT, funds);
    for (id, node) in [(LAZY, "dlazy"), (OTHER, "dother")] {
        h.connector
            .add_peer(id, &format!("{OWN}.{node}"), None, LedgerId::StubUsd, 0)
            .unwrap();
    }
    h
}

/// Sends `amount` from `from` to `to`, below our address, with a request ID
/// of its own.
fn send(h: &mut Harness, from: EndpointId, to: &str, amount: u64, opener: &mut Opener) -> Outcome {
    let packet = prepare(&format!("{OWN}.{to}"), amount, NOW + 20_000, [1; 32]);
    let prices = h.prices(NOW);
    let request_id = u32::try_from(h.connector.pending_count()).unwrap();
    let outcome = h.connector.handle_prepare_with(
        NOW,
        (from, request_id),
        &packet,
        &mut h.ledger,
        &prices,
        opener,
        0,
        &mut h.buffer,
    );
    h.check();
    outcome
}

fn rejected(h: &Harness, outcome: Outcome) -> ErrorCode {
    match outcome {
        Outcome::Respond(length) => h.reject_written(length).0,
        other => panic!("expected a reject, got {other:?}"),
    }
}

fn account(h: &Harness) -> Option<AccountId> {
    h.ledger.find(&AccountPath::new(
        LedgerId::StubUsd,
        AccountName::AssetsInterledger(lazy()),
    ))
}

#[test]
fn a_packet_that_moves_money_to_a_downlink_opens_its_accounts() {
    let mut h = harness(UNIT);
    let mut opener = Opener {
        room: true,
        ..Opener::default()
    };
    // Nothing moves: nothing opens.
    assert!(matches!(
        send(&mut h, ALICE, "dlazy", 0, &mut opener),
        Outcome::Forward(_)
    ));
    assert_eq!((opener.opened, account(&h)), (0, None));
    // Money moves: its accounts open, and take the packet's transfer.
    assert!(matches!(
        send(&mut h, ALICE, "dlazy.x", 100, &mut opener),
        Outcome::Forward(_)
    ));
    let opened = account(&h).expect("its accounts opened");
    assert_eq!(h.connector.account_of(LAZY), Some(opened));
    let pending = h.ledger.pending_transfers().next().unwrap();
    assert_eq!((pending.credit, pending.amount), (opened, 100));
    // The next one books on them, and opens nothing.
    assert!(matches!(
        send(&mut h, ALICE, "dlazy.y", 100, &mut opener),
        Outcome::Forward(_)
    ));
    assert_eq!(opener.opened, 1);
}

#[test]
fn accounts_opened_for_a_packet_the_books_refuse_are_taken_back() {
    let mut h = harness(100);
    let mut opener = Opener {
        room: true,
        ..Opener::default()
    };
    let accounts = h.ledger.accounts().count();
    let outcome = send(&mut h, ALICE, "dlazy", 101, &mut opener);
    assert_eq!(rejected(&h, outcome), ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
    assert_eq!((opener.opened, opener.taken_back), (1, 1));
    assert_eq!((h.ledger.accounts().count(), account(&h)), (accounts, None));
    assert_eq!(h.connector.account_of(LAZY), None);
}

#[test]
fn without_room_for_its_accounts_a_downlink_is_paid_nothing() {
    let mut h = harness(UNIT);
    let mut opener = Opener::default();
    let outcome = send(&mut h, ALICE, "dlazy", 100, &mut opener);
    assert_eq!(rejected(&h, outcome), ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
    assert_eq!(account(&h), None);
    assert_eq!(h.ledger.pending_transfers().count(), 0);
}

// A downlink without accounts has no credit with us: what it sends that
// carries money is refused as it would be with nothing on them, and opens
// nothing.
#[test]
fn a_downlink_without_accounts_sends_no_money() {
    let mut h = harness(UNIT);
    let mut opener = Opener {
        room: true,
        ..Opener::default()
    };
    let outcome = send(&mut h, OTHER, "alice", 100, &mut opener);
    assert_eq!(rejected(&h, outcome), ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
    assert_eq!(opener.opened, 0);
    assert!(matches!(
        send(&mut h, OTHER, "alice", 0, &mut opener),
        Outcome::Forward(_)
    ));
}
