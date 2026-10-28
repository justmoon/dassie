//! Two schemes on the node's first XRPL testnet account, against the fake
//! ledger, as DSUSD shares it with `xrpl-testnet-xrp`
//! (spec/areas/settlement.md#external-assets): the `xrpl-testnet-xrp` scheme,
//! and a witness that books nothing and keeps the account running while
//! the other is away. What a scheme misses while it is not on the account,
//! such as the owner's payment, it reads again once back, and books once.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_ledger::Name;
use dassie_settlement::{
    AssetEffect, Detach, Effect, ExternalAsset, LedgerInput, LedgerStatus, OwnerPaymentState,
    SettlementScheme,
};
use dassie_xrpl::account::{self, MAX_SETTLING_MS, News, XrplAccount};
use dassie_xrpl::fake::{FIRST_LEDGER, FakeLedger, RESERVE_BASE};
use dassie_xrpl::{AccountId, UNITS_PER_DROP, Wallet, XrplScheme};

const ENTROPY: [u8; 32] = [1; 32];
const XRPL: Name = dassie_ledger::name!(dassie_xrpl::scheme::SCHEME_ID);
const WITNESS: Name = dassie_ledger::name!("witness");
/// What the owner pays out of the account: 20 XRP.
const OWNER_AMOUNT: u64 = 20_000_000 * UNITS_PER_DROP;

/// The account as the node runs it, with both schemes on it.
struct Shared {
    ledger: FakeLedger,
    account: XrplAccount,
    /// The `xrpl-testnet-xrp` scheme, once active.
    scheme: Option<XrplScheme>,
    /// Whether the books stopped it: it is told nothing more.
    stopped: bool,
    /// Where the witness reads from.
    witness: Option<(u32, u32)>,
    /// What the books held for the `xrpl-testnet-xrp` scheme when it was last
    /// activated.
    checkpoint: u64,
    /// What the `xrpl-testnet-xrp` scheme booked since, in order.
    booked: Vec<Effect>,
    connection: u64,
    /// Whether the network loses submissions.
    drop_submits: bool,
    /// Whether the server answers no lookup of a transaction: no proof
    /// asked for without its ledger.
    drop_lookups: bool,
    /// Whether the books refuse what the `xrpl-testnet-xrp` scheme is next told
    /// of its own, which stops it.
    refuse_own: bool,
    /// What a server's answer to a proof of a transaction is changed to
    /// say.
    tamper_lookup: Option<fn(&mut serde_json::Value)>,
    now: u64,
}

impl Shared {
    fn new(ledger: FakeLedger, state: &[u8], witness: Option<(u32, u32)>) -> Self {
        let schemes = [XRPL, WITNESS];
        // It trusts the fake ledger's validators.
        let config = dassie_xrpl::fake::with_publishers("", &dassie_xrpl::network::TESTNET);
        let account = XrplAccount::new(account::FIRST, &schemes, &config, &ENTROPY, state).unwrap();
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
            now: dassie_xrpl::fake::FIRST_CLOSE_MS,
        }
    }

    fn address(&self) -> AccountId {
        self.account.account()
    }

    fn scheme(&self) -> &XrplScheme {
        self.scheme.as_ref().unwrap()
    }

    /// Activates the `xrpl-testnet-xrp` scheme from what the books kept for it.
    fn activate(&mut self, checkpoint: Option<u64>, state: &[u8]) {
        let scheme = XrplScheme::new(&self.account, checkpoint, state).unwrap();
        self.scheme = Some(scheme);
        self.checkpoint = checkpoint.unwrap_or(0);
        self.stopped = false;
        self.tell();
    }

    /// Stops the `xrpl-testnet-xrp` scheme, as the books refusing what it said
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
                AssetEffect::Fetch { url, .. } => {
                    let body = self.ledger.fetch(&url);
                    let fetched = LedgerInput::Fetched {
                        url: &url,
                        body: body.as_deref(),
                    };
                    self.input(fetched);
                }
                AssetEffect::SendTo { .. } => panic!("an XRPL account names no endpoints"),
            }
        }
    }

    fn input(&mut self, input: LedgerInput<'_>) {
        // The node's clock reads the ledger's validations as fresh.
        self.now = self.now.max(self.ledger.now_ms());
        self.account.on_ledger(self.now, input);
        self.tell();
    }

    fn request(&mut self, message: &[u8]) {
        let request: serde_json::Value = serde_json::from_slice(message).unwrap();
        if self.drop_submits && request["command"] == "submit" {
            return;
        }
        let proof = request["command"] == "tx_proof";
        if self.drop_lookups && proof && request.get("ledger_index").is_none() {
            return;
        }
        // A lookup is its proof and the transaction's bytes, from `tx`.
        let lookup = proof || request["command"] == "tx";
        let mut reply = self.ledger.request(self.connection, message);
        if let (Some(tamper), true) = (self.tamper_lookup, lookup) {
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

    /// A ledger closes. A settlement through the account is near
    /// throughout, so that it anchors on every ledger, as these tests of
    /// what is booked need (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
    fn close(&mut self) {
        self.account.settling(self.now + MAX_SETTLING_MS);
        self.tell();
        self.now += 4_000;
        let close_time = dassie_xrpl::unl::ledger_seconds(self.now);
        for (connection, message) in self.ledger.close_at(close_time) {
            if connection == self.connection {
                self.input(LedgerInput::Message(&message));
            }
        }
    }

    fn closes(&mut self, count: usize) {
        for _ in 0..count {
            self.close();
        }
    }

    /// More ledgers than the history bound close while the account is
    /// away, and its server forgets the ledgers past what it keeps: the
    /// history the account walks has a gap.
    fn away_beyond_the_bound(&mut self) {
        self.account.on_ledger(self.now, LedgerInput::Disconnected);
        for _ in 0..account::HISTORY_BOUND + 300 {
            self.now += 4_000;
            self.ledger
                .close_at(dassie_xrpl::unl::ledger_seconds(self.now));
        }
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

    /// What the books should hold: the balance less the reserve.
    fn bookable(&self) -> u64 {
        (self.ledger.balance(self.address()).unwrap() - RESERVE_BASE) * UNITS_PER_DROP
    }

    /// What the books hold now, as the node books what the scheme said
    /// since it was activated: the owner's payment that applied takes
    /// [`OWNER_AMOUNT`] off them.
    fn books(&self) -> i128 {
        let moved = |effect: &Effect| -> i128 {
            match *effect {
                Effect::Deposit { amount } => i128::from(amount),
                Effect::Withdrawal { amount } | Effect::Fee { amount, .. } => -i128::from(amount),
                Effect::Reconcile { amount, gain, .. } if gain => i128::from(amount),
                Effect::Reconcile { amount, .. } => -i128::from(amount),
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
/// address of another account, funded too.
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
    assert!(!shared.scheme().peers());
    (shared, theirs)
}

/// The owner pays out of the account, and the `xrpl-testnet-xrp` scheme
/// then stops while the payment is on its way; what happens to it then,
/// `meanwhile` makes. The node then restarts: the account from what the
/// witness had it write, the scheme from what it wrote before it stopped.
/// With `short_history`, the server's account history (`account_tx`) no
/// longer reaches back to the payment when it does: a hint the account
/// does not need.
fn stopped_while_paying(
    drop_submits: bool,
    meanwhile: usize,
    short_history: bool,
) -> (Shared, Name) {
    let (mut shared, theirs) = synced();
    shared.drop_submits = drop_submits;
    let id = shared.pay_owner(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    for _ in 0..meanwhile {
        shared.close();
    }
    if short_history {
        shared.ledger.shorten_account_history();
    }
    (restart(shared, checkpoint, &state), id)
}

/// The node restarts: the account from what the witness had it write, the
/// `xrpl-testnet-xrp` scheme from `checkpoint` and `state`.
fn restart(shared: Shared, checkpoint: u64, state: &[u8]) -> Shared {
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.tamper_lookup = shared.tamper_lookup;
    restarted.activate(Some(checkpoint), state);
    restarted.connect();
    restarted.close();
    restarted.close();
    restarted
}

/// The owner pays out of the account, and the books refuse what the
/// `xrpl-testnet-xrp` scheme is told when the ledger validates the payment,
/// or, with `drop_submits`, when the account gives it up: its checkpoint
/// stays as it was, while the witness keeps the account's written. The
/// node then restarts, as after [`stopped_while_paying`].
fn refused_while_paying(drop_submits: bool, short_history: bool) -> (Shared, Name) {
    let (mut shared, theirs) = synced();
    shared.drop_submits = drop_submits;
    let id = shared.pay_owner(theirs);
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
    (restart(shared, checkpoint, &state), id)
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

/// Asserts that the owner's payment `id` was booked once, its hold posted
/// with its fee and never as a withdrawal, and that the books hold the
/// balance.
fn booked_once(shared: &Shared, id: Name) {
    let posted = shared.count(|e| *e == Effect::OwnerPaid { id, applied: true });
    let voided = shared.count(|e| matches!(e, Effect::OwnerPaid { applied: false, .. }));
    let fees = shared.count(|e| matches!(e, Effect::Fee { id: fee, .. } if *fee == id));
    let withdrawals = shared.count(|e| matches!(e, Effect::Withdrawal { .. }));
    assert_eq!(
        (posted, voided, fees, withdrawals),
        (1, 0, 1, 0),
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
    assert_eq!(
        shared.scheme().owner_payment(id),
        OwnerPaymentState::Applied
    );
}

/// Asserts that the owner's payment `id`, which never applied, was voided
/// once, as `outcome`, and nothing booked of it but that.
fn voided_once(shared: &Shared, id: Name, outcome: OwnerPaymentState) {
    let voided = shared.count(|e| *e == Effect::OwnerPaid { id, applied: false });
    let posted = shared.count(|e| matches!(e, Effect::OwnerPaid { applied: true, .. }));
    let fees = shared.count(|e| matches!(e, Effect::Fee { id: fee, .. } if *fee == id));
    assert_eq!((voided, posted, fees), (1, 0, 0), "{:?}", shared.booked);
    assert_eq!(shared.scheme().balance(), shared.bookable());
    assert_eq!(shared.scheme().owner_payment(id), outcome);
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

// Found in review: an owner's payment the ledger validated while the
// `xrpl-testnet-xrp` scheme was stopped, and the other scheme kept the account
// running, is kept for the scheme; after a restart the scheme reads it
// again as the owner's payment, and its hold is posted once: never voided
// as one whose outcome is unknown, nor read as a withdrawal.
#[test]
fn an_owner_payment_validated_while_its_scheme_stopped_is_posted_once() {
    let (shared, id) = stopped_while_paying(false, 2, false);
    booked_once(&shared, id);
}

// Found in review: as above, with a server whose account history no longer
// reaches back to the payment. The account walks the history the proven
// state names, which finds it by its hash: it is posted once with its fee,
// and nothing goes through suspense.
#[test]
fn an_owner_payment_validated_while_its_scheme_stopped_is_posted_once_without_its_history() {
    let (shared, id) = stopped_while_paying(false, 2, true);
    booked_once(&shared, id);
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: an owner's
// payment validated while its scheme was stopped, in history the server no
// longer has beyond the history bound, is one the scheme never reads again:
// the account tells it so, the sync books what the balance lost through
// suspense, and the hold is voided, what became of it unknown. Across
// another restart, nothing of it is booked again.
#[test]
fn an_owner_payment_never_read_is_in_the_balance() {
    let (mut shared, theirs) = synced();
    shared.closes(260);
    let id = shared.pay_owner(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    shared.close();
    shared.close();
    shared.away_beyond_the_bound();
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.activate(Some(checkpoint), &state);
    restarted.drop_lookups = true;
    restarted.connect();
    assert_eq!(restarted.account.unproven().history_gaps, 1);
    voided_once(&restarted, id, OwnerPaymentState::Unknown);
    assert_eq!(restarted.books(), i128::from(restarted.bookable()));
    let (state, checkpoint) = (restarted.scheme().state(), restarted.scheme().balance());
    let kept = restarted.account.state();
    let mut again = Shared::new(restarted.ledger, &kept, None);
    again.activate(Some(checkpoint), &state);
    again.connect();
    again.close();
    again.close();
    let told = again.count(|e| match e {
        Effect::OwnerPaid { id: paid, .. } | Effect::Fee { id: paid, .. } => *paid == id,
        _ => false,
    });
    assert_eq!(told, 0, "{:?}", again.booked);
    assert_eq!(again.scheme().balance(), again.bookable());
    assert_eq!(
        again.books(),
        i128::from(again.bookable()),
        "{:?}",
        again.booked
    );
}

// spec/areas/settlement.md#external-assets: the owner's payment that can
// never apply, found so while its scheme was stopped, is cancelled after a
// restart, its hold voided, rather than left on its way for good.
#[test]
fn an_owner_payment_given_up_while_its_scheme_stopped_is_cancelled() {
    let (shared, id) = stopped_while_paying(true, 25, false);
    voided_once(&shared, id, OwnerPaymentState::Cancelled);
}

// Found in review: the books refuse what a scheme is told with the owner's
// payment, validated, and stop it. The account no longer has it in flight,
// and its state is written on for the other scheme on it, while the
// scheme's checkpoint stays from before. The account keeps the payment for
// the scheme, as for one not on the account, so after a restart the scheme
// reads it again as its own: posted once with its fee, never also booked
// as a withdrawal.
#[test]
fn an_owner_payment_told_in_a_refused_batch_is_booked_once() {
    let (shared, id) = refused_while_paying(false, false);
    booked_once(&shared, id);
}

// As above, with a server whose account history no longer reaches back to
// the payment: a hint the account does not need, as the history the proven
// state names finds it, and it is booked once.
#[test]
fn an_owner_payment_told_in_a_refused_batch_is_booked_once_without_its_history() {
    let (shared, id) = refused_while_paying(false, true);
    booked_once(&shared, id);
}

// As above, with the books refusing what the scheme is told after the
// restart: told again after the next restart, it still books the payment
// once.
#[test]
fn an_owner_payment_refused_twice_is_booked_once() {
    let (mut shared, theirs) = synced();
    let id = shared.pay_owner(theirs);
    let (state, checkpoint) = (shared.scheme().state(), shared.scheme().balance());
    shared.stop();
    shared.close();
    shared.close();
    shared.ledger.shorten_account_history();
    let kept = shared.account.state();
    let mut restarted = Shared::new(shared.ledger, &kept, None);
    restarted.activate(Some(checkpoint), &state);
    restarted.refuse_own = true;
    restarted.connect();
    assert!(
        restarted.stopped,
        "the books refused what the scheme was told"
    );
    restarted.close();
    let again = restart(restarted, checkpoint, &state);
    booked_once(&again, id);
}

// The owner's payment given up while the books refuse what its scheme is
// told of it is cancelled after a restart, its hold voided.
#[test]
fn an_owner_payment_given_up_in_a_refused_batch_is_cancelled() {
    let (shared, id) = refused_while_paying(true, false);
    voided_once(&shared, id, OwnerPaymentState::Cancelled);
}
