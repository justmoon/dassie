//! The `xrpl-testnet` scheme against the fake ledger: each rule of how it
//! books transactions and settles (spec/areas/xrpl.md#following-the-account,
//! spec/areas/xrpl.md#settling), restarts and catch-up, with the books
//! checked against the ledger after every step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};

use dassie_ledger::Name;
use dassie_node_core::testing::SeededRng;
use dassie_settlement::{LedgerInput, LedgerStatus, ReconcileCause, SchemeError};
use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger, RESERVE_BASE};
use dassie_xrpl::scheme::MEMO_TYPE;
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
    /// Each peer's account, as what we owe it net of what it settled.
    peers: BTreeMap<Name, i128>,
    /// Our settlements prepared: peer and amount.
    pending: BTreeMap<Name, (Name, u64)>,
    /// Settlements in, by ID: each once.
    credited: BTreeSet<Name>,
    finalized: Vec<Name>,
    cancelled: Vec<Name>,
    /// Each reconciliation, in order: what the books gained, and why.
    reconciled: Vec<(i128, ReconcileCause)>,
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
        let mut world = Self {
            ledger,
            nodes: Vec::new(),
            now: 1_000,
            next_connection: 1,
        };
        for seed in 0..count {
            let node = name(&format!("node{seed}"));
            let scheme = XrplScheme::new("", &entropy(seed + 1), node, None, &[]).unwrap();
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
                Effect::Fee { amount, .. } => {
                    books.settlement -= i128::from(amount);
                    books.fees += i128::from(amount);
                }
                Effect::IncomingSettlement { peer, amount, id } => {
                    assert!(books.credited.insert(id.unwrap()), "credited twice");
                    books.settlement += i128::from(amount);
                    *books.peers.entry(peer).or_default() += i128::from(amount);
                }
                Effect::Finalize { settlement } => {
                    let (peer, amount) = books.pending.remove(&settlement).unwrap();
                    books.settlement -= i128::from(amount);
                    *books.peers.entry(peer).or_default() -= i128::from(amount);
                    books.finalized.push(settlement);
                }
                Effect::Cancel { settlement } => {
                    books.pending.remove(&settlement).unwrap();
                    books.cancelled.push(settlement);
                }
                Effect::SendMessage { .. } => {}
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
            }
        }
    }

    /// Closes a ledger, and delivers the streams.
    fn close(&mut self) {
        self.now += 4_000;
        for (connection, message) in self.ledger.close() {
            if let Some(index) = self
                .nodes
                .iter()
                .position(|n| n.online && n.connection == connection)
            {
                self.input(index, LedgerInput::Message(&message));
            }
        }
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
        self.check();
    }

    fn closes(&mut self, count: usize) {
        for _ in 0..count {
            self.close();
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
            // A reset the scheme has not seen yet is not its to know.
            if node.scheme.status() == LedgerStatus::Synced
                && let Some(balance) = self.ledger.balance(node.scheme.account())
            {
                let bookable = (balance * UNITS_PER_DROP).saturating_sub(node.scheme.reserve());
                assert_eq!(node.scheme.balance(), bookable, "{}", node.name);
            }
        }
        assert_eq!(self.ledger.total(), dassie_xrpl::MAX_DROPS);
    }

    /// Peers `a` and `b`, as the peer protocol would: `a` asks.
    fn peer(&mut self, a: usize, b: usize) -> (Vec<u8>, Vec<u8>) {
        let (name_a, name_b) = (self.nodes[a].name, self.nodes[b].name);
        let mut info = Vec::new();
        self.nodes[b].scheme.peering_info(&mut info);
        let mut request = Vec::new();
        self.nodes[a]
            .scheme
            .create_peering_request(name_b, &info, &mut request)
            .unwrap();
        let mut response = Vec::new();
        let state_at_b = self.nodes[b]
            .scheme
            .accept_peering_request(name_a, &request, &mut response)
            .unwrap();
        let state_at_a = self.nodes[a]
            .scheme
            .finalize_peering_request(name_b, &info, &response)
            .unwrap();
        self.nodes[b].scheme.on_peered(name_a, &state_at_b);
        self.nodes[a].scheme.on_peered(name_b, &state_at_a);
        (state_at_a, state_at_b)
    }

    /// `from` settles `amount` with `to`, whose state at `from` is
    /// `state`. Returns the settlement's ID.
    fn settle(&mut self, from: usize, to: usize, amount: u64, state: &[u8]) -> Name {
        let peer = self.nodes[to].name;
        let mut rng = SeededRng::new(1);
        let prepared = self.nodes[from]
            .scheme
            .prepare_settlement(&mut rng, peer, amount, state)
            .unwrap();
        self.nodes[from]
            .books
            .pending
            .insert(prepared.id, (peer, prepared.amount));
        let mut effects = Vec::new();
        self.nodes[from]
            .scheme
            .execute_settlement(prepared.id, &mut effects);
        self.apply(from, effects);
        prepared.id
    }
}

/// Two funded, synced nodes, peered: the states each keeps for the other.
fn funded_pair() -> (World, Vec<u8>, Vec<u8>) {
    let mut world = World::new(2);
    world.closes(2);
    for node in &world.nodes {
        assert_eq!(node.scheme.status(), LedgerStatus::Synced, "{}", node.name);
    }
    let (state_a, state_b) = world.peer(0, 1);
    (world, state_a, state_b)
}

const FUNDED: u64 = (FAUCET_DROPS - RESERVE_BASE) * UNITS_PER_DROP;

// ACC-19, ACC-20: an account the faucet creates is a deposit, the reserve
// left out.
#[test]
fn a_new_account_is_funded_and_its_reserve_never_booked() {
    let (world, _, _) = funded_pair();
    for node in &world.nodes {
        assert_eq!(node.books.owner, i128::from(FUNDED));
        assert_eq!(node.scheme.reserve(), RESERVE_BASE * UNITS_PER_DROP);
        assert!(node.scheme.deposit_address().unwrap().starts_with('r'));
    }
}

// ACC-21, ACC-22: a settlement is whole drops; it is posted once it
// succeeded, its fee booked, and the peer credits what was delivered,
// once.
#[test]
fn a_settlement_settles_both_books() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 5_000_000_999, &state_a);
    world.close();
    let (a, b) = (&world.nodes[0].books, &world.nodes[1].books);
    assert_eq!(a.finalized, [id]);
    assert_eq!(a.peers[&world.nodes[1].name], -5_000_000_000);
    assert_eq!(a.fees, 10 * i128::from(UNITS_PER_DROP));
    assert_eq!(b.peers[&world.nodes[0].name], 5_000_000_000);
    assert_eq!(b.credited.len(), 1);
    assert_eq!(b.owner, i128::from(FUNDED));
}

// A Payment with our memo from an address that is not the peer's is a
// deposit: attribution cannot be spoofed.
#[test]
fn a_settlement_from_the_wrong_address_is_a_deposit() {
    let mut world = World::new(3);
    world.closes(2);
    // Node 0 knows node 1's address; node 2 sends claiming to be node 1.
    let (_, _) = world.peer(0, 1);
    let mut info = Vec::new();
    world.nodes[0].scheme.peering_info(&mut info);
    let impostor = world.nodes[2].name;
    world.nodes[2].name = world.nodes[1].name;
    let mut request = Vec::new();
    world.nodes[2]
        .scheme
        .create_peering_request(name("node0"), &info, &mut request)
        .unwrap();
    let _ = impostor;
    world.settle(2, 0, 1_000_000_000, &info);
    world.close();
    let books = &world.nodes[0].books;
    assert!(books.credited.is_empty());
    assert_eq!(books.owner, i128::from(FUNDED) + 1_000_000_000);
}

// Peering attempts that never became peerings, ours and others', as many
// as the scheme keeps addresses, take no peer's place: the peer's next
// settlement is still its own, not the owner's deposit.
#[test]
fn peering_attempts_never_displace_a_peers_address() {
    let (mut world, _, state_b) = funded_pair();
    let peer = world.nodes[1].name;
    let mut info = Vec::new();
    world.nodes[1].scheme.peering_info(&mut info);
    for i in 0..dassie_xrpl::scheme::MAX_PEERS {
        let stranger = name(&format!("stranger{i}"));
        let (mut request, mut response) = (Vec::new(), Vec::new());
        let scheme = &mut world.nodes[0].scheme;
        scheme
            .create_peering_request(stranger, &info, &mut request)
            .unwrap();
        scheme
            .accept_peering_request(stranger, &info, &mut response)
            .unwrap();
        scheme.check_invariants();
    }
    world.settle(1, 0, 3_000_000_000, &state_b);
    world.close();
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(books.peers.get(&peer), Some(&3_000_000_000));
}

// A Payment the ledger refuses after taking its fee (here: to an account
// that does not exist, too little to create it) is cancelled, its fee
// booked.
#[test]
fn a_failed_settlement_is_cancelled_and_costs_its_fee() {
    let (mut world, _, _) = funded_pair();
    let nobody = dassie_xrpl::Wallet::from_seed([77; 16]).account();
    let mut state = Vec::new();
    let address = nobody.to_address();
    state.push(u8::try_from(address.len()).unwrap());
    state.extend_from_slice(address.as_bytes());
    let id = world.settle(0, 1, 1_000, &state);
    world.close();
    let books = &world.nodes[0].books;
    assert_eq!(books.cancelled, [id]);
    assert_eq!(books.fees, 10 * i128::from(UNITS_PER_DROP));
}

// A settlement whose submission was lost is looked for once past its last
// ledger, and cancelled when the ledger never saw it.
#[test]
fn a_lost_submission_expires_and_is_cancelled() {
    let (mut world, state_a, _) = funded_pair();
    world.nodes[0].drop_submits = true;
    let id = world.settle(0, 1, 2_000_000_000, &state_a);
    world.closes(25);
    assert_eq!(world.nodes[0].books.cancelled, [id]);
    assert!(world.nodes[0].books.pending.is_empty());
    // The sequence was read again: the next one gets through.
    world.nodes[0].drop_submits = false;
    let next = world.settle(0, 1, 2_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [next]);
}

// ACC-16: a settlement signed and kept before a restart is submitted again
// after it, and posted once; what happened while away is booked once,
// with who did it.
#[test]
fn a_restart_resubmits_and_catches_up() {
    let (mut world, state_a, state_b) = funded_pair();
    world.nodes[0].drop_submits = true;
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    // Node 0 goes down with its settlement signed; its state and balance
    // are what the node kept.
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    // Meanwhile node 1 settles with node 0, and someone deposits.
    world.settle(1, 0, 4_000_000_000, &state_b);
    world.ledger.deposit(world.account(0), 7_000_000);
    world.closes(2);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    world.nodes[0].drop_submits = false;
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert_eq!(books.credited.len(), 1);
    assert_eq!(books.owner, i128::from(FUNDED) + 7_000_000_000);
    assert_eq!(books.suspense, 0);
}

// When the server's history does not reach back, the balance says what
// changed: reconciled through suspense, exactly.
#[test]
fn a_short_history_reconciles_by_the_balance() {
    let (mut world, _, state_b) = funded_pair();
    world.disconnect(0);
    world.settle(1, 0, 4_000_000_000, &state_b);
    world.closes(3);
    world.ledger.forget_history();
    world.connect(0);
    world.closes(1);
    assert_eq!(world.nodes[0].books.suspense, 4_000_000_000);
    assert!(world.nodes[0].books.credited.is_empty());
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
    let (mut world, _, _) = funded_pair();
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
    let (mut world, _, _) = funded_pair();
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
    let (mut world, _, state_b) = funded_pair();
    world.disconnect(0);
    world.settle(1, 0, 4_000_000_000, &state_b);
    world.closes(3);
    world.ledger.forget_history();
    world.ledger.set_reserve(2 * RESERVE_BASE, 0);
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
    let (mut world, _, _) = funded_pair();
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
    let (mut world, _, _) = funded_pair();
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
    let mut scheme =
        XrplScheme::new(r#"{"faucet":null}"#, &entropy(1), name("node0"), None, &[]).unwrap();
    assert!(!scheme.has_faucet());
    let mut effects = Vec::new();
    assert_eq!(
        scheme.ask_faucet(&mut effects),
        Err(SchemeError::Unsupported)
    );
    assert!(effects.is_empty());
}

// spec/decisions/synced-before-peering.md: the scheme has synced once its
// first sync booked what the account held as the owner's, and the node peers
// on it only from then. That is kept with its state, so a restart that cannot
// reach the ledger keeps it.
#[test]
fn the_first_sync_is_kept_across_restarts() {
    let node = name("node0");
    let fresh = XrplScheme::new("", &entropy(1), node, None, &[]).unwrap();
    assert!(!fresh.has_synced(), "synced before it saw its ledger");
    let mut world = World::new(1);
    world.closes(2);
    assert!(world.nodes[0].scheme.has_synced(), "never synced");
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let restarted = XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    assert_eq!(restarted.status(), LedgerStatus::Connecting);
    assert!(restarted.has_synced(), "a restart forgot the first sync");
}

// spec/decisions/ledger-resets.md: a reset starts the chain again from a
// ledger below the newest seen, which closed later. It is recognised as one
// even when the account exists again on the new chain: the old funds are a
// loss, the new a deposit.
#[test]
fn a_reset_is_recognised_when_the_account_exists_again() {
    let (mut world, _, _) = funded_pair();
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

// Found in review: after a reconnect, a server behind the newest ledger
// seen answers that the account does not exist, as of a ledger before it
// was made, while a settlement of ours applied since. Nothing is cancelled
// or written off.
#[test]
fn an_absence_from_a_ledger_already_passed_cancels_nothing() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 5_000_000_000, &state_a);
    world.disconnect(0);
    world.close();
    assert!(world.nodes[1].books.credited.contains(&id));
    world.nodes[0].hold = Some("account_info");
    world.connect(0);
    let request = world.nodes[0].held_request.as_ref().unwrap();
    let reply = serde_json::to_vec(&serde_json::json!({
        "type": "response", "status": "error", "id": request["id"],
        "error": "actNotFound", "ledger_index": 1, "validated": true
    }))
    .unwrap();
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .on_ledger(world.now, LedgerInput::Message(&reply), &mut effects);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Cancel { settlement } if *settlement == id)),
        "payment already credited by receiver was cancelled: {effects:?}"
    );
}

// As above, against a server that stopped at the first ledger, before the
// account was made: it is never believed, and the scheme waits. Once a
// server has caught up, the settlement is posted.
#[test]
fn a_server_that_never_saw_the_account_writes_nothing_off() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 5_000_000_000, &state_a);
    world.disconnect(0);
    world.close();
    world.nodes[0].behind = Some(FakeLedger::new());
    world.connect(0);
    // Long enough to reconnect when the stopped server goes silent.
    world.closes(10);
    assert!(world.nodes[0].reconnects > 0);
    let books = &world.nodes[0].books;
    assert!(books.cancelled.is_empty());
    assert_eq!(books.suspense, 0);
    assert!(books.pending.contains_key(&id));
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
    world.nodes[0].behind = None;
    world.disconnect(0);
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert_eq!(books.suspense, 0);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// Found in review: the account's state, asked for while synced (here its
// sequence, read again once a settlement never executed is dropped), whose
// answer is older than what the scheme has seen, is asked again as the
// next ledger closes, not dropped for good.
#[test]
fn an_old_answer_while_synced_is_asked_again() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 1_000_000_000, &state_a)
        .unwrap();
    world.closes(20);
    world.nodes[0].hold = Some("account_info");
    world.close();
    let request = world.nodes[0].held_request.take().unwrap();
    let reply = serde_json::to_vec(&serde_json::json!({
        "type": "response", "status": "success", "id": request["id"],
        "result": {
            "account_data": { "Balance": "1000000000", "Sequence": 1, "OwnerCount": 0 },
            "ledger_index": 1, "validated": true,
        }
    }))
    .unwrap();
    world.input(0, LedgerInput::Message(&reply));
    world.nodes[0].hold = None;
    let asked = |world: &World| {
        let asked = &world.nodes[0].asked;
        asked.iter().filter(|c| *c == "account_info").count()
    };
    let before = asked(&world);
    world.close();
    assert_eq!(asked(&world), before + 1);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    world.close();
    assert_eq!(asked(&world), before + 1, "asked once");
}

// spec/decisions/ledger-resets.md: a server behind what the scheme booked
// (here a peer's settlement), or only behind the newest ledger it saw, says
// nothing new: nothing is booked from it, and the scheme waits for one that
// caught up.
#[test]
fn a_server_behind_is_never_booked_from() {
    for settled in [true, false] {
        let (mut world, _, state_b) = funded_pair();
        let behind = world.ledger.clone();
        if settled {
            world.settle(1, 0, 4_000_000_000, &state_b);
        }
        world.closes(3);
        let books = |world: &World| {
            let books = &world.nodes[0].books;
            (
                books.settlement,
                books.owner,
                books.suspense,
                books.credited.len(),
            )
        };
        let before = books(&world);
        world.disconnect(0);
        world.nodes[0].behind = Some(behind);
        world.connect(0);
        world.closes(3);
        assert_eq!(books(&world), before, "{settled}");
        assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
        world.nodes[0].behind = None;
        world.disconnect(0);
        world.connect(0);
        world.closes(2);
        assert_eq!(books(&world), before, "{settled}");
        assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    }
}

// The settlement message carries the transaction's hash, and books
// nothing: the ledger does.
#[test]
fn the_settlement_claim_books_nothing() {
    let (mut world, _, _) = funded_pair();
    let mut effects = Vec::new();
    let peer = world.nodes[1].name;
    world.nodes[0]
        .scheme
        .handle_settlement(peer, 10, &[1; 32], &[], &mut effects)
        .unwrap();
    assert!(effects.is_empty());
    assert!(
        world.nodes[0]
            .scheme
            .handle_settlement(peer, 10, &[1; 31], &[], &mut effects)
            .is_err()
    );
    let _ = MEMO_TYPE;
}

// Settlements wait while the scheme cannot settle: before it syncs, under
// a drop, or beyond what it holds above the reserve.
#[test]
fn a_settlement_waits_when_it_cannot_be_made() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    let scheme = &mut world.nodes[0].scheme;
    for amount in [999, FUNDED + UNITS_PER_DROP] {
        assert_eq!(
            scheme
                .prepare_settlement(&mut rng, peer, amount, &state_a)
                .err(),
            Some(dassie_settlement::SchemeError::Unavailable),
            "{amount}"
        );
    }
    world.disconnect(0);
    assert_eq!(
        world.nodes[0]
            .scheme
            .prepare_settlement(&mut rng, peer, 1_000_000, &state_a)
            .err(),
        Some(dassie_settlement::SchemeError::Unavailable)
    );
}

// Several settlements in flight at once, to several peers, with
// consecutive sequences: all get in.
#[test]
fn settlements_go_several_at_once() {
    let mut world = World::new(4);
    world.closes(2);
    let states: Vec<Vec<u8>> = (1..4).map(|b| world.peer(0, b).0).collect();
    let ids: Vec<Name> = states
        .iter()
        .enumerate()
        .map(|(i, state)| world.settle(0, i + 1, 1_000_000_000, state))
        .collect();
    world.close();
    assert_eq!(world.nodes[0].books.finalized, ids);
    for b in 1..4 {
        assert_eq!(world.nodes[b].books.credited.len(), 1);
    }
}

// Found in review: a settlement of ours that the ledger validated while the
// node was away, with an account history too short to read it from, is
// still posted, once looked up by its hash: what the balance booked
// through suspense comes back out of it.
#[test]
fn a_settlement_validated_while_away_is_still_posted() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    world.closes(2);
    world.ledger.shorten_account_history();
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    world.connect(0);
    world.closes(40);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert!(books.pending.is_empty());
    assert_eq!(books.suspense, 0, "the suspense entry came back out");
}

// As above, with a settlement and fee larger than what the account holds
// after it: posting them takes the books below zero, and what they are
// short comes from suspense, so the books still match the balance.
#[test]
fn a_large_settlement_validated_while_away_keeps_the_books() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 700_000_000_000, &state_a);
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    world.closes(2);
    world.ledger.shorten_account_history();
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    world.connect(0);
    world.closes(40);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert!(books.pending.is_empty());
    assert_eq!(books.suspense, 0, "the suspense entry came back out");
}

// Found in review: a settlement the node prepared but could not book, so
// never executed, is never sent, even across a reconnect.
#[test]
fn a_settlement_never_executed_is_never_sent() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 3_000_000_000, &state_a)
        .unwrap();
    world.disconnect(0);
    world.connect(0);
    world.closes(25);
    assert_eq!(world.nodes[1].books.credited.len(), 0, "nothing was paid");
    // One discarded at once frees its sequence for the next.
    let prepared = world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 1_000_000_000, &state_a)
        .unwrap();
    world.nodes[0].scheme.discard_settlement(prepared.id);
    let id = world.settle(0, 1, 1_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [id]);
}

// spec/areas/xrpl.md#state-and-restarts: a settlement signed and booked,
// but never executed (the node stopped first), is dropped past its last
// ledger, and the account remembers it as never sent. The scheme that
// reconciles it after a restart cancels it, rather than looking for a hash
// no ledger will ever hold and leaving it pending for good.
#[test]
fn a_settlement_never_sent_is_cancelled_after_a_restart() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    let prepared = world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 3_000_000_000, &state_a)
        .unwrap();
    world.nodes[0]
        .books
        .pending
        .insert(prepared.id, (peer, prepared.amount));
    world.closes(25);
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(prepared.id, prepared.amount)], &mut effects);
    assert_eq!(
        effects,
        [Effect::Cancel {
            settlement: prepared.id
        }]
    );
    world.apply(0, effects);
    world.nodes[0].asked.clear();
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.cancelled, [prepared.id]);
    assert!(books.pending.is_empty());
    assert!(
        !world.nodes[0].asked.iter().any(|command| command == "tx"),
        "looked for a transaction never sent"
    );
}

// Found in review: a connection that goes quiet is made anew, rather than
// leaving the scheme synced to a ledger it no longer hears.
#[test]
fn a_silent_connection_is_made_anew() {
    let (mut world, _, _) = funded_pair();
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
// apply: it is cancelled, and settling starts anew.
#[test]
fn a_reset_cancels_what_was_in_flight() {
    let (mut world, state_a, _) = funded_pair();
    world.nodes[0].drop_submits = true;
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    world.ledger.reset();
    world.disconnect(0);
    world.connect(0);
    world.closes(20);
    assert_eq!(world.nodes[0].books.cancelled, [id]);
    world.nodes[0].drop_submits = false;
    let next = world.settle(0, 1, 1_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [next]);
}

// Found in review: a settlement the node held pending across a restart
// though the scheme had settled it (its posting waited for an earlier
// announcement) is looked up by its hash, and posted.
#[test]
fn a_pending_settlement_unknown_to_the_scheme_is_recovered() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    world.close();
    // The scheme settled it; the node did not post it.
    let finalized = world.nodes[0].books.finalized.pop().unwrap();
    assert_eq!(finalized, id);
    let peer = world.nodes[1].name;
    let books = &mut world.nodes[0].books;
    books.pending.insert(id, (peer, 3_000_000_000));
    books.settlement += 3_000_000_000;
    *books.peers.get_mut(&peer).unwrap() += 3_000_000_000;
    // A restart hands the scheme what the node holds pending.
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(id, 3_000_000_000)], &mut effects);
    world.connect(0);
    world.closes(2);
    assert_eq!(world.nodes[0].books.finalized, [id]);
    assert!(world.nodes[0].books.pending.is_empty());
}

// A settlement left pending across a restart that no history holds cannot
// be known to have applied: it stays pending, for the owner
// (spec/areas/xrpl.md#state-and-restarts). It is asked again less and less
// often, not at every ledger.
#[test]
fn a_settlement_no_history_holds_stays_pending_and_is_asked_less_often() {
    let (mut world, _, _) = funded_pair();
    let id = Name::hex(&[7; 32], true);
    let peer = world.nodes[1].name;
    world.nodes[0]
        .books
        .pending
        .insert(id, (peer, 1_000_000_000));
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(id, 1_000_000_000)], &mut effects);
    world.connect(0);
    let lookups = |world: &World| world.nodes[0].asked.iter().filter(|c| *c == "tx").count();
    world.closes(600);
    // Asked at once, then after 1, 2, 4 … 256 ledgers: 10 times in 600.
    assert_eq!(lookups(&world), 10);
    world.closes(512);
    assert_eq!(lookups(&world), 12, "asked every 256 ledgers at most");
    let books = &world.nodes[0].books;
    assert!(books.finalized.is_empty() && books.cancelled.is_empty());
    assert!(books.pending.contains_key(&id));
}

// spec/decisions/tigerstyle.md: a server that reports more XRP for the
// account than the books can hold stops the scheme, rather than booking an
// amount clamped to fit.
#[test]
fn more_xrp_than_the_books_hold_stops_the_scheme() {
    let (mut world, _, _) = funded_pair();
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

// Found by the invariants (spec/decisions/tigerstyle.md): more settlements to
// look up after a restart than requests may wait made the scheme reconnect,
// and then look them all up again, for ever. It asks for a few at a time.
#[test]
fn many_settlements_to_recover_are_looked_up_a_few_at_a_time() {
    let (mut world, _, _) = funded_pair();
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let pending: Vec<(Name, u64)> = (0..20)
        .map(|byte| (Name::hex(&[byte; 32], true), 1_000_000_000))
        .collect();
    let mut effects = Vec::new();
    world.nodes[0].scheme.reconcile(&pending, &mut effects);
    world.nodes[0].hold = Some("tx");
    world.connect(0);
    world.closes(2);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    assert_eq!(world.nodes[0].reconnects, 0);
    assert_eq!(world.nodes[0].held, 8);
}

// An account at the last sequence there is can send nothing more: a
// settlement waits rather than reuse it.
#[test]
fn the_last_sequence_is_never_reused() {
    let (mut world, state_a, _) = funded_pair();
    let account = world.account(0);
    world.ledger.set_sequence(account, u32::MAX);
    world.disconnect(0);
    world.connect(0);
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    let prepared =
        world.nodes[0]
            .scheme
            .prepare_settlement(&mut rng, peer, 1_000_000_000, &state_a);
    assert_eq!(prepared.map(|_| ()), Err(SchemeError::Unavailable));
}

/// Whether node `index`'s scheme stopped.
fn failed(world: &World, index: usize) -> bool {
    matches!(world.nodes[index].scheme.status(), LedgerStatus::Failed(_))
}

// The network is checked first: the first request on a connection is
// `server_state`, and the account is subscribed only after it.
#[test]
fn the_network_is_checked_before_the_account_is_read() {
    let (world, _, _) = funded_pair();
    let asked = &world.nodes[0].asked;
    assert_eq!(asked[..3], ["server_state", "subscribe", "account_info"]);
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
        let peer = world.nodes[1].name;
        let (state, _) = world.peer(0, 1);
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
        let mut rng = SeededRng::new(1);
        let prepared =
            world.nodes[0]
                .scheme
                .prepare_settlement(&mut rng, peer, 1_000_000_000, &state);
        assert_eq!(
            prepared.err(),
            Some(SchemeError::Unavailable),
            "{network:?}"
        );
    }
}

// A synced scheme whose server is then off the testnet stops when it
// connects again: nothing is written off, and the books stay as they were.
// The other node, on its own connection, goes on.
#[test]
fn a_synced_scheme_whose_server_leaves_the_testnet_stops() {
    let (mut world, _, _) = funded_pair();
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
