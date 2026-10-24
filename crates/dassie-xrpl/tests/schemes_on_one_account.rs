//! Two schemes on the node's first XRPL testnet account, against the fake
//! ledger, as DSUSD shares it with `xrpl-testnet`
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
    AssetEffect, Detach, Effect, ExternalAsset, LedgerInput, LedgerStatus, SettlementScheme,
};
use dassie_xrpl::account::{self, News, XrplAccount};
use dassie_xrpl::fake::{FIRST_LEDGER, FakeLedger, RESERVE_BASE};
use dassie_xrpl::{AccountId, UNITS_PER_DROP, Wallet, XrplScheme};

const ENTROPY: [u8; 32] = [1; 32];
const XRPL: Name = dassie_ledger::name!(dassie_xrpl::scheme::SCHEME_ID);
const WITNESS: Name = dassie_ledger::name!("witness");
/// What the `xrpl-testnet` scheme settles: 50 XRP.
const AMOUNT: u64 = 50_000_000 * UNITS_PER_DROP;
/// What the owner pays out of the account: 20 XRP.
const OWNER_AMOUNT: u64 = 20_000_000 * UNITS_PER_DROP;

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
    /// What the books held for the `xrpl-testnet` scheme when it was last
    /// activated.
    checkpoint: u64,
    /// What the `xrpl-testnet` scheme booked since, in order.
    booked: Vec<Effect>,
    connection: u64,
    /// Whether the network loses submissions.
    drop_submits: bool,
    /// Whether the server answers no lookup of a transaction.
    drop_lookups: bool,
    /// Whether the books refuse what the `xrpl-testnet` scheme is next told
    /// of its own, which stops it.
    refuse_own: bool,
    /// What a server's answer to a lookup is changed to say.
    tamper_lookup: Option<fn(&mut serde_json::Value)>,
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
            checkpoint: 0,
            booked: Vec::new(),
            connection: 1,
            drop_submits: false,
            drop_lookups: false,
            refuse_own: false,
            tamper_lookup: None,
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
        self.checkpoint = checkpoint.unwrap_or(0);
        self.stopped = false;
        self.tell();
    }

    /// Stops the `xrpl-testnet` scheme, as the books refusing what it said
    /// does: the account lets go of it, and the witness runs on.
    fn stop(&mut self) {
        self.stopped = true;
        self.account.detach(XRPL, Detach::Stopped);
    }

    /// Tells every scheme on the account what it learnt, as the node does,
    /// and carries out what the account asks.
    /// With [`refuse_own`](Self::refuse_own), the books refuse what it
    /// says when it is told anything of its own, and the node stops it
    /// before the account forgets what it told, as it does on a refusal.
    fn tell(&mut self) {
        let own = self.account.news().iter().any(|news| match news {
            News::Transaction {
                ours: Some(ours), ..
            }
            | News::Unread { ours, .. } => ours.scheme == XRPL,
            News::Cancelled { scheme, .. } => *scheme == XRPL,
            _ => false,
        });
        if let (Some(scheme), false) = (&mut self.scheme, self.stopped) {
            let mut effects = Vec::new();
            scheme.on_asset(self.now, &mut self.account, &mut effects);
            if self.refuse_own && own {
                self.refuse_own = false;
                self.stopped = true;
                self.account.detach(XRPL, Detach::Stopped);
            } else {
                scheme.check_invariants();
                self.booked.append(&mut effects);
            }
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
                AssetEffect::SendTo { .. } => panic!("an XRPL account names no endpoints"),
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
        if self.drop_lookups && request["command"] == "tx" {
            return;
        }
        let mut reply = self.ledger.request(self.connection, message);
        if let (Some(tamper), true) = (self.tamper_lookup, request["command"] == "tx") {
            let mut answer: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            tamper(&mut answer);
            reply = serde_json::to_vec(&answer).unwrap();
        }
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

    /// The owner pays [`OWNER_AMOUNT`] to `to`. Returns its ID.
    fn pay_owner(&mut self, to: AccountId) -> Name {
        let payment = dassie_settlement::OwnerPayment {
            destination: to.to_address(),
            tag: Some(7),
            memo: None,
            amount: OWNER_AMOUNT,
        };
        let scheme = self.scheme.as_mut().unwrap();
        let id = scheme.pay_owner(Some(&mut self.account), &payment).unwrap();
        self.tell();
        id
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

    /// What the books hold now, as the node books what the scheme said
    /// since it was activated: a settlement of ours posted takes
    /// [`AMOUNT`] off them.
    fn books(&self) -> i128 {
        let moved = |effect: &Effect| -> i128 {
            match *effect {
                Effect::Deposit { amount } | Effect::IncomingSettlement { amount, .. } => {
                    i128::from(amount)
                }
                Effect::Withdrawal { amount } | Effect::Fee { amount, .. } => -i128::from(amount),
                Effect::Reconcile { amount, gain, .. } if gain => i128::from(amount),
                Effect::Reconcile { amount, .. } => -i128::from(amount),
                Effect::Finalize { .. } => -i128::from(AMOUNT),
                Effect::OwnerPaid { applied: true, .. } => -i128::from(OWNER_AMOUNT),
                _ => 0,
            }
        };
        i128::from(self.checkpoint) + self.booked.iter().map(moved).sum::<i128>()
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
/// in the books. With `short_history`, the server's account history no
/// longer reaches back to the settlement when it does.
fn stopped_while_settling(
    drop_submits: bool,
    meanwhile: usize,
    short_history: bool,
) -> (Shared, Name) {
    let (mut shared, theirs) = synced();
    shared.drop_submits = drop_submits;
    let id = shared.settle(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    for _ in 0..meanwhile {
        shared.close();
    }
    if short_history {
        shared.ledger.shorten_account_history();
    }
    (restart(shared, checkpoint, &state, id), id)
}

/// The node restarts: the account from what the witness had it write, the
/// `xrpl-testnet` scheme from `checkpoint` and `state`, with settlement
/// `id` still pending in the books.
fn restart(shared: Shared, checkpoint: u64, state: &[u8], id: Name) -> Shared {
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.tamper_lookup = shared.tamper_lookup;
    restarted.activate(Some(checkpoint), state);
    restarted.reconcile(&[(id, AMOUNT)]);
    restarted.connect();
    restarted.close();
    restarted.close();
    restarted
}

/// The `xrpl-testnet` scheme settles, and the books refuse what it is told
/// when the ledger validates the settlement, or, with `drop_submits`, when
/// the account gives it up: its checkpoint stays as it was, while the
/// witness keeps the account's written. The node then restarts, as after
/// [`stopped_while_settling`].
fn refused_while_settling(drop_submits: bool, short_history: bool) -> (Shared, Name) {
    let (mut shared, theirs) = synced();
    shared.drop_submits = drop_submits;
    let id = shared.settle(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.refuse_own = true;
    for _ in 0..25 {
        if shared.stopped {
            break;
        }
        shared.close();
    }
    assert!(shared.stopped, "the books refused what the scheme was told");
    shared.close();
    if short_history {
        shared.ledger.shorten_account_history();
    }
    (restart(shared, checkpoint, &state, id), id)
}

/// What suspense booked, net.
fn suspense(booked: &[Effect]) -> i128 {
    booked
        .iter()
        .map(|effect| match *effect {
            Effect::Reconcile { amount, gain, .. } if gain => i128::from(amount),
            Effect::Reconcile { amount, .. } => -i128::from(amount),
            _ => 0,
        })
        .sum()
}

/// Asserts that settlement `id` was booked once, posted with its fee and
/// never as a withdrawal, and that the books hold the balance.
fn booked_once(shared: &Shared, id: Name) {
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
    assert_eq!(suspense(&shared.booked), 0, "{:?}", shared.booked);
    assert_eq!(shared.scheme().balance(), shared.bookable());
    assert_eq!(
        shared.books(),
        i128::from(shared.bookable()),
        "{:?}",
        shared.booked
    );
    assert_eq!(shared.scheme().status(), LedgerStatus::Synced);
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
    let (shared, id) = stopped_while_settling(false, 2, false);
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
    assert_eq!(shared.books(), i128::from(shared.bookable()));
    assert_eq!(shared.scheme().status(), LedgerStatus::Synced);
}

// Found in review: as above, with a server whose account history no longer
// reaches back to the settlement. The sync books what the balance lost
// through suspense; the settlement, looked up by its hash, is posted with
// its fee, and what the balance booked comes back out of suspense, so the
// books hold the balance once, not the settlement twice.
#[test]
fn a_settlement_validated_while_its_scheme_stopped_is_booked_once_without_its_history() {
    let (shared, id) = stopped_while_settling(false, 2, true);
    let finalized = shared.count(|e| *e == Effect::Finalize { settlement: id });
    let fees = shared.count(|e| matches!(e, Effect::Fee { id: fee, .. } if *fee == id));
    let withdrawals = shared.count(|e| matches!(e, Effect::Withdrawal { .. }));
    assert_eq!(
        (finalized, fees, withdrawals),
        (1, 1, 0),
        "{:?}",
        shared.booked
    );
    let suspense: i128 = shared
        .booked
        .iter()
        .map(|effect| match *effect {
            Effect::Reconcile { amount, gain, .. } if gain => i128::from(amount),
            Effect::Reconcile { amount, .. } => -i128::from(amount),
            _ => 0,
        })
        .sum();
    assert_eq!(suspense, 0, "what suspense booked came back out");
    assert_eq!(shared.scheme().balance(), shared.bookable());
    assert_eq!(
        shared.books(),
        i128::from(shared.bookable()),
        "{:?}",
        shared.booked
    );
    assert_eq!(shared.scheme().status(), LedgerStatus::Synced);
}

// As above, with the node restarting again after the sync and before the
// lookup: the scheme keeps that it never read the settlement with its
// state, so it still books it as one it never read.
#[test]
fn a_settlement_never_read_is_booked_once_across_a_restart() {
    let (mut shared, theirs) = synced();
    let id = shared.settle(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    shared.close();
    shared.close();
    shared.ledger.shorten_account_history();
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.activate(Some(checkpoint), &state);
    restarted.reconcile(&[(id, AMOUNT)]);
    // Synced, and told what it did not read, but not yet looked up.
    restarted.drop_lookups = true;
    restarted.connect();
    let synced_books = restarted.books();
    let (state, checkpoint) = (restarted.scheme().state(), restarted.scheme().balance());
    assert_eq!(synced_books, i128::from(checkpoint));
    let kept = restarted.account.state();
    let mut again = Shared::new(restarted.ledger, &kept, None);
    again.activate(Some(checkpoint), &state);
    again.reconcile(&[(id, AMOUNT)]);
    again.connect();
    again.close();
    again.close();
    let finalized = again.count(|e| *e == Effect::Finalize { settlement: id });
    assert_eq!(finalized, 1, "{:?}", again.booked);
    assert_eq!(again.scheme().balance(), again.bookable());
    assert_eq!(
        again.books(),
        i128::from(again.bookable()),
        "{:?}",
        again.booked
    );
}

// spec/areas/settlement.md#external-assets: a settlement that can never
// apply, found so while its scheme was stopped, is cancelled after a
// restart, rather than left pending for good.
#[test]
fn a_settlement_given_up_while_its_scheme_stopped_is_cancelled() {
    let (shared, id) = stopped_while_settling(true, 25, false);
    let cancelled = shared.count(|e| *e == Effect::Cancel { settlement: id });
    let finalized = shared.count(|e| matches!(e, Effect::Finalize { .. }));
    assert_eq!((cancelled, finalized), (1, 0), "{:?}", shared.booked);
    assert_eq!(shared.scheme().balance(), shared.bookable());
}

// Found in review: the books refuse what a scheme is told with its own
// settlement, validated, and stop it. The account no longer has it in
// flight, and its state is written on for the other scheme on it, while the
// scheme's checkpoint stays from before. The account keeps the settlement
// for the scheme, as for one not on the account, so after a restart the
// scheme reads it again as its own: posted once with its fee, never also
// booked as a withdrawal.
#[test]
fn a_settlement_told_in_a_refused_batch_is_booked_once() {
    let (shared, id) = refused_while_settling(false, false);
    booked_once(&shared, id);
}

// As above, with a server whose account history no longer reaches back to
// the settlement: the scheme is told it did not read it again, and books it
// once found.
#[test]
fn a_settlement_told_in_a_refused_batch_is_booked_once_without_its_history() {
    let (shared, id) = refused_while_settling(false, true);
    booked_once(&shared, id);
}

// As above, with the books refusing what the scheme is told after the
// restart, that it did not read its settlement again: told again after the
// next restart, it still books the settlement once.
#[test]
fn a_settlement_unread_in_a_refused_batch_is_booked_once() {
    let (mut shared, theirs) = synced();
    let id = shared.settle(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    shared.close();
    shared.close();
    shared.ledger.shorten_account_history();
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.activate(Some(checkpoint), &state);
    restarted.reconcile(&[(id, AMOUNT)]);
    restarted.refuse_own = true;
    restarted.connect();
    assert!(
        restarted.stopped,
        "the books refused what the scheme was told"
    );
    restarted.close();
    let again = restart(restarted, checkpoint, &state, id);
    booked_once(&again, id);
}

// A settlement given up while the books refuse what its scheme is told of
// it is cancelled after a restart, rather than looked up for good.
#[test]
fn a_settlement_given_up_in_a_refused_batch_is_cancelled() {
    let (shared, id) = refused_while_settling(true, false);
    let cancelled = shared.count(|e| *e == Effect::Cancel { settlement: id });
    let finalized = shared.count(|e| matches!(e, Effect::Finalize { .. }));
    assert_eq!((cancelled, finalized), (1, 0), "{:?}", shared.booked);
    assert_eq!(shared.scheme().balance(), shared.bookable());
}

/// Past what the books can hold for the account, within what the parser
/// takes.
const TOO_MANY_DROPS: &str = "99999999999999999";

// Found in review: a server's answer to a lookup names more XRP than the
// books can hold, for its fee or for what it delivered. The account stops,
// as it does for such a transaction from the stream, and the scheme books
// nothing of it, rather than the node panicking.
#[test]
fn a_lookup_past_what_the_books_hold_stops_the_account() {
    let tampers: [fn(&mut serde_json::Value); 2] = [
        |answer| answer["result"]["Fee"] = TOO_MANY_DROPS.into(),
        |answer| answer["result"]["meta"]["delivered_amount"] = TOO_MANY_DROPS.into(),
    ];
    for tamper in tampers {
        let (mut shared, theirs) = synced();
        let id = shared.settle(theirs);
        let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
        shared.stop();
        shared.close();
        shared.close();
        shared.ledger.shorten_account_history();
        shared.tamper_lookup = Some(tamper);
        let restarted = restart(shared, checkpoint, &state, id);
        assert!(
            matches!(restarted.account.status(), LedgerStatus::Failed(_)),
            "{:?}",
            restarted.account.status()
        );
        let settled = restarted.count(|e| match e {
            Effect::Finalize { settlement } | Effect::Cancel { settlement } => *settlement == id,
            Effect::Fee { id: fee, .. } => *fee == id,
            _ => false,
        });
        assert_eq!(settled, 0, "{:?}", restarted.booked);
    }
}

// Found in review: an owner's payment the ledger validated while the
// `xrpl-testnet` scheme was stopped, and the other scheme kept the account
// running, is kept for the scheme; after a restart the scheme reads it
// again as the owner's payment, and its hold is posted once: never voided
// as one whose outcome is unknown, nor read as a settlement.
#[test]
fn an_owner_payment_validated_while_its_scheme_stopped_is_posted_once() {
    let (mut shared, theirs) = synced();
    let id = shared.pay_owner(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    shared.close();
    shared.close();
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.activate(Some(checkpoint), &state);
    restarted.connect();
    restarted.close();
    restarted.close();
    let posted = restarted.count(|e| *e == Effect::OwnerPaid { id, applied: true });
    let voided = restarted.count(|e| matches!(e, Effect::OwnerPaid { applied: false, .. }));
    let fees = restarted.count(|e| matches!(e, Effect::Fee { id: fee, .. } if *fee == id));
    let settled = restarted.count(|e| matches!(e, Effect::Finalize { .. } | Effect::Cancel { .. }));
    let withdrawals = restarted.count(|e| matches!(e, Effect::Withdrawal { .. }));
    assert_eq!(
        (posted, voided, fees, settled, withdrawals),
        (1, 0, 1, 0, 0),
        "{:?}",
        restarted.booked
    );
    assert_eq!(suspense(&restarted.booked), 0, "{:?}", restarted.booked);
    assert_eq!(restarted.scheme().balance(), restarted.bookable());
    assert_eq!(restarted.books(), i128::from(restarted.bookable()));
}
