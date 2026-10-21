//! Two schemes on the node's first XRPL testnet account, against the fake
//! ledger, as RLUSD will share it with `xrpl-testnet`
//! (spec/areas/settlement.md#external-assets): the `xrpl-testnet` scheme,
//! and a witness that books nothing and keeps the account running while
//! the other is away. What a scheme misses while it is not on the account,
//! it reads again once back, and books once.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_ledger::Name;
use dassie_node_core::testing::SeededRng;
use dassie_settlement::{
    AssetEffect, Effect, ExternalAsset, LedgerInput, LedgerStatus, SettlementScheme,
};
use dassie_xrpl::account::{self, XrplAccount};
use dassie_xrpl::fake::{FIRST_LEDGER, FakeLedger, RESERVE_BASE};
use dassie_xrpl::{AccountId, UNITS_PER_DROP, Wallet, XrplScheme};

const ENTROPY: [u8; 32] = [1; 32];
const XRPL: Name = dassie_ledger::name!(dassie_xrpl::scheme::SCHEME_ID);
const WITNESS: Name = dassie_ledger::name!("witness");
/// What the `xrpl-testnet` scheme settles: 50 XRP.
const AMOUNT: u64 = 50_000_000 * UNITS_PER_DROP;

/// The account as the node runs it, with both schemes on it.
struct Shared {
    ledger: FakeLedger,
    account: XrplAccount,
    /// The `xrpl-testnet` scheme, once active.
    scheme: Option<XrplScheme>,
    /// Whether the books stopped it: it is told nothing more.
    stopped: bool,
    /// Where the witness reads from.
    witness: Option<(u32, u32)>,
    /// What the `xrpl-testnet` scheme booked, in order.
    booked: Vec<Effect>,
    connection: u64,
    /// Whether the network loses submissions.
    drop_submits: bool,
    now: u64,
}

impl Shared {
    fn new(ledger: FakeLedger, state: &[u8], witness: Option<(u32, u32)>) -> Self {
        let schemes = [XRPL, WITNESS];
        let account = XrplAccount::new(account::FIRST, &schemes, "", &ENTROPY, state).unwrap();
        Self {
            ledger,
            account,
            scheme: None,
            stopped: false,
            witness,
            booked: Vec::new(),
            connection: 1,
            drop_submits: false,
            now: 1_000,
        }
    }

    fn address(&self) -> AccountId {
        self.account.account()
    }

    fn scheme(&self) -> &XrplScheme {
        self.scheme.as_ref().unwrap()
    }

    /// Activates the `xrpl-testnet` scheme from what the books kept for it.
    fn activate(&mut self, checkpoint: Option<u64>, state: &[u8]) {
        let node = Name::new("node").unwrap();
        let scheme = XrplScheme::new(&self.account, node, checkpoint, state).unwrap();
        self.scheme = Some(scheme);
        self.stopped = false;
        self.tell();
    }

    /// Stops the `xrpl-testnet` scheme, as the books refusing what it said
    /// does: the account lets go of it, and the witness runs on.
    fn stop(&mut self) {
        self.stopped = true;
        self.account.detach(XRPL);
    }

    /// Tells every scheme on the account what it learnt, as the node does,
    /// and carries out what the account asks.
    fn tell(&mut self) {
        if let (Some(scheme), false) = (&mut self.scheme, self.stopped) {
            scheme.on_asset(self.now, &mut self.account, &mut self.booked);
            scheme.check_invariants();
        }
        self.account.follow(WITNESS, self.witness, false);
        self.account.told();
        self.account.check_invariants();
        let mut effects = Vec::new();
        self.account.take_effects(&mut effects);
        for effect in effects {
            match effect {
                AssetEffect::Send { message } => self.request(&message),
                AssetEffect::Reconnect => self.connect(),
                AssetEffect::Fund { .. } => panic!("the account exists"),
            }
        }
    }

    fn input(&mut self, input: LedgerInput<'_>) {
        self.account.on_ledger(self.now, input);
        self.tell();
    }

    fn request(&mut self, message: &[u8]) {
        let request: serde_json::Value = serde_json::from_slice(message).unwrap();
        if self.drop_submits && request["command"] == "submit" {
            return;
        }
        let reply = self.ledger.request(self.connection, message);
        self.input(LedgerInput::Message(&reply));
    }

    /// Connects the account anew, as the node does when it asks, and when
    /// a scheme joins an account that has read its ledger.
    fn connect(&mut self) {
        self.ledger.disconnect(self.connection);
        self.connection += 1;
        self.account.on_ledger(self.now, LedgerInput::Disconnected);
        self.input(LedgerInput::Connected);
    }

    fn close(&mut self) {
        self.now += 4_000;
        for (connection, message) in self.ledger.close() {
            if connection == self.connection {
                self.input(LedgerInput::Message(&message));
            }
        }
    }

    /// Settles [`AMOUNT`] with `to`, prepared and executed. Returns its ID.
    fn settle(&mut self, to: AccountId) -> Name {
        let address = to.to_address();
        let mut peer_state = vec![u8::try_from(address.len()).unwrap()];
        peer_state.extend_from_slice(address.as_bytes());
        let peer = Name::new("peer").unwrap();
        let mut rng = SeededRng::new(1);
        let scheme = self.scheme.as_mut().unwrap();
        let prepared = scheme
            .prepare_settlement(&mut rng, Some(&mut self.account), peer, AMOUNT, &peer_state)
            .unwrap();
        assert_eq!(prepared.amount, AMOUNT);
        scheme.execute_settlement(Some(&mut self.account), prepared.id, &mut self.booked);
        self.tell();
        prepared.id
    }

    /// Hands the scheme what the books still hold pending, as the node does
    /// once it activated.
    fn reconcile(&mut self, pending: &[(Name, u64)]) {
        let scheme = self.scheme.as_mut().unwrap();
        scheme.reconcile(Some(&mut self.account), pending, &mut self.booked);
        self.tell();
    }

    /// What the books should hold: the balance less the reserve.
    fn bookable(&self) -> u64 {
        (self.ledger.balance(self.address()).unwrap() - RESERVE_BASE) * UNITS_PER_DROP
    }

    fn count(&self, matches: impl Fn(&Effect) -> bool) -> usize {
        self.booked.iter().filter(|effect| matches(effect)).count()
    }
}

/// The account funded and synced, with both schemes on it, and the
/// address of a peer's account, funded too.
fn synced() -> (Shared, AccountId) {
    let mut ledger = FakeLedger::new();
    let theirs = Wallet::from_entropy(&[2; 32]).account();
    ledger.fund(Wallet::from_entropy(&ENTROPY).account());
    ledger.fund(theirs);
    ledger.close();
    let mut shared = Shared::new(ledger, &[], None);
    shared.activate(None, &[]);
    shared.connect();
    shared.close();
    assert!(shared.scheme().has_synced());
    (shared, theirs)
}

/// The `xrpl-testnet` scheme settles, then stops while its transaction is
/// on its way; what happens to it then, `meanwhile` makes. The node then
/// restarts: the account from what the witness had it write, the scheme
/// from what it wrote before it stopped, with its settlement still pending
/// in the books.
fn stopped_while_settling(drop_submits: bool, meanwhile: usize) -> (Shared, Name) {
    let (mut shared, theirs) = synced();
    shared.drop_submits = drop_submits;
    let id = shared.settle(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    for _ in 0..meanwhile {
        shared.close();
    }
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.activate(Some(checkpoint), &state);
    restarted.reconcile(&[(id, AMOUNT)]);
    restarted.connect();
    restarted.close();
    restarted.close();
    (restarted, id)
}

// spec/areas/settlement.md#external-assets: a scheme that joins an account
// another scheme reads from earlier is told the account has synced, which
// books what the account holds as the owner's deposit, and none of the
// history the account reads again for the other.
#[test]
fn a_scheme_that_joins_books_its_first_sync_alone() {
    let mut ledger = FakeLedger::new();
    let address = Wallet::from_entropy(&ENTROPY).account();
    ledger.fund(address);
    ledger.close();
    ledger.deposit(address, 5_000_000);
    ledger.close();
    let mut shared = Shared::new(ledger, &[], Some((FIRST_LEDGER, 0)));
    shared.connect();
    shared.close();
    assert_eq!(shared.account.status(), LedgerStatus::Synced);
    shared.activate(None, &[]);
    shared.connect();
    shared.close();
    assert!(shared.scheme().has_synced());
    assert_eq!(shared.scheme().status(), LedgerStatus::Synced);
    let deposit = Effect::Deposit {
        amount: shared.bookable(),
    };
    assert_eq!(shared.booked, [deposit]);
    assert_eq!(shared.scheme().balance(), shared.bookable());
}

// spec/areas/settlement.md#external-assets: a settlement the ledger
// validated while its scheme was stopped, and the other scheme on the
// account ran on, is booked once after a restart: posted with its fee,
// never also as a withdrawal.
#[test]
fn a_settlement_validated_while_its_scheme_stopped_is_booked_once() {
    let (shared, id) = stopped_while_settling(false, 2);
    let finalized = shared.count(|e| *e == Effect::Finalize { settlement: id });
    let fees = shared.count(|e| matches!(e, Effect::Fee { id: fee, .. } if *fee == id));
    let withdrawals = shared.count(|e| matches!(e, Effect::Withdrawal { .. }));
    let cancelled = shared.count(|e| matches!(e, Effect::Cancel { .. }));
    assert_eq!(
        (finalized, fees, withdrawals, cancelled),
        (1, 1, 0, 0),
        "{:?}",
        shared.booked
    );
    assert_eq!(shared.scheme().balance(), shared.bookable());
    assert_eq!(shared.scheme().status(), LedgerStatus::Synced);
}

// spec/areas/settlement.md#external-assets: a settlement that can never
// apply, found so while its scheme was stopped, is cancelled after a
// restart, rather than left pending for good.
#[test]
fn a_settlement_given_up_while_its_scheme_stopped_is_cancelled() {
    let (shared, id) = stopped_while_settling(true, 25);
    let cancelled = shared.count(|e| *e == Effect::Cancel { settlement: id });
    let finalized = shared.count(|e| matches!(e, Effect::Finalize { .. }));
    assert_eq!((cancelled, finalized), (1, 0), "{:?}", shared.booked);
    assert_eq!(shared.scheme().balance(), shared.bookable());
}
