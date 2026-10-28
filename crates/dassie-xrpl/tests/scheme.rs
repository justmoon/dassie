//! The `xrpl-testnet-xrp` scheme against the fake ledger: each rule of how
//! the owner's book of the account's XRP books transactions
//! (spec/areas/xrpl.md#following-the-account), pays the owner's payments
//! (spec/areas/xrpl.md#deposits-and-withdrawals), restarts and catches up,
//! with the books checked against the ledger after every step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_ledger::Name;
use dassie_settlement::{
    ExternalAsset, LedgerInput, LedgerStatus, OwnerPayment, OwnerPaymentState, ReconcileCause,
    SchemeError,
};
use dassie_xrpl::account::{Doubt, HISTORY_BOUND, STALL_MS};
use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger, Lie, RESERVE_BASE};
use support::{Effect, XrplScheme};

mod support;
use dassie_xrpl::{AccountId, UNITS_PER_DROP};

/// What the node would book for a scheme, in internal units.
#[derive(Debug, Default)]
struct Books {
    /// `assets/settlement`, as a debit balance.
    settlement: i128,
    owner: i128,
    suspense: i128,
    fees: i128,
    /// Settlements cancelled: ones a restart left pending.
    cancelled: Vec<Name>,
    /// Settlements signed before the XRPL schemes stopped peering, held
    /// pending as the node holds them: by ID, what each pays.
    settling: std::collections::BTreeMap<Name, u64>,
    /// Those of them posted, in order.
    finalized: Vec<Name>,
    /// Each reconciliation, in order: what the books gained, and why.
    reconciled: Vec<(i128, ReconcileCause)>,
    /// The transaction of each fee, in order.
    fee_ids: Vec<Name>,
    /// The owner's payments held while on their way, as the node holds
    /// them: by ID, what each pays.
    held: std::collections::BTreeMap<Name, u64>,
    /// The owner's payments no longer on their way, in order, and whether
    /// each applied.
    paid: Vec<(Name, bool)>,
}

struct Node {
    name: Name,
    scheme: XrplScheme,
    connection: u64,
    online: bool,
    books: Books,
    /// Submissions the network loses.
    drop_submits: bool,
    /// Requests of this command go unanswered, and are counted.
    hold: Option<&'static str>,
    held: usize,
    /// The last request held.
    held_request: Option<serde_json::Value>,
    /// The transactions it asked the proofs of, in order.
    proved: Vec<String>,
    /// Whether its books may lag the ledger, as while its server lies.
    lagging: bool,
    /// A server behind the others, stopped where the ledger once was: the
    /// node's requests go to it while it is set.
    behind: Option<FakeLedger>,
    reconnects: usize,
    /// The commands it sent the ledger, in order.
    asked: Vec<String>,
}

struct World {
    ledger: FakeLedger,
    nodes: Vec<Node>,
    now: u64,
    next_connection: u64,
}

fn entropy(seed: u8) -> [u8; 32] {
    [seed; 32]
}

fn name(text: &str) -> Name {
    Name::new(text).unwrap()
}

impl World {
    fn new(count: u8) -> Self {
        Self::on(FakeLedger::new(), count)
    }

    /// `count` nodes, connected to `ledger`.
    fn on(ledger: FakeLedger, count: u8) -> Self {
        // The node's clock reads the ledger's validations as fresh.
        let now = ledger.now_ms();
        let mut world = Self {
            ledger,
            nodes: Vec::new(),
            now,
            next_connection: 1,
        };
        for seed in 0..count {
            let node = name(&format!("node{seed}"));
            let scheme = XrplScheme::new("", &entropy(seed + 1), None, &[]).unwrap();
            world.nodes.push(Node {
                name: node,
                scheme,
                connection: 0,
                online: false,
                books: Books::default(),
                drop_submits: false,
                hold: None,
                held: 0,
                held_request: None,
                proved: Vec::new(),
                lagging: false,
                behind: None,
                reconnects: 0,
                asked: Vec::new(),
            });
        }
        for index in 0..world.nodes.len() {
            world.connect(index);
        }
        world
    }

    fn account(&self, index: usize) -> AccountId {
        self.nodes[index].scheme.account()
    }

    fn connect(&mut self, index: usize) {
        self.next_connection += 1;
        self.nodes[index].connection = self.next_connection;
        self.nodes[index].online = true;
        self.input(index, LedgerInput::Connected);
    }

    fn disconnect(&mut self, index: usize) {
        let connection = self.nodes[index].connection;
        self.ledger.disconnect(connection);
        self.nodes[index].online = false;
        self.input(index, LedgerInput::Disconnected);
    }

    fn input(&mut self, index: usize, input: LedgerInput<'_>) {
        let mut effects = Vec::new();
        self.nodes[index]
            .scheme
            .on_ledger(self.now, input, &mut effects);
        self.nodes[index].scheme.check_invariants();
        self.apply(index, effects);
        self.nodes[index].scheme.check_invariants();
    }

    /// Sends a request to the ledger, and its answer back, unless the
    /// node is offline or the network loses it.
    fn request(&mut self, index: usize, message: &[u8]) {
        let node = &mut self.nodes[index];
        if !node.online {
            return;
        }
        let request: serde_json::Value = serde_json::from_slice(message).unwrap();
        let command = request["command"].as_str().unwrap_or_default();
        node.asked.push(command.to_string());
        if let Some(hash) = request["tx_hash"].as_str() {
            node.proved.push(hash.to_owned());
        }
        // A transaction's bytes are read in binary, only to build the leaf
        // of a proof asked for (spec/designs/xrpl-validation.md#4-what-is-proven-and-how).
        if command == "tx" {
            assert_eq!(request["binary"], true, "{request}");
            let hash = request["transaction"].as_str().unwrap();
            assert!(node.proved.iter().any(|proved| proved == hash), "{request}");
        }
        if command == "submit" && node.drop_submits {
            return;
        }
        if node.hold == Some(command) {
            node.held += 1;
            node.held_request = Some(request);
            return;
        }
        let connection = node.connection;
        let reply = match &mut node.behind {
            Some(behind) => behind.request(connection, message),
            None => self.ledger.request(connection, message),
        };
        self.input(index, LedgerInput::Message(&reply));
    }

    /// Books the effects as the node would, and carries out the rest.
    fn apply(&mut self, index: usize, effects: Vec<Effect>) {
        for effect in effects {
            let node = &mut self.nodes[index];
            let books = &mut node.books;
            match effect {
                Effect::Deposit { amount } => {
                    books.settlement += i128::from(amount);
                    books.owner += i128::from(amount);
                }
                Effect::Withdrawal { amount } => {
                    books.settlement -= i128::from(amount);
                    books.owner -= i128::from(amount);
                }
                Effect::OwnerPaid { id, applied } => {
                    let amount = books.held.remove(&id).unwrap();
                    books.paid.push((id, applied));
                    if applied {
                        books.settlement -= i128::from(amount);
                        books.owner -= i128::from(amount);
                    }
                }
                Effect::Reconcile {
                    amount,
                    gain,
                    cause,
                } => {
                    let amount = if gain {
                        i128::from(amount)
                    } else {
                        -i128::from(amount)
                    };
                    books.settlement += amount;
                    books.suspense += amount;
                    books.reconciled.push((amount, cause));
                }
                Effect::Fee { amount, id } => {
                    books.settlement -= i128::from(amount);
                    books.fees += i128::from(amount);
                    books.fee_ids.push(id);
                }
                Effect::Cancel { settlement } => {
                    books.settling.remove(&settlement);
                    books.cancelled.push(settlement);
                }
                Effect::Finalize { settlement } => {
                    let amount = books.settling.remove(&settlement).unwrap();
                    books.settlement -= i128::from(amount);
                    books.finalized.push(settlement);
                }
                Effect::LedgerSend { message } => self.request(index, &message),
                Effect::Reconnect => {
                    node.reconnects += 1;
                    self.disconnect(index);
                    self.connect(index);
                }
                Effect::Fund { url, body } => {
                    assert!(url.starts_with("https://"));
                    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let to = AccountId::parse(request["destination"].as_str().unwrap()).unwrap();
                    self.ledger.fund(to);
                    self.input(index, LedgerInput::Funded(true));
                }
                Effect::Fetch { url } => self.fetch(index, &url),
            }
        }
    }

    /// Node `index` fetches `url`: the fake's validator list, from its
    /// site.
    fn fetch(&mut self, index: usize, url: &str) {
        let body = self.ledger.fetch(url);
        let fetched = LedgerInput::Fetched {
            url,
            body: body.as_deref(),
        };
        self.input(index, fetched);
    }

    /// Closes a ledger, and delivers the streams.
    fn close(&mut self) {
        self.now += 4_000;
        let close_time = dassie_xrpl::unl::ledger_seconds(self.now);
        for (connection, message) in self.ledger.close_at(close_time) {
            if let Some(index) = self
                .nodes
                .iter()
                .position(|n| n.online && n.connection == connection)
            {
                self.input(index, LedgerInput::Message(&message));
            }
        }
        self.timers();
        self.check();
    }

    /// Runs the nodes' timers that are due.
    fn timers(&mut self) {
        for index in 0..self.nodes.len() {
            if self.nodes[index]
                .scheme
                .deadline()
                .is_some_and(|at| at <= self.now)
            {
                let mut effects = Vec::new();
                self.nodes[index].scheme.on_timer(self.now, &mut effects);
                self.apply(index, effects);
            }
        }
    }

    fn closes(&mut self, count: usize) {
        for _ in 0..count {
            self.close();
        }
    }

    /// Time passes, by `ms`, without a ledger closing.
    fn wait(&mut self, ms: u64) {
        self.now += ms;
        self.timers();
        self.check();
    }

    /// `count` ledgers close while no node reads them, as fast as the fake
    /// closes them.
    fn pass(&mut self, count: u32) {
        for _ in 0..count {
            self.now += 4_000;
            self.ledger
                .close_at(dassie_xrpl::unl::ledger_seconds(self.now));
        }
    }

    /// The books of every synced node match its account: its balance less
    /// the reserve. And the ledger made and lost no XRP.
    fn check(&self) {
        for node in &self.nodes {
            assert_eq!(
                node.books.settlement,
                i128::from(node.scheme.balance()),
                "{}",
                node.name
            );
            // A reset the scheme has not seen yet is not its to know, nor
            // what its lying server keeps from it.
            if node.scheme.status() == LedgerStatus::Synced
                && !node.lagging
                && let Some(balance) = self.ledger.balance(node.scheme.account())
            {
                let bookable = (balance * UNITS_PER_DROP).saturating_sub(node.scheme.reserve());
                assert_eq!(node.scheme.balance(), bookable, "{}", node.name);
            }
        }
        assert_eq!(self.ledger.total(), dassie_xrpl::MAX_DROPS);
    }

    /// Node `index`'s owner pays `payment` out of the account, which the
    /// node holds against the owner's funds while it is on its way.
    fn pay_owner(
        &mut self,
        index: usize,
        payment: &OwnerPayment,
        effects: &mut Vec<Effect>,
    ) -> Result<Name, SchemeError> {
        let id = self.nodes[index].scheme.pay_owner(payment, effects)?;
        self.nodes[index]
            .books
            .held
            .insert(id, payment.amount / UNITS_PER_DROP * UNITS_PER_DROP);
        Ok(id)
    }

    /// Node `from`'s owner pays `amount` to node `to`'s account, and it is
    /// submitted. Returns the payment's ID.
    fn pay(&mut self, from: usize, to: usize, amount: u64) -> Name {
        let payment = OwnerPayment {
            destination: self.account(to).to_address(),
            tag: None,
            memo: None,
            amount,
        };
        let mut effects = Vec::new();
        let id = self.pay_owner(from, &payment, &mut effects).unwrap();
        self.apply(from, effects);
        id
    }

    /// What the node keeps of node `index`'s scheme and account: their
    /// state, and the balance it checkpointed.
    fn kept(&self, index: usize) -> (Vec<u8>, u64) {
        (
            self.nodes[index].scheme.state(),
            self.nodes[index].scheme.balance(),
        )
    }

    /// Node `index` restarts: its scheme and account made again from what
    /// the node kept.
    fn restore(&mut self, index: usize, (state, checkpoint): (Vec<u8>, u64)) {
        let seed = u8::try_from(index + 1).unwrap();
        let mut scheme = XrplScheme::new("", &entropy(seed), Some(checkpoint), &state).unwrap();
        let started = std::mem::take(&mut scheme.started);
        self.nodes[index].scheme = scheme;
        self.apply(index, started);
    }

    /// What became of node `index`'s owner's payment `id`.
    fn owner_payment(&self, index: usize, id: Name) -> OwnerPaymentState {
        self.nodes[index].scheme.owner_payment(id)
    }
}

/// Two funded, synced nodes.
fn funded() -> World {
    let mut world = World::new(2);
    world.closes(2);
    for node in &world.nodes {
        assert_eq!(node.scheme.status(), LedgerStatus::Synced, "{}", node.name);
    }
    world
}

const FUNDED: u64 = (FAUCET_DROPS - RESERVE_BASE) * UNITS_PER_DROP;
const FEE: i128 = 10 * UNITS_PER_DROP as i128;

// An account the faucet creates is a deposit, the reserve
// left out.
#[test]
fn a_new_account_is_funded_and_its_reserve_never_booked() {
    let world = funded();
    for node in &world.nodes {
        assert_eq!(node.books.owner, i128::from(FUNDED));
        assert_eq!(node.scheme.reserve(), RESERVE_BASE * UNITS_PER_DROP);
        assert!(node.scheme.deposit_address().unwrap().starts_with('r'));
    }
}

// spec/areas/xrpl.md#owner-books: the scheme is an owner book: it peers
// with no one, and its config cannot say otherwise.
#[test]
fn the_scheme_peers_with_no_one() {
    for config in [r#"{"peers": true}"#, r#"{"peers": false}"#] {
        assert!(dassie_xrpl::scheme::SchemeConfig::parse(config).is_err());
    }
    let world = funded();
    for node in &world.nodes {
        assert!(!node.scheme.peers(), "{}", node.name);
    }
}

// spec/areas/xrpl.md#following-the-account: a Payment with the memo a
// settlement carried while the XRPL schemes peered, naming a node, is the
// owner's deposit like any other: nothing is a settlement.
#[test]
fn a_payment_with_a_settlement_memo_is_a_deposit() {
    let mut world = funded();
    let payment = OwnerPayment {
        destination: world.account(0).to_address(),
        tag: None,
        memo: Some(dassie_settlement::OwnerMemo {
            memo_type: b"https://dassie.land/settlement/xrpl-testnet-xrp/v1".to_vec(),
            data: b"node1".to_vec(),
        }),
        amount: 1_000_000_000,
    };
    let mut effects = Vec::new();
    let id = world.pay_owner(1, &payment, &mut effects).unwrap();
    world.apply(1, effects);
    world.close();
    assert_eq!(world.owner_payment(1, id), OwnerPaymentState::Applied);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED) + 1_000_000_000);
    assert_eq!(books.suspense, 0);
}

// An owner's payment signed and kept before a restart is submitted again
// after it, and booked once; what happened while away is booked once, as
// what it was.
#[test]
fn a_restart_resubmits_and_catches_up() {
    let mut world = funded();
    world.nodes[0].drop_submits = true;
    let id = world.pay(0, 1, 3_000_000_000);
    // Node 0 goes down with its payment signed; its state and balance are
    // what the node kept.
    let kept = world.kept(0);
    world.disconnect(0);
    // Meanwhile node 1's owner pays node 0, and someone deposits.
    world.pay(1, 0, 4_000_000_000);
    world.ledger.deposit(world.account(0), 7_000_000);
    world.closes(2);
    world.restore(0, kept);
    world.nodes[0].drop_submits = false;
    world.connect(0);
    world.closes(2);
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::Applied);
    let books = &world.nodes[0].books;
    assert_eq!(books.paid, [(id, true)]);
    assert_eq!(
        books.owner,
        i128::from(FUNDED) - 3_000_000_000 + 4_000_000_000 + 7_000_000_000
    );
    assert_eq!(books.fees, FEE);
    assert_eq!(books.suspense, 0);
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: a server that no
// longer has a ledger within the history bound, which the proven state
// names, keeps too short a history: the account waits, says so, and books
// nothing it cannot prove.
#[test]
fn a_short_history_within_the_bound_stalls() {
    let mut world = funded();
    world.disconnect(0);
    world.pay(1, 0, 4_000_000_000);
    world.closes(3);
    world.ledger.forget_history();
    world.connect(0);
    world.closes(1);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED));
    assert!(books.reconciled.is_empty());
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
    let waiting = world.nodes[0].scheme.account.waiting().unwrap();
    assert!(waiting.contains("history is too short"), "{waiting}");
}

/// Two funded, synced nodes, the second of which pays `amount` to the first
/// while it is away for longer than the history bound, past the first flag
/// ledger. Its server has forgotten the payment's ledger.
fn away_beyond_the_bound(amount: u64) -> World {
    let mut world = funded();
    world.closes(260);
    world.disconnect(0);
    world.pay(1, 0, amount);
    world.closes(3);
    world.disconnect(1);
    world.pass(HISTORY_BOUND + 300);
    world
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: history the
// server no longer has, beyond the history bound as the proven state
// names it, is history it does not have: the balance says what changed,
// reconciled through suspense, exactly, and the gap is counted.
#[test]
fn history_beyond_the_bound_reconciles_by_the_balance() {
    let mut world = away_beyond_the_bound(4_000_000_000);
    world.connect(0);
    world.closes(1);
    assert_eq!(world.nodes[0].scheme.account.unproven().history_gaps, 1);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    assert_eq!(world.nodes[0].books.suspense, 4_000_000_000);
    assert_eq!(world.nodes[0].books.owner, i128::from(FUNDED));
    // Nothing the scheme can name: a warning.
    assert_eq!(
        world.nodes[0].books.reconciled,
        [(4_000_000_000, ReconcileCause::Unexplained)]
    );
}

// A reserve change moves what is spendable: reconciled at once, as the
// reserve's, which is expected.
#[test]
fn a_new_reserve_is_reconciled() {
    let mut world = funded();
    assert!(world.nodes[0].books.reconciled.is_empty());
    world.ledger.set_reserve(2 * RESERVE_BASE, 0);
    world.close();
    let reserve = i128::from(RESERVE_BASE * UNITS_PER_DROP);
    assert_eq!(world.nodes[0].books.suspense, -reserve);
    assert_eq!(
        world.nodes[0].books.reconciled,
        [(-reserve, ReconcileCause::Reserve)]
    );
}

// A reserve above the balance explains what the books hold, and no more:
// they never hold less than nothing. Lowered again, it explains what comes
// back.
#[test]
fn a_reserve_above_the_balance_explains_down_to_nothing() {
    let mut world = funded();
    let held = i128::from(world.nodes[0].scheme.balance());
    assert!(held > 0);
    world.ledger.set_reserve(2 * FAUCET_DROPS, 0);
    world.close();
    assert_eq!(world.nodes[0].scheme.balance(), 0);
    world.ledger.set_reserve(RESERVE_BASE, 0);
    world.close();
    assert_eq!(
        world.nodes[0].books.reconciled,
        [
            (-held, ReconcileCause::Reserve),
            (held, ReconcileCause::Reserve)
        ]
    );
}

// A reserve change and a difference the scheme cannot name, met in one
// match: each is reconciled for its own part, and together they make the
// books match the balance.
#[test]
fn a_reserve_change_is_told_from_an_unexplained_difference() {
    let mut world = away_beyond_the_bound(4_000_000_000);
    world.ledger.set_reserve(2 * RESERVE_BASE, 0);
    world.close();
    world.connect(0);
    world.closes(1);
    let reserve = i128::from(RESERVE_BASE * UNITS_PER_DROP);
    let books = &world.nodes[0].books;
    assert_eq!(books.suspense, 4_000_000_000 - reserve);
    let mut reconciled = books.reconciled.clone();
    reconciled.sort_by_key(|(amount, _)| *amount);
    assert_eq!(
        reconciled,
        [
            (-reserve, ReconcileCause::Reserve),
            (4_000_000_000, ReconcileCause::Unexplained)
        ]
    );
}

// A testnet reset takes the funds: booked as a loss, and the faucet asked
// again, whose funds are a deposit.
#[test]
fn a_testnet_reset_is_a_loss_and_a_new_deposit() {
    let mut world = funded();
    world.ledger.reset();
    world.disconnect(0);
    world.connect(0);
    // The faucet is asked at most once a minute.
    world.closes(20);
    let books = &world.nodes[0].books;
    assert_eq!(books.suspense, -i128::from(FUNDED));
    assert_eq!(
        books.reconciled,
        [(-i128::from(FUNDED), ReconcileCause::Unexplained)]
    );
    assert_eq!(books.owner, 2 * i128::from(FUNDED));
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// The owner may ask the faucet for more, at its pace: what it sends is a
// deposit once the ledger shows it, never booked at the asking.
#[test]
fn the_owner_asks_the_faucet_for_more() {
    let mut world = funded();
    assert!(world.nodes[0].scheme.has_faucet());
    let ask = |world: &mut World| {
        let mut effects = Vec::new();
        let asked = world.nodes[0].scheme.ask_faucet(&mut effects);
        world.nodes[0].scheme.check_invariants();
        (asked, effects)
    };
    // Asked when the account was made, less than a minute ago.
    assert_eq!(ask(&mut world), (Err(SchemeError::Unavailable), Vec::new()));
    world.closes(15);
    let (asked, effects) = ask(&mut world);
    assert_eq!(asked, Ok(()));
    assert!(matches!(effects.as_slice(), [Effect::Fund { .. }]));
    assert_eq!(world.nodes[0].books.owner, i128::from(FUNDED));
    world.apply(0, effects);
    assert_eq!(ask(&mut world).0, Err(SchemeError::Unavailable));
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(
        books.owner,
        i128::from(FUNDED) + i128::from(FAUCET_DROPS * UNITS_PER_DROP)
    );
    assert_eq!(books.suspense, 0);
    // Not while there is no connection to take its answer on.
    world.closes(15);
    world.disconnect(0);
    assert_eq!(ask(&mut world).0, Err(SchemeError::Unavailable));
}

#[test]
fn a_ledger_without_a_faucet_is_never_asked() {
    let mut scheme = XrplScheme::new(r#"{"faucet":null}"#, &entropy(1), None, &[]).unwrap();
    assert!(!scheme.has_faucet());
    let mut effects = Vec::new();
    assert_eq!(
        scheme.ask_faucet(&mut effects),
        Err(SchemeError::Unsupported)
    );
    assert!(effects.is_empty());
}

// The scheme has synced once its first sync booked what the account held
// as the owner's, from when on it tells a deposit from what it held. That
// is kept with its state, so a restart that cannot reach the ledger keeps
// it.
#[test]
fn the_first_sync_is_kept_across_restarts() {
    let fresh = XrplScheme::new("", &entropy(1), None, &[]).unwrap();
    assert!(!fresh.has_synced(), "synced before it saw its ledger");
    let mut world = World::new(1);
    world.closes(2);
    assert!(world.nodes[0].scheme.has_synced(), "never synced");
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let restarted = XrplScheme::new("", &entropy(1), Some(checkpoint), &state).unwrap();
    assert_eq!(restarted.status(), LedgerStatus::Connecting);
    assert!(restarted.has_synced(), "a restart forgot the first sync");
}

// spec/decisions/ledger-resets.md: a reset starts the chain again from a
// ledger below the newest seen, which closed later. It is recognised as one
// even when the account exists again on the new chain: the old funds are a
// loss, the new a deposit.
#[test]
fn a_reset_is_recognised_when_the_account_exists_again() {
    let mut world = funded();
    world.disconnect(0);
    world.ledger.reset();
    world.ledger.fund(world.account(0));
    world.close();
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.suspense, -i128::from(FUNDED));
    assert_eq!(books.owner, 2 * i128::from(FUNDED));
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// Found in review: after a reconnect, a server says the account does not
// exist, while the owner's payment applied. An absence counts only when
// proven: nothing is cancelled or written off, and once the server is
// honest the payment is booked.
#[test]
fn an_absence_the_server_made_up_cancels_nothing() {
    let mut world = funded();
    let id = world.pay(0, 1, 5_000_000_000);
    world.disconnect(0);
    world.close();
    assert_eq!(world.owner_payment(1, id), OwnerPaymentState::Unknown);
    let write_offs = world.nodes[0].scheme.account.write_offs();
    world.ledger.set_lie_for_all(Some(Lie::FalseAbsence));
    world.connect(0);
    world.closes(2);
    world.ledger.set_lie_for_all(None);
    assert!(world.ledger.lies_told(Lie::FalseAbsence) > 0);
    let books = &world.nodes[0].books;
    assert!(books.paid.is_empty() && books.reconciled.is_empty());
    assert_eq!(world.nodes[0].scheme.account.write_offs(), write_offs);
    world.wait(STALL_MS);
    world.closes(2);
    assert_eq!(world.nodes[0].books.paid, [(id, true)]);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// As above, against a server that stopped at the first ledger, before the
// account was made: it is never believed, and the scheme waits. Once a
// server has caught up, the payment is booked.
#[test]
fn a_server_that_never_saw_the_account_writes_nothing_off() {
    let mut world = funded();
    let id = world.pay(0, 1, 5_000_000_000);
    world.disconnect(0);
    world.close();
    world.nodes[0].behind = Some(FakeLedger::new());
    world.connect(0);
    // Long enough to reconnect when the stopped server goes silent.
    world.closes(10);
    assert!(world.nodes[0].reconnects > 0);
    let books = &world.nodes[0].books;
    assert!(books.paid.is_empty());
    assert_eq!(books.suspense, 0);
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::InFlight);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
    world.nodes[0].behind = None;
    world.disconnect(0);
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.paid, [(id, true)]);
    assert_eq!(books.suspense, 0);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// spec/decisions/ledger-resets.md: a server behind what the scheme booked
// (here a deposit), or only behind the newest ledger it saw, says nothing
// new: nothing is booked from it, and the scheme waits for one that caught
// up.
#[test]
fn a_server_behind_is_never_booked_from() {
    for paid in [true, false] {
        let mut world = funded();
        let behind = world.ledger.clone();
        if paid {
            world.pay(1, 0, 4_000_000_000);
        }
        world.closes(3);
        let books = |world: &World| {
            let books = &world.nodes[0].books;
            (books.settlement, books.owner, books.suspense)
        };
        let before = books(&world);
        world.disconnect(0);
        world.nodes[0].behind = Some(behind);
        world.connect(0);
        world.closes(3);
        assert_eq!(books(&world), before, "{paid}");
        assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
        world.nodes[0].behind = None;
        world.disconnect(0);
        world.connect(0);
        world.closes(2);
        assert_eq!(books(&world), before, "{paid}");
        assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    }
}

// The owner's payment waits while the account cannot make it: beyond what
// it holds above the reserve, or before it syncs again.
#[test]
fn an_owners_payment_waits_when_it_cannot_be_made() {
    let mut world = funded();
    let to = world.account(1).to_address();
    let payment = |amount| OwnerPayment {
        destination: to.clone(),
        tag: None,
        memo: None,
        amount,
    };
    assert_eq!(
        world.pay_owner(0, &payment(FUNDED + UNITS_PER_DROP), &mut Vec::new()),
        Err(SchemeError::Unavailable)
    );
    world.disconnect(0);
    assert_eq!(
        world.pay_owner(0, &payment(1_000_000), &mut Vec::new()),
        Err(SchemeError::Unavailable)
    );
}

// Found in review: the owner's payment that the ledger validated while the
// node was away, with an account history too short to read it from, is
// still booked, once, as applied, with its fee: nothing goes through
// suspense.
#[test]
fn an_owners_payment_validated_while_away_is_still_booked() {
    let mut world = funded();
    let id = world.pay(0, 1, 3_000_000_000);
    let kept = world.kept(0);
    world.disconnect(0);
    world.closes(2);
    world.ledger.shorten_account_history();
    world.restore(0, kept);
    world.connect(0);
    world.closes(40);
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::Applied);
    let books = &world.nodes[0].books;
    assert_eq!(books.paid, [(id, true)]);
    assert!(books.held.is_empty());
    assert_eq!(books.owner, i128::from(FUNDED) - 3_000_000_000);
    assert_eq!(books.fees, FEE);
    assert_eq!(books.suspense, 0);
}

// Found in review: a connection that goes quiet is made anew, rather than
// leaving the scheme synced to a ledger it no longer hears.
#[test]
fn a_silent_connection_is_made_anew() {
    let mut world = funded();
    // The ledger forgets the connection; the scheme is not told.
    let connection = world.nodes[0].connection;
    world.ledger.disconnect(connection);
    world.nodes[0].online = false;
    world.now += 31_000;
    let mut effects = Vec::new();
    world.nodes[0].scheme.on_timer(world.now, &mut effects);
    assert!(effects.contains(&Effect::Reconnect), "{effects:?}");
    world.apply(0, effects);
    world.closes(2);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// Found in review: after a testnet reset, what was in flight can never
// apply: it is cancelled, and paying starts anew.
#[test]
fn a_reset_cancels_what_was_in_flight() {
    let mut world = funded();
    world.nodes[0].drop_submits = true;
    let id = world.pay(0, 1, 3_000_000_000);
    world.ledger.reset();
    world.disconnect(0);
    world.connect(0);
    world.closes(20);
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::Cancelled);
    world.nodes[0].drop_submits = false;
    // The faucet made the account again, on the new chain.
    world.ledger.fund(world.account(1));
    world.closes(2);
    let next = world.pay(0, 1, 1_000_000_000);
    world.close();
    assert_eq!(world.owner_payment(0, next), OwnerPaymentState::Applied);
}

// spec/areas/xrpl.md#state-and-restarts: a settlement a node still holds
// pending after a restart, signed before the XRPL schemes stopped peering,
// is cancelled: none is booked as a settlement any more.
#[test]
fn a_settlement_left_pending_is_cancelled() {
    let mut world = funded();
    let kept = world.kept(0);
    world.disconnect(0);
    world.restore(0, kept);
    world.connect(0);
    let id = Name::hex(&[7; 32], true);
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(id, 1_000_000_000)], &mut effects);
    assert_eq!(effects, [Effect::Cancel { settlement: id }]);
    world.apply(0, effects);
    world.closes(2);
    assert_eq!(world.nodes[0].books.cancelled, [id]);
    assert_eq!(world.nodes[0].books.owner, i128::from(FUNDED));
}

/// Node 0 signs, as the XRPL schemes did to settle, a Payment of `drops`
/// to node 1 that is not the owner's, and executes it if `executed`, and
/// restarts: the node holds the settlement pending, and hands it to the
/// scheme. Returns the settlement's ID and what the scheme asked.
fn left_pending(world: &mut World, drops: u64, executed: bool) -> (Name, Vec<Effect>) {
    let scheme = dassie_ledger::name!(dassie_xrpl::scheme::SCHEME_ID);
    let to = world.account(1);
    let account = &mut world.nodes[0].scheme.account;
    let hash = account
        .sign_payment_to(scheme, (to, None), drops, Vec::new())
        .unwrap();
    let id = Name::hex(&hash, true);
    world.nodes[0]
        .books
        .settling
        .insert(id, drops * UNITS_PER_DROP);
    // Its submission is lost with the restart.
    if executed {
        assert!(world.nodes[0].scheme.account.execute(scheme, hash));
    }
    let kept = world.kept(0);
    world.disconnect(0);
    world.restore(0, kept);
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(id, drops * UNITS_PER_DROP)], &mut effects);
    (id, effects)
}

// spec/areas/xrpl.md#state-and-restarts: a settlement signed before the
// XRPL schemes stopped peering, its Payment executed and in flight at the
// restart, stays pending until the ledger says what became of it: it may
// still apply, and cancelling one that applied would pay twice. Once
// validated, it is posted, with its fee, never booked as the owner's
// withdrawal.
#[test]
fn a_settlement_in_flight_at_the_restart_waits_for_the_ledger() {
    let mut world = funded();
    let (id, effects) = left_pending(&mut world, 2_000, true);
    assert!(
        effects
            .iter()
            .all(|effect| !matches!(effect, Effect::Cancel { .. }))
    );
    world.apply(0, effects);
    world.connect(0);
    world.closes(3);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert!(books.cancelled.is_empty(), "{:?}", books.cancelled);
    assert_eq!(books.fee_ids, [id]);
    assert_eq!(books.owner, i128::from(FUNDED), "no withdrawal");
    assert!(books.reconciled.is_empty(), "{:?}", books.reconciled);
    world.check();
}

// spec/areas/xrpl.md#state-and-restarts: one signed and never executed was
// never submitted: it is given up, and cancelled at once.
#[test]
fn a_settlement_never_executed_is_cancelled_at_the_restart() {
    let mut world = funded();
    let (id, effects) = left_pending(&mut world, 2_000, false);
    assert_eq!(effects, [Effect::Cancel { settlement: id }]);
    world.apply(0, effects);
    world.connect(0);
    world.closes(3);
    let books = &world.nodes[0].books;
    assert!(books.finalized.is_empty() && books.settling.is_empty());
    assert_eq!(books.owner, i128::from(FUNDED));
    world.check();
    // Its sequence is taken by the next.
    let next = world.pay(0, 1, 1_000_000_000);
    world.close();
    assert_eq!(world.owner_payment(0, next), OwnerPaymentState::Applied);
}

/// What node 1 paid node 0 to enter, in internal units: 0.1 XRP.
const ENTRY_PAID: u64 = 100_000_000;

/// Node 0's scheme state, as the scheme wrote it while the XRPL schemes
/// peered (version 1), in a world of three nodes once node 2 had peered
/// with it and node 1 had paid it [`ENTRY_PAID`] to enter, validated at
/// ledger 5000003, index 0, and held off its books.
const PEERING_STATE: [&str; 17] = [
    // The version.
    "01",
    // The cursor: present, at ledger 5000003, index 0.
    "FF",
    "004C4B43",
    "00000000",
    // The count of write-offs.
    "0000000000000001",
    // The peers' addresses: node2's.
    "00000001",
    "056E6F646532",
    "F59A8039C40BF1A6ADF7CEC4374C6449CBD0F114",
    // The settlements looked up after a restart: none.
    "00000000",
    // The entry payments held: node1's, from its account, of
    // ENTRY_PAID, its hash and the ledger its hold ran to.
    "00000001",
    "056E6F646531",
    "12EF6422DC22833EF13442F83E89FB113C01A65A",
    "0000000005F5E100",
    "C0C37ED66D6059BCB2DAABF06E40DC2CBFECFC708B62EA7CBDD7C97D9FC28C77",
    "004C4EC7",
    // No owner's payment on its way, and none done.
    "00",
    "00",
];

// spec/areas/xrpl.md#state-and-restarts: a scheme reads the state it wrote
// while it peered. It drops the peers' addresses, and books the entry
// payments it held as the owner's deposit, once, as it did with a hold
// that ran out: the books then match the account, nothing booked twice
// and nothing lost, and the state it writes is the owner book's.
#[test]
fn a_state_written_while_peering_books_its_entry_payments_as_a_deposit() {
    let mut world = World::new(3);
    world.closes(2);
    let account_state = world.nodes[0].scheme.account.state();
    world.disconnect(0);
    // The payment node 0 held, made again at the same place in the ledger.
    let id = world.pay(1, 0, ENTRY_PAID);
    world.close();
    let record = validated(&mut world, id.as_str());
    assert_eq!((record.ledger_index, record.index), (0x004C_4B43, 0));
    let scheme_state = dassie_xrpl::from_hex(&PEERING_STATE.concat()).unwrap();
    let state = support::join_bytes(&account_state, &scheme_state);
    world.restore(0, (state, FUNDED));
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED + ENTRY_PAID));
    assert_eq!(books.settlement, i128::from(FUNDED + ENTRY_PAID));
    assert!(books.reconciled.is_empty(), "{:?}", books.reconciled);
    assert_eq!(world.nodes[0].scheme.balance(), FUNDED + ENTRY_PAID);
    let written = dassie_settlement::SettlementScheme::state(&world.nodes[0].scheme.scheme);
    assert_eq!(written[0], 2, "the owner book's");
    // A restart from what it writes now books nothing again.
    let kept = world.kept(0);
    world.disconnect(0);
    world.restore(0, kept);
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED + ENTRY_PAID));
    assert!(books.reconciled.is_empty(), "{:?}", books.reconciled);
}

// spec/decisions/tigerstyle.md: a server that reports more XRP for the
// account than the books can hold stops the scheme, rather than booking an
// amount clamped to fit.
#[test]
fn more_xrp_than_the_books_hold_stops_the_scheme() {
    let mut world = funded();
    let owner = world.nodes[0].books.owner;
    let account = world.account(0);
    world.ledger.deposit(account, dassie_xrpl::MAX_DROPS / 10);
    world.closes(2);
    assert!(matches!(
        world.nodes[0].scheme.status(),
        LedgerStatus::Failed(_)
    ));
    assert_eq!(world.nodes[0].books.owner, owner);
    assert_eq!(world.nodes[0].scheme.deadline(), None);
    assert_eq!(world.nodes[1].scheme.status(), LedgerStatus::Synced);
}

// An account at the last sequence there is can send nothing more: the
// owner's payment waits rather than reuse it.
#[test]
fn the_last_sequence_is_never_reused() {
    let mut world = funded();
    let account = world.account(0);
    world.ledger.set_sequence(account, u32::MAX);
    world.close();
    world.disconnect(0);
    world.connect(0);
    let payment = OwnerPayment {
        destination: world.account(1).to_address(),
        tag: None,
        memo: None,
        amount: 1_000_000_000,
    };
    assert_eq!(
        world.pay_owner(0, &payment, &mut Vec::new()),
        Err(SchemeError::Unavailable)
    );
}

/// Whether node `index`'s scheme stopped.
fn failed(world: &World, index: usize) -> bool {
    matches!(world.nodes[index].scheme.status(), LedgerStatus::Failed(_))
}

// The network is checked first: the first request on a connection is
// `server_state`, and the account is subscribed only after it, then
// anchored on a ledger its UNL validated, which proves it: it is never
// read on the server's word
// (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
#[test]
fn the_network_is_checked_before_the_account_is_read() {
    let world = funded();
    let asked = &world.nodes[0].asked;
    assert_eq!(
        asked[..4],
        ["server_state", "subscribe", "state_proof", "validations"]
    );
    // `tx` gives a transaction's bytes only once its proof came, which
    // they must then prove (`World::request`).
    for read in ["account_info", "ledger_entry", "amm_info"] {
        assert!(!asked.iter().any(|asked| asked == read), "{read}");
    }
}

// A server off the XRP Ledger testnet (mainnet 0, devnet 2, a network
// above 1024), or one that names no network, stops the scheme before it
// reads the account: the faucet is never asked, nothing is booked or
// written off, and nothing can be signed.
#[test]
fn a_server_off_the_testnet_stops_the_scheme() {
    for network in [Some(0), Some(2), Some(21_338), None] {
        let mut ledger = FakeLedger::new();
        ledger.set_network_id(network);
        let mut world = World::on(ledger, 2);
        world.closes(20);
        for index in 0..2 {
            assert!(failed(&world, index), "{network:?}");
            assert_eq!(world.nodes[index].asked, ["server_state"], "{network:?}");
            // The faucet would have made the account.
            assert_eq!(world.ledger.balance(world.account(index)), None);
            let books = &world.nodes[index].books;
            assert_eq!(
                (books.settlement, books.owner, books.suspense, books.fees),
                (0, 0, 0, 0),
                "{network:?}"
            );
            assert_eq!(world.nodes[index].scheme.deadline(), None);
        }
        let payment = OwnerPayment {
            destination: world.account(1).to_address(),
            tag: None,
            memo: None,
            amount: 1_000_000_000,
        };
        assert_eq!(
            world.pay_owner(0, &payment, &mut Vec::new()),
            Err(SchemeError::Unavailable),
            "{network:?}"
        );
    }
}

// A synced scheme whose server is then off the testnet stops when it
// connects again: nothing is written off, and the books stay as they were.
// The other node, on its own connection, goes on.
#[test]
fn a_synced_scheme_whose_server_leaves_the_testnet_stops() {
    let mut world = funded();
    let before = (world.nodes[0].books.owner, world.nodes[0].books.settlement);
    world.ledger.set_network_id(Some(0));
    world.disconnect(0);
    world.connect(0);
    world.closes(20);
    assert!(failed(&world, 0));
    let books = &world.nodes[0].books;
    assert_eq!((books.owner, books.settlement), before);
    assert_eq!(books.suspense, 0);
    assert_eq!(world.nodes[0].scheme.balance(), FUNDED);
    assert_eq!(world.nodes[0].scheme.deadline(), None);
    assert_eq!(world.nodes[1].scheme.status(), LedgerStatus::Synced);
}

/// What the ledger holds under `hash`, validated.
fn validated(world: &mut World, hash: &str) -> dassie_xrpl::api::TxRecord {
    let hash: [u8; 32] = dassie_xrpl::from_hex(hash).unwrap().try_into().unwrap();
    let reply = world
        .ledger
        .request(999, &dassie_xrpl::api::tx_anywhere(1, &hash));
    let outcome = match dassie_xrpl::api::parse(&reply) {
        dassie_xrpl::api::Message::Response { outcome, .. } => Some(outcome),
        _ => None,
    };
    dassie_xrpl::api::tx_result(&outcome.unwrap().unwrap()).unwrap()
}

/// The transaction of the last fee node `index` booked.
fn submitted_hash(world: &World, index: usize) -> String {
    let fee_ids: Vec<&Name> = world.nodes[index].books.fee_ids.iter().collect();
    fee_ids.last().unwrap().as_str().to_string()
}

// spec/areas/xrpl.md#deposits-and-withdrawals: the owner pays out of the
// account, to a destination with a tag and one memo, as a deposit into a
// settlement hub is made. Once the ledger validated it, it is the owner's
// withdrawal, with its fee; the account it reached books it as a deposit.
// One at a time.
#[test]
fn the_owner_pays_out_of_the_account() {
    use dassie_settlement::OwnerMemo;

    let mut world = funded();
    let to = world.account(1);
    let payment = OwnerPayment {
        destination: to.to_address(),
        tag: Some(42),
        memo: Some(OwnerMemo {
            memo_type: b"https://example.com/memo".to_vec(),
            data: vec![7; 32],
        }),
        amount: 3_000_000_999,
    };
    let mut effects = Vec::new();
    let id = world.pay_owner(0, &payment, &mut effects).unwrap();
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::InFlight);
    assert_eq!(
        world.pay_owner(0, &payment, &mut Vec::new()),
        Err(SchemeError::Unavailable),
        "one at a time"
    );
    world.apply(0, effects);
    world.close();
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::Applied);
    let (a, b) = (&world.nodes[0].books, &world.nodes[1].books);
    assert_eq!(a.owner, i128::from(FUNDED) - 3_000_000_000);
    assert_eq!(a.fees, FEE);
    assert_eq!(b.owner, i128::from(FUNDED) + 3_000_000_000);
    assert_eq!(a.suspense + b.suspense, 0);
    let hash = submitted_hash(&world, 0);
    assert_eq!(hash, id.as_str(), "its ID is its hash");
    let record = validated(&mut world, &hash);
    assert_eq!(record.destination, Some(to));
    assert_eq!(record.destination_tag, Some(42));
    assert_eq!(record.delivered, Some(3_000_000));
    assert_eq!(
        record.memo,
        Some((b"https://example.com/memo".to_vec(), vec![7; 32]))
    );
}

// spec/areas/xrpl.md#deposits-and-withdrawals: the owner's payment on its
// way is kept across a restart and booked once after; what became of it
// outlives the next restart, for the last payment only.
#[test]
fn the_owners_payment_outlives_a_restart() {
    let mut world = funded();
    let id = world.pay(0, 1, 3_000_000_000);
    world.close();
    world.nodes[0].drop_submits = true;
    let second = world.pay(0, 1, 3_000_000_000);
    let kept = world.kept(0);
    world.disconnect(0);
    world.restore(0, kept);
    assert_eq!(world.owner_payment(0, second), OwnerPaymentState::InFlight);
    world.nodes[0].drop_submits = false;
    world.connect(0);
    world.closes(2);
    // Its outcome outlives another restart; the one before is no longer
    // the scheme's to tell.
    let kept = world.kept(0);
    world.restore(0, kept);
    assert_eq!(world.owner_payment(0, second), OwnerPaymentState::Applied);
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::Unknown);
    let a = &world.nodes[0].books;
    assert_eq!(a.owner, i128::from(FUNDED) - 6_000_000_000);
    assert_eq!(a.fees, 2 * FEE);
    assert_eq!(a.suspense, 0);
}

// An owner's payment the ledger refuses costs its fee and nothing else; one
// to the account itself, of nothing, or with too long a memo is refused
// before anything is signed.
#[test]
fn a_failed_owner_payment_costs_only_its_fee() {
    use dassie_settlement::{OwnerMemo, PaymentFailure};

    let mut world = funded();
    let nobody = dassie_xrpl::Wallet::from_seed([77; 16]).account();
    let payment = |destination: AccountId, amount: u64, memo: usize| OwnerPayment {
        destination: destination.to_address(),
        tag: None,
        memo: (memo > 0).then(|| OwnerMemo {
            memo_type: b"type".to_vec(),
            data: vec![1; memo],
        }),
        amount,
    };
    let own = world.account(0);
    for refused in [
        payment(own, 1_000_000, 0),
        payment(nobody, 999, 0),
        payment(nobody, 1_000_000, 65),
    ] {
        assert_eq!(
            world.pay_owner(0, &refused, &mut Vec::new()),
            Err(SchemeError::InvalidData)
        );
    }
    // Too little to make a new account.
    let mut effects = Vec::new();
    let id = world
        .pay_owner(0, &payment(nobody, 1_000_000, 0), &mut effects)
        .unwrap();
    world.apply(0, effects);
    world.close();
    // A failure the ledger recorded, too little: never taken for one that
    // never reached the ledger, also after a restart.
    let too_little = OwnerPaymentState::Failed(PaymentFailure::TooLittle);
    assert_eq!(world.owner_payment(0, id), too_little);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(books.fees, FEE);
    assert_eq!(books.paid, [(id, false)]);
    // Any other failure is told apart from it: to an account that takes
    // only payments with a destination tag, without one.
    let peer = world.account(1);
    let flags = world.ledger.account_flags(peer).unwrap();
    world.ledger.set_account_flags(
        peer,
        flags | dassie_xrpl::api::account_flags::REQUIRE_DEST_TAG,
    );
    let mut effects = Vec::new();
    let other = world
        .pay_owner(0, &payment(peer, 1_000_000, 0), &mut effects)
        .unwrap();
    world.apply(0, effects);
    world.close();
    assert_eq!(
        world.owner_payment(0, other),
        OwnerPaymentState::Failed(PaymentFailure::Other)
    );
    // What became of the last outlives a restart.
    let mut effects = Vec::new();
    let again = world
        .pay_owner(0, &payment(nobody, 1_000_000, 0), &mut effects)
        .unwrap();
    world.apply(0, effects);
    world.close();
    let kept = world.kept(0);
    world.restore(0, kept);
    assert_eq!(world.owner_payment(0, again), too_little);
}

// An owner's payment whose submission was lost is looked for once past its
// last ledger and, never seen there, cancelled: it moved nothing, not even
// a fee, which a failure the ledger recorded would have cost. The sequence
// is read again: the next one gets through.
#[test]
fn a_lost_owners_payment_is_cancelled_not_failed() {
    let mut world = funded();
    world.nodes[0].drop_submits = true;
    let id = world.pay(0, 1, 3_000_000_000);
    world.closes(25);
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::Cancelled);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(books.fees, 0);
    world.nodes[0].drop_submits = false;
    let next = world.pay(0, 1, 2_000_000_000);
    world.close();
    assert_eq!(world.owner_payment(0, next), OwnerPaymentState::Applied);
}

/// What node `index`'s books say happened: the owner's payments done,
/// what was cancelled, what suspense reconciled, and the account's
/// write-offs.
fn outcomes(world: &World, index: usize) -> (Vec<(Name, bool)>, Vec<Name>, usize, u64) {
    let node = &world.nodes[index];
    (
        node.books.paid.clone(),
        node.books.cancelled.clone(),
        node.books.reconciled.len(),
        node.scheme.account.write_offs(),
    )
}

// spec/designs/xrpl-validation.md#1-the-rule: whatever a server lies about,
// in each way the lying fake ledger tells, nothing is booked that the
// ledger did not apply, nothing it applied is cancelled, and nothing is
// written off. The owner's payment and a payment to the account applied
// while node 0 was away, and it comes back to a server that lies; what it
// books meanwhile, if anything, is what the ledger applied. Once its server
// is honest, it books both, once, and its books hold the ledger's.
#[test]
fn a_lying_server_books_cancels_and_writes_off_nothing() {
    for lie in Lie::ALL {
        let mut world = funded();
        // Ledgers enough that some closed long enough ago for their
        // validations to be stale, which an old anchor is.
        world.closes(70);
        let before = outcomes(&world, 0);
        let ours = world.pay(0, 1, 2_000_000_000);
        world.pay(1, 0, 3_000_000_000);
        // And a deposit: three transactions, one in the middle.
        let account = world.account(0);
        world.ledger.deposit(account, 5_000_000);
        world.nodes[0].lagging = true;
        world.disconnect(0);
        world.close();
        let lying = world.next_connection + 1;
        world.ledger.set_lie(lying, Some(lie));
        world.connect(0);
        world.closes(3);
        assert!(world.ledger.lies_told(lie) > 0, "{lie:?} never told");
        let (paid, cancelled, reconciled, write_offs) = outcomes(&world, 0);
        assert!(paid.iter().all(|done| *done == (ours, true)), "{lie:?}");
        assert_eq!(cancelled, before.1, "{lie:?}");
        assert_eq!((reconciled, write_offs), (before.2, before.3), "{lie:?}");
        assert!(
            world.nodes[0].books.owner <= i128::from(FUNDED) + 8_000_000_000,
            "{lie:?}"
        );
        assert!(!failed(&world, 0), "{lie:?}");
        world.ledger.set_lie(lying, None);
        world.wait(STALL_MS);
        world.closes(3);
        world.nodes[0].lagging = false;
        world.check();
        assert_eq!(
            world.nodes[0].scheme.status(),
            LedgerStatus::Synced,
            "{lie:?}"
        );
        let (paid, cancelled, reconciled, write_offs) = outcomes(&world, 0);
        assert_eq!(paid, [(ours, true)], "{lie:?}");
        assert_eq!(cancelled, before.1, "{lie:?}");
        assert_eq!((reconciled, write_offs), (before.2, before.3), "{lie:?}");
        assert_eq!(
            world.nodes[0].books.owner,
            i128::from(FUNDED) - 2_000_000_000 + 3_000_000_000 + 5_000_000_000,
            "{lie:?}"
        );
    }
}

// spec/designs/xrpl-validation.md#4-what-is-proven-and-how: the owner's
// payment that never applied, while the account's sequence moved past it
// with no transaction taking its number, as for an account deleted and
// made again, can be proven neither applied nor absent. A server that
// proves it applied, in a ledger the anchor's chain does not hold, is
// caught: it is not booked as applied, nor as cancelled once the server is
// honest.
#[test]
fn a_made_up_proof_of_ours_books_nothing() {
    let mut world = funded();
    let account = world.account(0);
    let sequence = world.ledger.sequence(account).unwrap();
    let ours = world.pay(0, 1, 2_000_000_000);
    world.ledger.set_sequence(account, sequence + 5);
    world.nodes[0].lagging = true;
    world.ledger.set_lie_for_all(Some(Lie::Transaction));
    world.closes(30);
    assert!(world.ledger.lies_told(Lie::Transaction) > 0);
    assert!(world.nodes[0].scheme.account.doubts(Doubt::Ledger) > 0);
    let applied = |world: &World| world.nodes[0].books.paid.contains(&(ours, true));
    assert!(!applied(&world));
    assert!(world.nodes[0].books.cancelled.is_empty());
    world.ledger.set_lie_for_all(None);
    world.wait(STALL_MS);
    world.closes(3);
    world.nodes[0].lagging = false;
    assert!(!applied(&world));
    assert_ne!(world.owner_payment(0, ours), OwnerPaymentState::Applied);
    assert_ne!(world.owner_payment(0, ours), OwnerPaymentState::Cancelled);
    assert_eq!(world.nodes[0].books.owner, i128::from(FUNDED));
}

// spec/designs/xrpl-validation.md#4-what-is-proven-and-how: the owner's
// payment that never applied is cancelled only once proven absent: the
// account's sequence, proven at an anchor past its last ledger, is not
// past its own.
#[test]
fn an_owners_payment_is_cancelled_only_once_proven_absent() {
    let mut world = funded();
    world.nodes[0].drop_submits = true;
    let id = world.pay(0, 1, 3_000_000_000);
    world.closes(20);
    assert_eq!(
        world.owner_payment(0, id),
        OwnerPaymentState::InFlight,
        "not yet past its last ledger"
    );
    world.closes(2);
    assert_eq!(world.owner_payment(0, id), OwnerPaymentState::Cancelled);
    assert_eq!(world.nodes[0].scheme.account.unproven().settlements, 0);
}
