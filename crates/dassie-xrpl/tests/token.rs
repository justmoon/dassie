//! The token scheme's rules in depth (spec/areas/xrpl.md#the-token-scheme),
//! in DSUSD: `xrpl-testnet-dsusd` against the fake ledger's testnet, wired
//! to its account as the node wires it, alone or next to `xrpl-testnet-xrp` on
//! the same account: the line set before it syncs, settling, booking by the
//! line, the issuer's freezes and clawbacks, gas, testnet resets and
//! restarts, with the books checked against the ledger after every ledger.
//! Every token scheme is this code; `tests/tokens.rs` pins that mainnet's
//! RLUSD runs on it unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};

use dassie_kit::sim::SeededRng;
use dassie_ledger::Name;
use dassie_settlement::{
    AssetEffect, Detach, DollarRate, Effect, EntryPayment, ExternalAsset, LedgerInput,
    LedgerStatus, PegPause, PreparedSettlement, ReconcileCause, SchemeError, SettlementScheme,
};
use dassie_xrpl::account::{self, News, XrplAccount};
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::codec::{Amount, Payment, Signing, TokenValue, Transact, TrustSet};
use dassie_xrpl::dsusd::{self, DsusdScheme, UNITS_PER_MICRO};
use dassie_xrpl::entry::ENTRY_HOLD_LEDGERS;
use dassie_xrpl::fake::{FAUCET_DROPS, FEE_BASE, FakeLedger, Lie, RESERVE_BASE, RESERVE_INC};
use dassie_xrpl::{AccountId, UNITS_PER_DROP, Wallet, XrplScheme, api};
use proptest::prelude::*;

/// The XRP scheme's books and the DSUSD scheme's.
const XRP: usize = 0;
const USD: usize = 1;

/// What the node would book for a scheme, in internal units.
#[derive(Debug, Default)]
struct Books {
    /// `assets/settlement`, as a debit balance.
    settlement: i128,
    owner: i128,
    suspense: i128,
    fees: i128,
    /// Every deposit and withdrawal booked, in order.
    deposits: Vec<u64>,
    withdrawals: Vec<u64>,
    /// Every loss reconciled through suspense, in order.
    losses: Vec<u64>,
    /// Each reconciliation, in order: what the books gained, and why.
    reconciled: Vec<(i128, ReconcileCause)>,
    peers: BTreeMap<Name, i128>,
    pending: BTreeMap<Name, (Name, u64)>,
    credited: BTreeSet<Name>,
    finalized: Vec<Name>,
    cancelled: Vec<Name>,
    /// Every fee booked, by transaction: each once.
    fee_ids: BTreeSet<Name>,
    /// The owner's payments held against the owner's funds while they are
    /// on their way, as the node holds them, by ID.
    owner_holds: BTreeMap<Name, u64>,
}

impl Books {
    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::Deposit { amount } => {
                self.settlement += i128::from(amount);
                self.owner += i128::from(amount);
                self.deposits.push(amount);
            }
            Effect::OwnerPaid { id, applied } => {
                let amount = self
                    .owner_holds
                    .remove(&id)
                    .expect("an owner's payment held");
                if applied {
                    self.settlement -= i128::from(amount);
                    self.owner -= i128::from(amount);
                }
            }
            Effect::Withdrawal { amount } => {
                self.settlement -= i128::from(amount);
                self.owner -= i128::from(amount);
                self.withdrawals.push(amount);
            }
            Effect::Reconcile {
                amount,
                gain,
                cause,
            } => {
                if !gain {
                    self.losses.push(amount);
                }
                let amount = if gain {
                    i128::from(amount)
                } else {
                    -i128::from(amount)
                };
                self.settlement += amount;
                self.suspense += amount;
                self.reconciled.push((amount, cause));
            }
            Effect::Fee { amount, id } => {
                assert!(self.fee_ids.insert(id), "a fee booked twice");
                self.settlement -= i128::from(amount);
                self.fees += i128::from(amount);
            }
            Effect::IncomingSettlement { peer, amount, id } => {
                assert!(self.credited.insert(id.unwrap()), "credited twice");
                self.settlement += i128::from(amount);
                *self.peers.entry(peer).or_default() += i128::from(amount);
            }
            Effect::Finalize { settlement } => {
                let (peer, amount) = self.pending.remove(&settlement).unwrap();
                self.settlement -= i128::from(amount);
                *self.peers.entry(peer).or_default() -= i128::from(amount);
                self.finalized.push(settlement);
            }
            Effect::Cancel { settlement } => {
                self.pending.remove(&settlement).unwrap();
                self.cancelled.push(settlement);
            }
            Effect::PeerLoss { .. } => panic!("an XRPL scheme books no peer losses"),
        }
    }

    /// What the owner may spend, as the node tells the account.
    fn funds(&self) -> u64 {
        let pending: i128 = self
            .pending
            .values()
            .map(|(_, a)| i128::from(*a))
            .sum::<i128>()
            + self
                .owner_holds
                .values()
                .map(|a| i128::from(*a))
                .sum::<i128>();
        u64::try_from((self.owner + self.suspense - self.fees - pending).max(0)).unwrap()
    }
}

struct Node {
    name: Name,
    seed: u8,
    account: XrplAccount,
    xrp: Option<XrplScheme>,
    dsusd: Option<DsusdScheme>,
    books: [Books; 2],
    connection: u64,
    online: bool,
    /// What the owner may spend on the XRP ledger, as the node says it;
    /// the books' own figure unless a test sets it.
    xrp_funds: Option<u64>,
    /// Each scheme the books stopped, or the owner removed: told nothing,
    /// its state as it was then.
    away: [bool; 2],
}

impl Node {
    fn scheme(&mut self, ledger: usize) -> &mut dyn SettlementScheme {
        if ledger == XRP {
            self.xrp.as_mut().unwrap()
        } else {
            self.dsusd.as_mut().unwrap()
        }
    }

    fn dsusd(&self) -> &DsusdScheme {
        self.dsusd.as_ref().unwrap()
    }

    /// Whether the scheme of `ledger` runs on the account.
    fn runs(&self, ledger: usize) -> bool {
        let made = if ledger == XRP {
            self.xrp.is_some()
        } else {
            self.dsusd.is_some()
        };
        made && !self.away[ledger]
    }

    fn check_invariants(&self) {
        self.account.check_invariants();
        if let Some(xrp) = &self.xrp {
            xrp.check_invariants();
        }
        if let Some(dsusd) = &self.dsusd {
            dsusd.check_invariants();
        }
    }

    fn status(&self, ledger: usize) -> LedgerStatus {
        match self.account.status() {
            LedgerStatus::Synced if ledger == XRP => self.xrp.as_ref().unwrap().status(),
            LedgerStatus::Synced => self.dsusd().status(),
            other => other,
        }
    }

    /// The states the books keep: the account's, and each scheme's with its
    /// balance.
    fn states(&self) -> (Vec<u8>, [Option<Kept>; 2]) {
        (
            self.account.state(),
            [
                self.xrp.as_ref().map(|s| (s.balance(), s.state())),
                self.dsusd.as_ref().map(|s| (s.balance(), s.state())),
            ],
        )
    }
}

/// What the books keep of a scheme: its balance and its state.
type Kept = (u64, Vec<u8>);

/// The ID of the scheme of `ledger`.
fn scheme_name(ledger: usize) -> Name {
    if ledger == XRP {
        name(dassie_xrpl::scheme::SCHEME_ID)
    } else {
        name(dsusd::SCHEME_ID)
    }
}

impl Node {
    /// The books refused what the scheme of `ledger` said, in `effects`,
    /// and the node stops it before the account forgets what it told: none
    /// of it is booked, and the scheme is as its checkpoint `kept` it.
    fn stopped(&mut self, ledger: usize, kept: &Kept, effects: &mut Vec<Effect>) {
        effects.clear();
        let (balance, state) = kept;
        if ledger == XRP {
            self.xrp =
                Some(XrplScheme::new(&self.account, self.name, Some(*balance), state).unwrap());
        } else {
            self.dsusd =
                Some(DsusdScheme::new(&self.account, self.name, Some(*balance), state).unwrap());
        }
        self.away[ledger] = true;
        self.account.detach(scheme_name(ledger), Detach::Stopped);
    }
}

struct World {
    ledger: FakeLedger,
    nodes: Vec<Node>,
    now: u64,
    next_connection: u64,
    /// Whether the server answers no lookup of a transaction.
    drop_lookups: bool,
    /// The scheme, by its ledger, whose bookings the books refuse when it
    /// is next told anything of its own, which stops it.
    refuse_own: Option<usize>,
    /// What a server's answer to a lookup is changed to say.
    tamper_lookup: Option<fn(&mut serde_json::Value)>,
    /// Whether the books may lag the ledger, as while a server lies.
    lagging: bool,
}

fn name(text: &str) -> Name {
    Name::new(text).unwrap()
}

fn value(text: &str) -> TokenValue {
    TokenValue::parse(text).unwrap()
}

fn entropy(seed: u8) -> [u8; 32] {
    [seed; 32]
}

/// The account of `seed`, through which both XRPL schemes may settle, from
/// `state`.
fn new_account(seed: u8, state: &[u8]) -> XrplAccount {
    let schemes = [name(dassie_xrpl::scheme::SCHEME_ID), name(dsusd::SCHEME_ID)];
    // It trusts the fake ledger's validators.
    let config = dassie_xrpl::fake::with_publishers("", &dassie_xrpl::network::TESTNET);
    XrplAccount::new(account::FIRST, &schemes, &config, &entropy(seed), state).unwrap()
}

/// Dollars in internal units.
const fn usd(micros: u64) -> u64 {
    micros * UNITS_PER_MICRO
}

impl World {
    /// Nodes on a fresh fake ledger, each with the DSUSD scheme and, where
    /// `xrp` says, the XRP scheme on the same account; funded by the
    /// faucet.
    fn new(xrp: &[bool]) -> Self {
        let mut world = Self {
            ledger: FakeLedger::new(),
            nodes: Vec::new(),
            now: dassie_xrpl::fake::FIRST_CLOSE_MS,
            next_connection: 1,
            drop_lookups: false,
            refuse_own: None,
            tamper_lookup: None,
            lagging: false,
        };
        for (index, with_xrp) in xrp.iter().enumerate() {
            let seed = u8::try_from(index).unwrap() + 1;
            world.add(seed, *with_xrp);
        }
        world
    }

    /// As [`new`](Self::new), on a ledger past its chain's first flag
    /// ledger, so that every anchor a node keeps keeps one.
    fn past_a_flag_ledger(xrp: &[bool]) -> Self {
        let mut world = Self::new(&[]);
        for _ in 0..260 {
            world.now += 4_000;
            world
                .ledger
                .close_at(dassie_xrpl::unl::ledger_seconds(world.now));
        }
        for (index, with_xrp) in xrp.iter().enumerate() {
            let seed = u8::try_from(index).unwrap() + 1;
            world.add(seed, *with_xrp);
        }
        world
    }

    fn add(&mut self, seed: u8, with_xrp: bool) {
        self.add_with(seed, with_xrp, None);
    }

    /// Adds a node, which tells its account `xrp_funds` as the owner's
    /// funds on the XRP ledger, if set, rather than what its books say.
    fn add_with(&mut self, seed: u8, with_xrp: bool, xrp_funds: Option<u64>) {
        let node = name(&format!("node{seed}"));
        let account = new_account(seed, &[]);
        let address = account.account();
        self.ledger.fund(address);
        self.close();
        let xrp = with_xrp.then(|| XrplScheme::new(&account, node, None, &[]).unwrap());
        let dsusd = Some(DsusdScheme::new(&account, node, None, &[]).unwrap());
        self.nodes.push(Node {
            name: node,
            seed,
            account,
            xrp,
            dsusd,
            books: [Books::default(), Books::default()],
            connection: 0,
            online: false,
            xrp_funds,
            away: [false; 2],
        });
        let index = self.nodes.len() - 1;
        self.tell(index);
        self.connect(index);
    }

    /// Node `index`'s connection drops and is made again.
    fn reconnect(&mut self, index: usize) {
        let connection = self.nodes[index].connection;
        self.ledger.disconnect(connection);
        self.input(index, LedgerInput::Disconnected);
        self.connect(index);
    }

    fn address(&self, index: usize) -> AccountId {
        self.nodes[index].account.account()
    }

    fn connect(&mut self, index: usize) {
        self.next_connection += 1;
        self.nodes[index].connection = self.next_connection;
        self.nodes[index].online = true;
        self.input(index, LedgerInput::Connected);
    }

    fn input(&mut self, index: usize, input: LedgerInput<'_>) {
        // The node's clock reads the ledger's validations as fresh.
        self.now = self.now.max(self.ledger.now_ms());
        self.nodes[index].account.on_ledger(self.now, input);
        self.tell(index);
    }

    /// Hands every scheme on the account what it learnt, in the node's
    /// order, with the owner's funds first; books what they say, and
    /// carries out what the account asks.
    fn tell(&mut self, index: usize) {
        let now = self.now;
        let node = &mut self.nodes[index];
        let funds = [node.books[XRP].funds(), node.books[USD].funds()];
        let xrp_funds = node.xrp_funds.unwrap_or(funds[XRP]);
        if node.runs(XRP) {
            node.account
                .owner_funds(name(dassie_xrpl::scheme::SCHEME_ID), xrp_funds);
        }
        if node.runs(USD) {
            node.account.owner_funds(name(dsusd::SCHEME_ID), funds[USD]);
        }
        let mut booked: [Vec<Effect>; 2] = [Vec::new(), Vec::new()];
        let refused = self.refuse_own.filter(|ledger| {
            let scheme = scheme_name(*ledger);
            node.runs(*ledger)
                && node.account.news().iter().any(|news| match news {
                    News::Transaction {
                        ours: Some(ours), ..
                    }
                    | News::Unread { ours, .. } => ours.scheme == scheme,
                    News::Cancelled { scheme: of, .. } => *of == scheme,
                    _ => false,
                })
        });
        // What the books keep of the scheme they refuse: its checkpoint
        // stays from before.
        let kept = refused.map(|ledger| node.states().1[ledger].clone().unwrap());
        if let (Some(xrp), false) = (&mut node.xrp, node.away[XRP]) {
            xrp.on_asset(now, &mut node.account, &mut booked[XRP]);
        }
        if refused == Some(XRP) {
            node.stopped(XRP, kept.as_ref().unwrap(), &mut booked[XRP]);
        }
        if let (Some(dsusd), false) = (&mut node.dsusd, node.away[USD]) {
            dsusd.on_asset(now, &mut node.account, &mut booked[USD]);
        }
        if refused == Some(USD) {
            node.stopped(USD, kept.as_ref().unwrap(), &mut booked[USD]);
        }
        if refused.is_some() {
            self.refuse_own = None;
        }
        node.account.told();
        for (ledger, effects) in booked.into_iter().enumerate() {
            for effect in effects {
                // The line's reserve is XRP, off the DSUSD books.
                assert!(
                    ledger == XRP
                        || !matches!(
                            effect,
                            Effect::Reconcile {
                                cause: ReconcileCause::Reserve,
                                ..
                            }
                        ),
                    "{effect:?}"
                );
                node.books[ledger].apply(effect);
            }
        }
        node.check_invariants();
        self.carry_out(index);
    }

    /// Carries out what the account asks of its ledger.
    fn carry_out(&mut self, index: usize) {
        let mut asked = Vec::new();
        self.nodes[index].account.take_effects(&mut asked);
        for effect in asked {
            match effect {
                AssetEffect::Send { message } => self.request(index, &message),
                AssetEffect::Reconnect => self.reconnect(index),
                AssetEffect::Fund { body, .. } => {
                    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let to = AccountId::parse(request["destination"].as_str().unwrap()).unwrap();
                    self.ledger.fund(to);
                    self.input(index, LedgerInput::Funded(true));
                }
                AssetEffect::Fetch { url, .. } => {
                    let body = self.ledger.fetch(&url);
                    let fetched = LedgerInput::Fetched {
                        url: &url,
                        body: body.as_deref(),
                    };
                    self.input(index, fetched);
                }
                AssetEffect::SendTo { .. } => panic!("an XRPL account names no endpoints"),
            }
        }
    }

    fn request(&mut self, index: usize, message: &[u8]) {
        if !self.nodes[index].online {
            return;
        }
        let request: serde_json::Value = serde_json::from_slice(message).unwrap();
        let proof = request["command"] == "tx_proof";
        if self.drop_lookups && proof && request.get("ledger_index").is_none() {
            return;
        }
        // A lookup is its proof and the transaction's bytes, from `tx`.
        let lookup = proof || request["command"] == "tx";
        let mut reply = self.ledger.request(self.nodes[index].connection, message);
        if let (Some(tamper), true) = (self.tamper_lookup, lookup) {
            let mut answer: serde_json::Value = serde_json::from_slice(&reply).unwrap();
            tamper(&mut answer);
            reply = serde_json::to_vec(&answer).unwrap();
        }
        self.input(index, LedgerInput::Message(&reply));
    }

    /// Closes a ledger, delivers the streams, and checks every node's books
    /// against the ledger.
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
        self.check();
    }

    fn closes(&mut self, count: usize) {
        for _ in 0..count {
            self.close();
        }
    }

    /// Time passes, by `ms`, without a ledger closing.
    fn wait(&mut self, ms: u64) {
        self.now += ms;
        for index in 0..self.nodes.len() {
            if self.nodes[index].online {
                dassie_settlement::ExternalAsset::on_timer(
                    &mut self.nodes[index].account,
                    self.now,
                );
                self.tell(index);
            }
        }
    }

    /// More ledgers than the history bound close while node `index` is
    /// away, and its server forgets the ledgers past what it keeps.
    fn away_beyond_the_bound(&mut self, index: usize) {
        let connection = self.nodes[index].connection;
        self.ledger.disconnect(connection);
        self.input(index, LedgerInput::Disconnected);
        self.nodes[index].online = false;
        for _ in 0..account::HISTORY_BOUND + 300 {
            self.now += 4_000;
            self.ledger
                .close_at(dassie_xrpl::unl::ledger_seconds(self.now));
        }
    }

    /// The node's settlement check: the DSUSD scheme reads the issuer.
    fn settlement_check(&mut self, index: usize) {
        let node = &mut self.nodes[index];
        if let Some(dsusd) = &mut node.dsusd {
            dsusd.on_check(self.now, &mut node.account);
        }
        self.carry_out(index);
    }

    /// Every node's books match what the ledger holds: its line in whole
    /// millionths, and its XRP less the reserve where the XRP scheme runs.
    /// The ledger made and lost no XRP and no token.
    fn check(&self) {
        for node in &self.nodes {
            let address = node.account.account();
            if let (Some(dsusd), false) = (&node.dsusd, node.away[USD]) {
                assert_eq!(
                    node.books[USD].settlement,
                    i128::from(dsusd.balance()),
                    "{}",
                    node.name
                );
                if node.status(USD) == LedgerStatus::Synced && !self.lagging {
                    let held = self
                        .ledger
                        .token_balance(address, IssuedAsset::DSUSD)
                        .map_or(0, |value| value.floor_units(6).unwrap());
                    // Less what it holds of entry payments, which is off
                    // the books (spec/decisions/pay-to-enter.md).
                    assert_eq!(
                        i128::from(dsusd.balance()),
                        (held.max(0) * i128::from(UNITS_PER_MICRO)
                            - i128::from(dsusd.entries_held()))
                        .max(0),
                        "{}",
                        node.name
                    );
                }
            }
            if let (Some(xrp), false) = (&node.xrp, node.away[XRP]) {
                assert_eq!(
                    node.books[XRP].settlement,
                    i128::from(xrp.balance()),
                    "{}",
                    node.name
                );
                if node.status(XRP) == LedgerStatus::Synced && !self.lagging {
                    let balance = self.ledger.balance(address).unwrap();
                    let bookable = (balance * UNITS_PER_DROP).saturating_sub(xrp.reserve());
                    assert_eq!(xrp.balance(), bookable, "{}", node.name);
                }
            }
        }
        assert_eq!(self.ledger.total(), dassie_xrpl::MAX_DROPS);
        assert!(self.ledger.tokens_conserved());
    }

    /// Peers `a` and `b` on the scheme of `ledger`: `a` asks. Returns the
    /// states each keeps for the other.
    fn peer(&mut self, ledger: usize, a: usize, b: usize) -> (Vec<u8>, Vec<u8>) {
        let (name_a, name_b) = (self.nodes[a].name, self.nodes[b].name);
        let mut info = Vec::new();
        self.nodes[b].scheme(ledger).peering_info(&mut info);
        let mut request = Vec::new();
        self.nodes[a]
            .scheme(ledger)
            .create_peering_request(name_b, &info, &mut request)
            .unwrap();
        let state_at_b = self.nodes[b]
            .scheme(ledger)
            .accept_peering_request(name_a, &request)
            .unwrap();
        let state_at_a = self.nodes[a]
            .scheme(ledger)
            .finalize_peering_request(name_b, &info)
            .unwrap();
        self.nodes[b].scheme(ledger).on_peered(name_a, &state_at_b);
        self.nodes[a].scheme(ledger).on_peered(name_b, &state_at_a);
        (state_at_a, state_at_b)
    }

    /// `from` prepares a settlement of `amount` with `to` on the scheme of
    /// `ledger`.
    fn prepare(
        &mut self,
        ledger: usize,
        from: usize,
        to: usize,
        amount: u64,
        state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        let peer = self.nodes[to].name;
        let funds = self.nodes[from].books[XRP].funds();
        let node = &mut self.nodes[from];
        if node.runs(XRP) {
            let funds = node.xrp_funds.unwrap_or(funds);
            node.account
                .owner_funds(name(dassie_xrpl::scheme::SCHEME_ID), funds);
        }
        let mut rng = SeededRng::new(1);
        let prepared = if ledger == XRP {
            node.xrp.as_mut().unwrap().prepare_settlement(
                &mut rng,
                Some(&mut node.account),
                peer,
                amount,
                state,
            )
        } else {
            node.dsusd.as_mut().unwrap().prepare_settlement(
                &mut rng,
                Some(&mut node.account),
                peer,
                amount,
                state,
            )
        }?;
        node.books[ledger]
            .pending
            .insert(prepared.id, (peer, prepared.amount));
        Ok(prepared)
    }

    /// `from` settles `amount` with `to` on the scheme of `ledger`: prepared,
    /// then executed, as the node does once its pending transfer is on disk.
    fn settle(&mut self, ledger: usize, from: usize, to: usize, amount: u64, state: &[u8]) -> Name {
        let prepared = self.prepare(ledger, from, to, amount, state).unwrap();
        self.execute(ledger, from, prepared.id);
        prepared.id
    }

    fn execute(&mut self, ledger: usize, from: usize, id: Name) {
        let node = &mut self.nodes[from];
        let mut effects = Vec::new();
        if ledger == XRP {
            node.xrp.as_mut().unwrap().execute_settlement(
                Some(&mut node.account),
                id,
                &mut effects,
            );
        } else {
            node.dsusd.as_mut().unwrap().execute_settlement(
                Some(&mut node.account),
                id,
                &mut effects,
            );
        }
        for effect in effects {
            node.books[ledger].apply(effect);
        }
        self.carry_out(from);
    }

    /// The owner of node `index` pays `payment` out of its DSUSD line, held
    /// against the owner's funds on the books as the node holds it.
    fn pay_owner(
        &mut self,
        index: usize,
        payment: &dassie_settlement::OwnerPayment,
    ) -> Result<Name, SchemeError> {
        let funds = self.nodes[index].books[XRP].funds();
        let node = &mut self.nodes[index];
        if node.runs(XRP) {
            let funds = node.xrp_funds.unwrap_or(funds);
            node.account
                .owner_funds(name(dassie_xrpl::scheme::SCHEME_ID), funds);
        }
        let id = node
            .dsusd
            .as_mut()
            .unwrap()
            .pay_owner(Some(&mut node.account), payment)?;
        node.books[USD].owner_holds.insert(id, payment.amount);
        node.check_invariants();
        self.carry_out(index);
        Ok(id)
    }

    /// The issuer mints `amount` of test DSUSD to node `index`.
    fn mint(&mut self, index: usize, amount: &str) {
        let to = self.address(index);
        self.ledger
            .issue(to, IssuedAsset::DSUSD.amount(value(amount)))
            .unwrap();
    }

    /// Node `index` stops, as at a crash, and starts again from what its
    /// books kept, connecting anew; what it held pending is handed to its
    /// schemes.
    fn restart(&mut self, index: usize) {
        let connection = self.nodes[index].connection;
        self.ledger.disconnect(connection);
        let (account_state, schemes) = self.nodes[index].states();
        let node = &mut self.nodes[index];
        node.online = false;
        node.away = [false; 2];
        node.account = new_account(node.seed, &account_state);
        if let Some((balance, state)) = &schemes[XRP] {
            node.xrp =
                Some(XrplScheme::new(&node.account, node.name, Some(*balance), state).unwrap());
        }
        if let Some((balance, state)) = &schemes[USD] {
            node.dsusd =
                Some(DsusdScheme::new(&node.account, node.name, Some(*balance), state).unwrap());
        }
        self.tell(index);
        for ledger in [XRP, USD] {
            let pending: Vec<(Name, u64)> = self.nodes[index].books[ledger]
                .pending
                .iter()
                .map(|(id, (_, amount))| (*id, *amount))
                .collect();
            if pending.is_empty() {
                continue;
            }
            let node = &mut self.nodes[index];
            let mut effects = Vec::new();
            if ledger == XRP {
                node.xrp.as_mut().unwrap().reconcile(
                    Some(&mut node.account),
                    &pending,
                    &mut effects,
                );
            } else {
                node.dsusd.as_mut().unwrap().reconcile(
                    Some(&mut node.account),
                    &pending,
                    &mut effects,
                );
            }
            for effect in effects {
                node.books[ledger].apply(effect);
            }
        }
        self.connect(index);
    }

    /// Node `index` goes down: its connection drops, and it hears nothing
    /// until it restarts.
    fn down(&mut self, index: usize) {
        let connection = self.nodes[index].connection;
        self.ledger.disconnect(connection);
        self.nodes[index].online = false;
        self.input(index, LedgerInput::Disconnected);
    }

    /// The scheme of `ledger` leaves node `index`'s account, as the books
    /// stopping it or the owner removing it does: it is told nothing more,
    /// and its state stays as it was, as its checkpoint does.
    fn leave(&mut self, index: usize, ledger: usize, why: Detach) {
        let scheme = if ledger == XRP {
            name(dassie_xrpl::scheme::SCHEME_ID)
        } else {
            name(dsusd::SCHEME_ID)
        };
        let node = &mut self.nodes[index];
        node.away[ledger] = true;
        node.account.detach(scheme, why);
    }

    /// The owner adds the scheme of `ledger` to node `index` again: made
    /// from what its books kept, it hands back what it holds pending, and
    /// the account connects anew, so that it reads from the scheme's place.
    fn add_again(&mut self, index: usize, ledger: usize) {
        let node = &mut self.nodes[index];
        node.away[ledger] = false;
        let mut effects = Vec::new();
        let pending: Vec<(Name, u64)> = node.books[ledger]
            .pending
            .iter()
            .map(|(id, (_, amount))| (*id, *amount))
            .collect();
        if ledger == XRP {
            let (balance, state) = {
                let xrp = node.xrp.as_ref().unwrap();
                (xrp.balance(), xrp.state())
            };
            let mut xrp = XrplScheme::new(&node.account, node.name, Some(balance), &state).unwrap();
            xrp.reconcile(Some(&mut node.account), &pending, &mut effects);
            node.xrp = Some(xrp);
        } else {
            let (balance, state) = (node.dsusd().balance(), node.dsusd().state());
            let mut dsusd =
                DsusdScheme::new(&node.account, node.name, Some(balance), &state).unwrap();
            dsusd.reconcile(Some(&mut node.account), &pending, &mut effects);
            node.dsusd = Some(dsusd);
        }
        for effect in effects {
            node.books[ledger].apply(effect);
        }
        self.tell(index);
        self.reconnect(index);
    }
}

/// What the faucet gives above the reserve, in internal units.
const FUNDED: i128 = ((FAUCET_DROPS - RESERVE_BASE) * UNITS_PER_DROP) as i128;

/// Nodes on DSUSD alone, synced with their lines set, and peered `0 → 1`.
fn synced(count: usize) -> (World, Vec<u8>, Vec<u8>) {
    let mut world = World::new(&vec![false; count]);
    world.closes(3);
    for node in &world.nodes {
        assert_eq!(node.status(USD), LedgerStatus::Synced, "{}", node.name);
        assert!(node.dsusd().has_synced());
    }
    let (a, b) = world.peer(USD, 0, 1);
    (world, a, b)
}

// spec/areas/xrpl.md#the-token-scheme: before the scheme counts as synced, the
// account has a line to the issuer with rippling disabled on its side and the
// largest limit: it signs the TrustSet once the account holds XRP for it, and
// takes no peer before the ledger shows the line. Its fee and reserve are gas,
// off the books without the XRP scheme.
#[test]
fn the_line_is_set_before_the_scheme_syncs() {
    let mut world = World::new(&[false]);
    let address = world.address(0);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Syncing);
    assert!(!world.nodes[0].dsusd().has_synced());
    assert!(
        world
            .ledger
            .line(address, dsusd::ISSUER, dsusd::CURRENCY)
            .is_none()
    );
    world.close();
    let line = world
        .ledger
        .line(address, dsusd::ISSUER, dsusd::CURRENCY)
        .unwrap()
        .view(address)
        .unwrap();
    assert!(line.ours.no_ripple);
    assert_eq!(line.limit, TokenValue::MAX);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    assert!(world.nodes[0].dsusd().has_synced());
    let books = &world.nodes[0].books[USD];
    assert_eq!((books.settlement, books.fees, books.suspense), (0, 0, 0));
    // The account spent the fee and holds the line's reserve, in XRP.
    assert_eq!(world.ledger.balance(address), Some(FAUCET_DROPS - FEE_BASE));
    assert_eq!(world.ledger.owner_count(address), Some(1));
    // The wallet sees both holdings, as the ledger says.
    let holdings = world.nodes[0].account.holdings();
    assert_eq!(holdings.len(), 2);
    assert_eq!(holdings[1].code, "DSUSD");
    assert_eq!(
        holdings[1].issuer.as_deref(),
        Some(dsusd::ISSUER.to_address().as_str())
    );
    assert_eq!(holdings[0].reserve, u128::from(RESERVE_BASE + RESERVE_INC));
}

// An account without the XRP to hold the line signs no TrustSet: the
// scheme waits, unsynced, and sets the line once the owner sends XRP.
#[test]
fn no_line_without_xrp_for_it() {
    let mut world = World {
        ledger: FakeLedger::new(),
        nodes: Vec::new(),
        now: dassie_xrpl::fake::FIRST_CLOSE_MS,
        next_connection: 1,
        drop_lookups: false,
        refuse_own: None,
        tamper_lookup: None,
        lagging: false,
    };
    let seed = 9;
    let address = Wallet::from_entropy(&entropy(seed)).account();
    world.ledger.deposit(address, RESERVE_BASE + 5);
    world.ledger.close();
    let node = name("poor");
    world.nodes.push(Node {
        name: node,
        seed,
        dsusd: Some(DsusdScheme::new(&new_account(seed, &[]), node, None, &[]).unwrap()),
        account: new_account(seed, &[]),
        xrp: None,
        books: [Books::default(), Books::default()],
        connection: 0,
        online: false,
        xrp_funds: None,
        away: [false; 2],
    });
    world.tell(0);
    world.connect(0);
    world.closes(3);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Syncing);
    assert_eq!(world.ledger.owner_count(address), Some(0));
    world.ledger.deposit(address, 2 * RESERVE_INC);
    world.closes(3);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
}

// spec/areas/xrpl.md#the-token-scheme: a line made outside the node, with
// rippling enabled on our side and a lower limit, is corrected, and the scheme
// syncs only once the ledger shows it so. What the line held then is the
// owner's deposit, in whole millionths.
#[test]
fn a_line_made_outside_the_node_is_corrected() {
    let mut ledger = FakeLedger::new();
    let seed = 3;
    let wallet = Wallet::from_entropy(&entropy(seed));
    let address = wallet.account();
    ledger.fund(address);
    ledger.close();
    let info = serde_json::json!({ "command": "account_info", "account": address.to_address() });
    let reply: serde_json::Value =
        serde_json::from_slice(&ledger.request(0, info.to_string().as_bytes())).unwrap();
    let sequence = u32::try_from(
        reply["result"]["account_data"]["Sequence"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let signed = TrustSet {
        account: address,
        limit: IssuedAsset::DSUSD.amount(value("100")),
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(&wallet);
    let submit =
        serde_json::json!({ "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&signed.blob) });
    ledger.request(0, submit.to_string().as_bytes());
    ledger.close();
    ledger
        .issue(address, IssuedAsset::DSUSD.amount(value("12.3456789")))
        .unwrap();
    ledger.close();
    let view = ledger
        .line(address, dsusd::ISSUER, dsusd::CURRENCY)
        .unwrap()
        .view(address)
        .unwrap();
    assert_eq!(view.limit, value("100"));

    let mut world = World {
        ledger,
        nodes: Vec::new(),
        now: dassie_xrpl::fake::FIRST_CLOSE_MS,
        next_connection: 1,
        drop_lookups: false,
        refuse_own: None,
        tamper_lookup: None,
        lagging: false,
    };
    let node = name("outside");
    world.nodes.push(Node {
        name: node,
        seed,
        dsusd: Some(DsusdScheme::new(&new_account(seed, &[]), node, None, &[]).unwrap()),
        account: new_account(seed, &[]),
        xrp: None,
        books: [Books::default(), Books::default()],
        connection: 0,
        online: false,
        xrp_funds: None,
        away: [false; 2],
    });
    world.tell(0);
    world.connect(0);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Syncing);
    assert!(!world.nodes[0].dsusd().has_synced());
    world.close();
    let view = world
        .ledger
        .line(address, dsusd::ISSUER, dsusd::CURRENCY)
        .unwrap()
        .view(address)
        .unwrap();
    assert!(view.ours.no_ripple);
    assert_eq!(view.limit, TokenValue::MAX);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    // 12.345678, the rest stays on the line.
    assert_eq!(world.nodes[0].books[USD].owner, i128::from(usd(12_345_678)));
    // What was left below a unit and a tenth of a millionth more add up to
    // one: a deposit of it.
    world.mint(0, "0.0000001");
    world.close();
    assert_eq!(world.nodes[0].books[USD].owner, i128::from(usd(12_345_679)));
}

// spec/areas/xrpl.md#the-token-scheme: two nodes settle DSUSD. The payer's
// settlement is posted, the payee credits what was delivered, once, and both
// books hold their lines; the fees are gas, off the books.
#[test]
fn two_nodes_settle_dsusd() {
    let (mut world, state_a, state_b) = synced(2);
    world.mint(0, "50");
    world.close();
    assert_eq!(world.nodes[0].books[USD].owner, i128::from(usd(50_000_000)));
    // Less than a millionth is not settled.
    let id = world.settle(USD, 0, 1, usd(12_500_000) + 999, &state_a);
    world.close();
    let (a, b) = (&world.nodes[0].books[USD], &world.nodes[1].books[USD]);
    assert_eq!(a.finalized, [id]);
    assert_eq!(a.peers[&world.nodes[1].name], -i128::from(usd(12_500_000)));
    assert_eq!(b.peers[&world.nodes[0].name], i128::from(usd(12_500_000)));
    assert_eq!(b.credited.len(), 1);
    assert_eq!((a.fees, b.fees), (0, 0));
    assert_eq!(
        world
            .ledger
            .token_balance(world.address(1), IssuedAsset::DSUSD),
        Some(value("12.5"))
    );
    // And back.
    let id = world.settle(USD, 1, 0, usd(2_000_001), &state_b);
    world.close();
    assert_eq!(world.nodes[1].books[USD].finalized, [id]);
    assert_eq!(
        world.nodes[0].books[USD].peers[&world.nodes[1].name],
        -i128::from(usd(10_499_999))
    );
}

// As in the XRP scheme, a peer's address goes when its settlement channel
// closes (spec/decisions/closing-channels.md): a Payment from it after that
// is the owner's deposit. The state version changes with it, though the
// balance does not, so the node writes the state without the address and a
// restart does not bring it back.
#[test]
fn a_closed_peers_address_is_forgotten() {
    let (mut world, _, state_b) = synced(2);
    let peer = world.nodes[1].name;
    let address = world.address(1);
    world.mint(1, "50");
    world.close();
    world.settle(USD, 1, 0, usd(1_000_000), &state_b);
    world.close();
    let settled = i128::from(usd(1_000_000));
    assert_eq!(world.nodes[0].books[USD].peers.get(&peer), Some(&settled));
    let (version, balance) = (
        world.nodes[0].dsusd().state_version(),
        world.nodes[0].dsusd().balance(),
    );
    world.nodes[0].scheme(USD).on_closed(peer);
    world.nodes[0].check_invariants();
    assert_eq!(world.nodes[0].dsusd().balance(), balance);
    assert_ne!(world.nodes[0].dsusd().state_version(), version);
    let state = world.nodes[0].dsusd().state();
    assert!(!state.windows(20).any(|bytes| bytes == address.0));
    // A channel closed again changes nothing more.
    let version = world.nodes[0].dsusd().state_version();
    world.nodes[0].scheme(USD).on_closed(peer);
    assert_eq!(world.nodes[0].dsusd().state_version(), version);
    // The node restarts from the state it wrote then.
    world.restart(0);
    world.closes(2);
    world.settle(USD, 1, 0, usd(2_000_000), &state_b);
    world.close();
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.peers.get(&peer), Some(&settled));
    assert_eq!(books.owner, i128::from(usd(2_000_000)));
}

// spec/decisions/pay-to-enter.md: the token scheme holds an entry payment as
// the XRP scheme does: a Payment of the token with its memo from a node not
// yet peered, of at least one largest packet, held off the books, bound to
// the node and its address, and booked as its settlement once, when it
// peers.
#[test]
fn a_token_payment_before_peering_is_held_until_its_node_peers() {
    const ENTRY: u64 = 100_000_000;
    let mut world = World::new(&[false, false]);
    world.closes(3);
    world.nodes[0]
        .dsusd
        .as_mut()
        .unwrap()
        .take_entries_from(ENTRY);
    world.mint(1, "5");
    world.close();
    let newcomer = world.nodes[1].name;
    let (mut info, mut request) = (Vec::new(), Vec::new());
    world.nodes[0].scheme(USD).peering_info(&mut info);
    let uplink = world.nodes[0].name;
    world.nodes[1]
        .scheme(USD)
        .create_peering_request(uplink, &info, &mut request)
        .unwrap();
    world.settle(USD, 1, 0, ENTRY, &info);
    world.close();
    let entry = |world: &mut World, data: &[u8]| world.nodes[0].scheme(USD).entry(newcomer, data);
    assert_eq!(world.nodes[0].dsusd().entries_held(), ENTRY);
    assert_eq!(world.nodes[0].books[USD].owner, 0, "no deposit");
    assert_eq!(entry(&mut world, &request), EntryPayment::Held);
    assert_eq!(entry(&mut world, &info), EntryPayment::Missing);
    world.peer(USD, 1, 0);
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme(USD)
        .take_entry(newcomer, &mut effects);
    for effect in effects {
        world.nodes[0].books[USD].apply(effect);
    }
    world.nodes[0].check_invariants();
    world.check();
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.peers[&newcomer], i128::from(ENTRY));
    assert_eq!(books.credited.len(), 1);
    assert_eq!(world.nodes[0].dsusd().entries_held(), 0);
    assert_eq!(entry(&mut world, &request), EntryPayment::Missing);
}

/// Two nodes on DSUSD, node 0 taking entry payments of `entry` and holding
/// one from node 1, of which the issuer then clawed back all but `left`
/// millionths, as a clawback can: less than the entry held. Returns node
/// 1's request data, as it peers with node 0.
fn line_drained_with_entry(entry: u64, left: u64) -> (World, Vec<u8>) {
    let mut world = World::new(&[false, false]);
    world.closes(3);
    world.nodes[0]
        .dsusd
        .as_mut()
        .unwrap()
        .take_entries_from(entry);
    world.mint(1, "5");
    world.close();
    let (mut info, mut request) = (Vec::new(), Vec::new());
    world.nodes[0].scheme(USD).peering_info(&mut info);
    let uplink = world.nodes[0].name;
    world.nodes[1]
        .scheme(USD)
        .create_peering_request(uplink, &info, &mut request)
        .unwrap();
    world.settle(USD, 1, 0, entry, &info);
    world.close();
    assert_eq!(world.nodes[0].dsusd().entries_held(), entry);
    let clawed = entry / UNITS_PER_MICRO - left;
    let amount = TokenValue::from_units(clawed, 6).unwrap();
    world
        .ledger
        .claw_back(world.address(0), IssuedAsset::DSUSD.amount(amount))
        .unwrap();
    world.close();
    assert_eq!(
        world.nodes[0].dsusd().balance(),
        0,
        "the books hold nothing"
    );
    (world, request)
}

// spec/decisions/pay-to-enter.md: on a token line as on the XRP scheme, an
// entry payment taken once the line no longer holds all of it credits its
// node what it paid, and the books match what is left through suspense,
// before and after a restart.
#[test]
fn a_token_entry_payment_taken_short_is_matched_through_suspense() {
    const ENTRY: u64 = 100_000_000;
    const LEFT: u64 = 40_000;
    let (mut world, _) = line_drained_with_entry(ENTRY, LEFT);
    let newcomer = world.nodes[1].name;
    world.peer(USD, 1, 0);
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme(USD)
        .take_entry(newcomer, &mut effects);
    for effect in effects {
        world.nodes[0].books[USD].apply(effect);
    }
    world.nodes[0].check_invariants();
    world.check();
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.peers[&newcomer], i128::from(ENTRY));
    assert_eq!(books.settlement, i128::from(usd(LEFT)));
    world.restart(0);
    world.closes(2);
    assert_eq!(world.nodes[0].dsusd().balance(), usd(LEFT));
    assert_eq!(world.nodes[0].books[USD].credited.len(), 1);
}

// spec/decisions/pay-to-enter.md: on a token line, an entry payment whose
// hold is over once the line no longer holds all of it is the owner's
// deposit, and the books match what is left, before and after a restart.
#[test]
fn a_token_entry_payment_expired_short_is_matched_through_suspense() {
    const ENTRY: u64 = 100_000_000;
    const LEFT: u64 = 40_000;
    let (mut world, _) = line_drained_with_entry(ENTRY, LEFT);
    let owner = world.nodes[0].books[USD].owner;
    for _ in 0..ENTRY_HOLD_LEDGERS {
        world.now += 4_000;
        world
            .ledger
            .close_at(dassie_xrpl::unl::ledger_seconds(world.now));
    }
    world.closes(2);
    assert_eq!(world.nodes[0].dsusd().entries_held(), 0);
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.owner, owner + i128::from(ENTRY));
    assert_eq!(books.settlement, i128::from(usd(LEFT)));
    world.restart(0);
    world.closes(2);
    assert_eq!(world.nodes[0].dsusd().balance(), usd(LEFT));
    assert!(world.nodes[0].books[USD].credited.is_empty());
}

// spec/areas/xrpl.md#the-token-scheme: what is not a peer's settlement moves
// the line as a deposit or a withdrawal, an issuer's clawback included; a
// Payment with our memo from an address that is not the peer's is a deposit;
// XRP and other issuers' tokens move nothing on this ledger.
#[test]
fn the_line_books_deposits_withdrawals_and_clawbacks() {
    let (mut world, _, _) = synced(3);
    world.mint(0, "20");
    world.close();
    world
        .ledger
        .claw_back(world.address(0), IssuedAsset::DSUSD.amount(value("7.25")))
        .unwrap();
    world.close();
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.owner, i128::from(usd(12_750_000)));
    // Node 2 pays node 0 with our memo, naming node 1: not node 1's
    // address, so a deposit.
    world.mint(2, "3");
    world.close();
    let mut info = Vec::new();
    world.nodes[0]
        .dsusd
        .as_ref()
        .unwrap()
        .peering_info(&mut info);
    let impostor = world.nodes[1].name;
    let node = &mut world.nodes[2];
    let (balance, state) = (node.dsusd().balance(), node.dsusd().state());
    node.dsusd = Some(DsusdScheme::new(&node.account, impostor, Some(balance), &state).unwrap());
    world.reconnect(2);
    world.settle(USD, 2, 0, usd(1_000_000), &info);
    world.close();
    let books = &world.nodes[0].books[USD];
    assert!(books.credited.is_empty());
    assert_eq!(books.owner, i128::from(usd(13_750_000)));
    // XRP moves nothing here.
    world.ledger.deposit(world.address(0), 5_000_000);
    world.close();
    assert_eq!(world.nodes[0].books[USD].owner, i128::from(usd(13_750_000)));
}

// spec/areas/xrpl.md#the-token-scheme: the issuer's freezes. A frozen line
// settles nothing but stays synced and still receives; a deep-frozen line, or a
// global freeze read at the settlement check, cannot receive either; a transfer
// fee also stops settling. Lifted, it settles again.
#[test]
fn freezes_stop_settling_and_receiving() {
    let (mut world, state_a, _) = synced(2);
    world.mint(0, "10");
    world.close();
    world.settlement_check(0);
    assert!(world.prepare(USD, 0, 1, usd(1), &state_a).is_ok());
    // The prepared one is never executed: dropped as the node would.
    let pending: Vec<Name> = world.nodes[0].books[USD].pending.keys().copied().collect();
    for id in pending {
        let node = &mut world.nodes[0];
        node.dsusd
            .as_mut()
            .unwrap()
            .discard_settlement(Some(&mut node.account), id);
        node.books[USD].pending.remove(&id);
    }
    let address = world.address(0);
    let unavailable = |world: &mut World| {
        world.prepare(USD, 0, 1, usd(1), &state_a).unwrap_err() == SchemeError::Unavailable
    };

    world
        .ledger
        .freeze(address, IssuedAsset::DSUSD, true)
        .unwrap();
    world.close();
    assert!(unavailable(&mut world));
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    assert!(world.nodes[0].dsusd().can_receive());

    world
        .ledger
        .deep_freeze(address, IssuedAsset::DSUSD, true)
        .unwrap();
    world.close();
    assert!(unavailable(&mut world));
    assert!(!world.nodes[0].dsusd().can_receive());
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);

    world
        .ledger
        .freeze(address, IssuedAsset::DSUSD, false)
        .unwrap();
    world.close();
    assert!(world.nodes[0].dsusd().can_receive());

    // A global freeze changes only the issuer's account: seen at the next
    // settlement check, proven at the anchor.
    world.ledger.global_freeze(dsusd::ISSUER, true).unwrap();
    world.close();
    assert!(world.nodes[0].dsusd().can_receive());
    world.settlement_check(0);
    assert!(!world.nodes[0].dsusd().can_receive());
    assert!(unavailable(&mut world));
    world.ledger.global_freeze(dsusd::ISSUER, false).unwrap();
    world.close();
    world.settlement_check(0);
    assert!(world.nodes[0].dsusd().can_receive());

    // A transfer fee no DSUSD node pays: no peer can pay us either
    // (spec/areas/settlement.md#when-peers-cannot-pay-us).
    world
        .ledger
        .set_transfer_rate(dsusd::ISSUER, 1_002_000_000)
        .unwrap();
    world.close();
    world.settlement_check(0);
    assert!(unavailable(&mut world));
    assert!(!world.nodes[0].dsusd().can_receive());
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    world.ledger.set_transfer_rate(dsusd::ISSUER, 0).unwrap();
    world.close();
    world.settlement_check(0);
    assert!(world.nodes[0].dsusd().can_receive());
    let id = world.settle(USD, 0, 1, usd(1_000_000), &state_a);
    world.close();
    assert_eq!(world.nodes[0].books[USD].finalized, [id]);
}

// spec/designs/xrpl-validation.md#4-what-is-proven-and-how: the issuer's
// account is taken only as proven at the anchor, never on a server's word:
// a server behind the anchor cannot prove it there, and nothing is told.
#[test]
fn an_old_answer_of_the_issuer_is_not_taken() {
    let (mut world, _, _) = synced(2);
    world.ledger.global_freeze(dsusd::ISSUER, true).unwrap();
    world.close();
    let behind = world.ledger.clone();
    world.ledger.global_freeze(dsusd::ISSUER, false).unwrap();
    world.closes(2);
    world.settlement_check(0);
    assert!(world.nodes[0].dsusd().can_receive());
    // A server behind says the token is still frozen: not taken.
    let mut current = std::mem::replace(&mut world.ledger, behind);
    world.settlement_check(0);
    assert!(world.nodes[0].dsusd().can_receive());
    std::mem::swap(&mut world.ledger, &mut current);
}

// spec/decisions/external-assets.md: on an account the XRP
// scheme settles from too, the two never take the same sequence, and every
// XRP fee is booked once, on the XRP ledger: the DSUSD scheme's as gas,
// its line's reserve reconciled there. Its books match the account's XRP
// less the reserve, and the DSUSD books the line.
#[test]
fn both_xrpl_schemes_share_one_account() {
    let mut world = World::new(&[true, true]);
    world.closes(3);
    for index in 0..2 {
        assert_eq!(world.nodes[index].status(XRP), LedgerStatus::Synced);
        assert_eq!(world.nodes[index].status(USD), LedgerStatus::Synced);
    }
    let (xrp_a, _) = world.peer(XRP, 0, 1);
    let (usd_a, _) = world.peer(USD, 0, 1);
    world.mint(0, "100");
    world.close();
    let mut ids = Vec::new();
    for round in 0..4 {
        // Both schemes sign in one ledger: consecutive sequences.
        ids.push(world.settle(XRP, 0, 1, 1_000_000 + round, &xrp_a));
        ids.push(world.settle(USD, 0, 1, usd(1_000_000 + round), &usd_a));
        world.close();
    }
    world.closes(2);
    let address = world.address(0);
    let history = api::account_tx_result(
        &serde_json::from_slice::<serde_json::Value>(
            &world.ledger.request(
                0,
                serde_json::json!({ "command": "account_tx", "account": address.to_address() })
                    .to_string()
                    .as_bytes(),
            ),
        )
        .unwrap()["result"],
    )
    .unwrap()
    .transactions;
    let ours: Vec<&api::TxRecord> = history.iter().filter(|t| t.account == address).collect();
    assert!(ours.iter().all(|t| t.succeeded()), "{ours:?}");
    let sequences: BTreeSet<u32> = ours.iter().map(|t| t.sequence).collect();
    assert_eq!(sequences.len(), ours.len(), "a sequence used twice");
    // Every fee the account paid: the TrustSet's, four XRP settlements',
    // four DSUSD settlements', each booked once on the XRP ledger.
    let paid: u64 = ours.iter().map(|t| t.fee).sum();
    assert_eq!(ours.len(), 9);
    let books = &world.nodes[0].books[XRP];
    assert_eq!(books.fees, i128::from(paid * UNITS_PER_DROP));
    assert_eq!(books.fee_ids.len(), 9);
    assert_eq!(world.nodes[0].books[USD].fees, 0);
    // The line's reserve went out through suspense, as the reserve's, which
    // is expected; nothing else did.
    let reserve = -i128::from(RESERVE_INC * UNITS_PER_DROP);
    assert_eq!(books.suspense, reserve);
    assert_eq!(books.reconciled, [(reserve, ReconcileCause::Reserve)]);
    assert_eq!(books.owner, FUNDED);
    assert_eq!(world.nodes[0].books[XRP].finalized.len(), 4);
    assert_eq!(world.nodes[0].books[USD].finalized.len(), 4);
}

// spec/decisions/external-assets.md: on a shared account the
// gas is the owner's XRP on the XRP ledger: without funds there the DSUSD
// scheme signs no TrustSet and settles nothing, though the account holds
// XRP.
#[test]
fn gas_on_a_shared_account_is_the_owners_to_spend() {
    let mut world = World {
        ledger: FakeLedger::new(),
        nodes: Vec::new(),
        now: dassie_xrpl::fake::FIRST_CLOSE_MS,
        next_connection: 1,
        drop_lookups: false,
        refuse_own: None,
        tamper_lookup: None,
        lagging: false,
    };
    world.add_with(1, true, Some(0));
    world.closes(3);
    assert_eq!(world.nodes[0].status(XRP), LedgerStatus::Synced);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Syncing);
    let address = world.address(0);
    assert_eq!(world.ledger.owner_count(address), Some(0));
    // Funds for the fee and the reserve, and no more.
    world.nodes[0].xrp_funds = Some((FEE_BASE + RESERVE_INC) * UNITS_PER_DROP);
    world.closes(2);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    world.add(2, true);
    world.closes(3);
    let (usd_a, _) = world.peer(USD, 0, 1);
    world.mint(0, "5");
    world.close();
    world.nodes[0].xrp_funds = Some(FEE_BASE * UNITS_PER_DROP - 1);
    assert_eq!(
        world
            .prepare(USD, 0, 1, usd(1_000_000), &usd_a)
            .unwrap_err(),
        SchemeError::Unavailable
    );
    world.nodes[0].xrp_funds = Some(FEE_BASE * UNITS_PER_DROP);
    world.settle(USD, 0, 1, usd(1_000_000), &usd_a);
    world.close();
    assert_eq!(world.nodes[0].books[USD].finalized.len(), 1);
}

// spec/areas/xrpl.md#state-and-restarts: a node that stops with a DSUSD
// settlement on its way and starts again submits it again and posts it
// once; one that validated while it was down is found in the history and
// posted once. Nothing is credited twice.
#[test]
fn a_restart_mid_settlement_posts_it_once() {
    let (mut world, state_a, _) = synced(2);
    world.mint(0, "10");
    world.close();
    // Prepared and booked, never sent: the node stops before executing.
    let prepared = world.prepare(USD, 0, 1, usd(1_000_000), &state_a).unwrap();
    world.restart(0);
    world.closes(2);
    assert_eq!(world.nodes[0].books[USD].finalized, [prepared.id]);
    // Executed and submitted, validated while the node is down.
    let id = world.settle(USD, 0, 1, usd(2_000_000), &state_a);
    let connection = world.nodes[0].connection;
    world.ledger.disconnect(connection);
    world.nodes[0].online = false;
    world.input(0, LedgerInput::Disconnected);
    world.closes(2);
    world.restart(0);
    world.closes(2);
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.finalized, [prepared.id, id]);
    assert_eq!(
        books.peers[&world.nodes[1].name],
        -i128::from(usd(3_000_000))
    );
    let books = &world.nodes[1].books[USD];
    assert_eq!(books.credited.len(), 2);
    assert_eq!(
        books.peers[&world.nodes[0].name],
        i128::from(usd(3_000_000))
    );
}

// spec/areas/xrpl.md#the-token-scheme: a transaction read again after a
// restart, from the cursor on, is booked by what it did to the line, from its
// own metadata: one that moved no DSUSD, such as the issuer's freeze while the
// node was down, books nothing, though the scheme has not been told the line
// yet. The node simulation found a restart that booked the whole line as a
// deposit here, and took it back through suspense.
#[test]
fn a_restart_books_what_a_transaction_did_to_the_line_by_its_metadata() {
    let (mut world, _, _) = synced(2);
    world.mint(0, "10");
    world.closes(2);
    let held = i128::from(usd(10_000_000));
    assert_eq!(world.nodes[0].books[USD].owner, held);
    let connection = world.nodes[0].connection;
    world.ledger.disconnect(connection);
    world.nodes[0].online = false;
    world.input(0, LedgerInput::Disconnected);
    let address = world.address(0);
    world
        .ledger
        .freeze(address, IssuedAsset::DSUSD, true)
        .unwrap();
    world.closes(2);
    world.restart(0);
    world.closes(3);
    let books = &world.nodes[0].books[USD];
    assert_eq!((books.owner, books.suspense), (held, 0));
    assert_eq!(books.settlement, held);
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
}

// spec/areas/xrpl.md#the-token-scheme: a testnet reset takes the line and what
// it held: written off through suspense, and the line made again once the
// account and the issuer are back.
#[test]
fn a_reset_writes_off_the_line_and_makes_it_again() {
    let (mut world, _, _) = synced(2);
    world.mint(0, "4");
    world.close();
    world.ledger.reset();
    world.closes(6);
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.suspense, -i128::from(usd(4_000_000)));
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    let address = world.address(0);
    assert!(
        world
            .ledger
            .line(address, dsusd::ISSUER, dsusd::CURRENCY)
            .is_some()
    );
}

// A Payment of XRP signed by hand on the shared account moves nothing on
// the DSUSD ledger, and the XRP scheme books it as a withdrawal, as ever:
// only what a scheme on the account signed is booked as gas.
#[test]
fn xrp_paid_by_hand_is_a_withdrawal_and_moves_no_dsusd() {
    let mut world = World::new(&[true, true]);
    world.closes(3);
    let (_, _) = world.peer(USD, 0, 1);
    let wallet = Wallet::from_entropy(&entropy(world.nodes[0].seed));
    let address = wallet.account();
    let info = serde_json::json!({ "command": "account_info", "account": address.to_address() });
    let reply: serde_json::Value =
        serde_json::from_slice(&world.ledger.request(0, info.to_string().as_bytes())).unwrap();
    let sequence = u32::try_from(
        reply["result"]["account_data"]["Sequence"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let signed = Payment {
        account: address,
        destination: world.address(1),
        destination_tag: None,
        amount: Amount::Xrp(3_000_000),
        send_max: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(world.ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(&wallet);
    let submit =
        serde_json::json!({ "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&signed.blob) });
    world.ledger.request(0, submit.to_string().as_bytes());
    let owner = world.nodes[0].books[XRP].owner;
    world.closes(2);
    let books = &world.nodes[0].books[XRP];
    assert_eq!(
        books.owner,
        owner - i128::from((3_000_000 + FEE_BASE) * UNITS_PER_DROP)
    );
    assert_eq!(world.nodes[0].books[USD].owner, 0);
}

// spec/areas/xrpl.md#the-token-scheme: what moved the line while the node was
// down is booked after the restart by what each transaction did to the line, as
// its metadata says, never against a line the scheme has not seen since it
// started: the issuer's mint is a deposit of what it minted, its clawback a
// withdrawal of what it took, and nothing goes through suspense; what is left
// below a unit adds up with what comes next.
#[test]
fn what_moved_the_line_while_down_is_booked_by_the_line() {
    let (mut world, _, _) = synced(2);
    world.mint(0, "10");
    world.close();
    world.down(0);
    world.mint(0, "5.0000003");
    world.close();
    let address = world.address(0);
    world
        .ledger
        .claw_back(address, IssuedAsset::DSUSD.amount(value("2")))
        .unwrap();
    world.close();
    world.restart(0);
    world.closes(2);
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.deposits, [usd(10_000_000), usd(5_000_000)]);
    assert_eq!(books.withdrawals, [usd(2_000_000)]);
    assert_eq!(books.owner, i128::from(usd(13_000_000)));
    assert_eq!(books.suspense, 0);
    world.down(0);
    world.mint(0, "0.0000007");
    world.close();
    world.restart(0);
    world.closes(2);
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.deposits, [usd(10_000_000), usd(5_000_000), usd(1)]);
    assert_eq!(books.suspense, 0);
}

// Found in review: a line deleted while the node was down, which the
// anchor proves absent, has no chain from the anchor. A peer's settlement
// on it, which changed the line alone, is still found: the owner's
// TrustSet that deleted it changed the root, whose chain finds it, and
// its metadata names what changed the line before. The issuer's clawback
// empties the line first, as a line holding tokens is never deleted.
// Both are booked, and nothing goes through suspense.
#[test]
fn a_settlement_on_a_line_deleted_while_down_is_found() {
    let (mut world, state_a, _) = synced(2);
    world.mint(0, "10");
    world.close();
    world.down(1);
    world.settle(USD, 0, 1, usd(2_000_000), &state_a);
    world.close();
    let address = world.address(1);
    world
        .ledger
        .claw_back(address, IssuedAsset::DSUSD.amount(value("2")))
        .unwrap();
    world.close();
    let wallet = Wallet::from_entropy(&entropy(world.nodes[1].seed));
    let info = serde_json::json!({ "command": "account_info", "account": address.to_address() });
    let reply: serde_json::Value =
        serde_json::from_slice(&world.ledger.request(0, info.to_string().as_bytes())).unwrap();
    let sequence = u32::try_from(
        reply["result"]["account_data"]["Sequence"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let deleting = TrustSet {
        account: address,
        limit: IssuedAsset::DSUSD.amount(TokenValue::ZERO),
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(world.ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(&wallet);
    let submit = serde_json::json!({
        "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&deleting.blob),
    });
    world.ledger.request(0, submit.to_string().as_bytes());
    world.close();
    assert!(
        world
            .ledger
            .line(address, dsusd::ISSUER, dsusd::CURRENCY)
            .is_none()
    );
    world.restart(1);
    world.closes(2);
    let books = &world.nodes[1].books[USD];
    assert_eq!(books.credited.len(), 1);
    assert_eq!(
        books.peers[&world.nodes[0].name],
        i128::from(usd(2_000_000))
    );
    assert_eq!(books.withdrawals, [usd(2_000_000)]);
    assert_eq!((books.suspense, books.settlement), (0, 0));
    assert_eq!(world.nodes[1].status(USD), LedgerStatus::Synced);
}

// spec/areas/xrpl.md#following-the-account: a testnet reset while one
// scheme on the account is away, stopped or removed, and the other runs on
// and is told. The one away missed the write-off: back, after a restart or
// added again, it writes off what it held through suspense and drops its
// cursor on the chain gone, and the account syncs on the new chain for
// both; the DSUSD scheme makes its line again.
#[test]
fn a_scheme_away_through_a_reset_writes_it_off_once_back() {
    for away in [USD, XRP] {
        for why in [Detach::Stopped, Detach::Removed] {
            let mut world = World::new(&[true]);
            world.closes(3);
            world.mint(0, "4");
            world.closes(3);
            for ledger in [XRP, USD] {
                assert_eq!(world.nodes[0].status(ledger), LedgerStatus::Synced);
            }
            let held = world.nodes[0].scheme(away).balance();
            assert!(held > 0);
            world.leave(0, away, why);
            world.ledger.reset();
            world.closes(6);
            if why == Detach::Stopped {
                world.restart(0);
            } else {
                world.add_again(0, away);
            }
            world.closes(6);
            let node = &world.nodes[0];
            for ledger in [XRP, USD] {
                assert_eq!(node.status(ledger), LedgerStatus::Synced, "{away} {why:?}");
            }
            assert!(node.dsusd().has_synced());
            assert!(node.books[away].losses.contains(&held), "{away} {why:?}");
            assert_eq!(node.books[USD].suspense, -i128::from(usd(4_000_000)));
            let address = world.address(0);
            assert!(
                world
                    .ledger
                    .line(address, dsusd::ISSUER, dsusd::CURRENCY)
                    .is_some()
            );
        }
    }
}

// spec/areas/xrpl.md#the-account: every XRP fee is booked
// once. A DSUSD settlement that validates while its scheme is away has
// its fee booked by the XRP scheme then; back, after a restart or added
// again, the DSUSD scheme reads it again as its own and posts it, and the
// XRP scheme does not book its fee again.
#[test]
fn the_fee_of_a_scheme_away_is_booked_once() {
    for why in [Detach::Stopped, Detach::Removed] {
        let mut world = World::new(&[true, false]);
        world.closes(3);
        let (usd_a, _) = world.peer(USD, 0, 1);
        world.mint(0, "10");
        world.close();
        let id = world.settle(USD, 0, 1, usd(1_000_000), &usd_a);
        world.leave(0, USD, why);
        world.closes(2);
        let fees = world.nodes[0].books[XRP].fee_ids.clone();
        assert!(fees.contains(&id));
        if why == Detach::Stopped {
            world.restart(0);
        } else {
            world.add_again(0, USD);
        }
        world.closes(2);
        let books = &world.nodes[0].books;
        assert_eq!(books[USD].finalized, [id]);
        assert_eq!(books[XRP].fee_ids, fees);
        assert_eq!(books[USD].fees, 0);
    }
}

// Found in review: as above, with a server whose account history no longer
// reaches back to the settlement once its scheme is back: a hint the
// account does not need, as the history of the line, which the proven
// line names, finds it. It is posted once, and nothing goes through
// suspense.
#[test]
fn a_settlement_of_a_scheme_away_is_booked_once_without_its_history() {
    for why in [Detach::Stopped, Detach::Removed] {
        let mut world = World::new(&[true, false]);
        world.closes(3);
        let (usd_a, _) = world.peer(USD, 0, 1);
        world.mint(0, "10");
        world.close();
        let id = world.settle(USD, 0, 1, usd(2_000_000), &usd_a);
        world.leave(0, USD, why);
        world.closes(2);
        world.ledger.shorten_account_history();
        if why == Detach::Stopped {
            world.restart(0);
        } else {
            world.add_again(0, USD);
        }
        world.closes(2);
        let books = &world.nodes[0].books[USD];
        assert_eq!(books.finalized, [id], "{why:?}");
        assert!(books.pending.is_empty(), "{why:?}");
        assert_eq!(
            books.suspense, 0,
            "{why:?}: what suspense booked came back out"
        );
        assert_eq!(books.settlement, i128::from(usd(8_000_000)), "{why:?}");
    }
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: a settlement of
// the scheme while it was away, in history the server no longer has beyond
// the history bound, is one it never reads again: the sync books what the
// line lost through suspense, and the scheme keeps that it never read it
// across another restart. No server has it to prove: it stays pending for
// the owner, never cancelled, nor booked twice.
#[test]
fn a_settlement_never_read_stays_pending_across_a_restart() {
    let mut world = World::past_a_flag_ledger(&[true, false]);
    world.closes(3);
    let (usd_a, _) = world.peer(USD, 0, 1);
    world.mint(0, "10");
    world.close();
    let id = world.settle(USD, 0, 1, usd(2_000_000), &usd_a);
    world.leave(0, USD, Detach::Stopped);
    world.closes(2);
    world.away_beyond_the_bound(0);
    world.drop_lookups = true;
    world.restart(0);
    world.close();
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    assert_eq!(world.nodes[0].account.unproven().history_gaps, 1);
    assert!(world.nodes[0].books[USD].finalized.is_empty());
    world.drop_lookups = false;
    world.restart(0);
    world.closes(2);
    let books = &world.nodes[0].books[USD];
    assert!(books.finalized.is_empty() && books.cancelled.is_empty());
    assert!(books.pending.contains_key(&id));
    assert_eq!(books.suspense, -i128::from(usd(2_000_000)));
    assert_eq!(books.settlement, i128::from(usd(8_000_000)));
}

// Found in review: the books refuse what the DSUSD scheme is told with its
// own settlement, validated, and stop it, while the XRP scheme runs on and
// has the account's state written. The account keeps the settlement for
// the DSUSD scheme, so after a restart it is posted once, never also booked
// as a withdrawal from the line, whether or not the server's account
// history still reaches back to it.
#[test]
fn a_settlement_told_in_a_refused_batch_is_booked_once() {
    for short_history in [false, true] {
        let mut world = World::new(&[true, false]);
        world.closes(3);
        let (usd_a, _) = world.peer(USD, 0, 1);
        world.mint(0, "10");
        world.close();
        let id = world.settle(USD, 0, 1, usd(2_000_000), &usd_a);
        world.refuse_own = Some(USD);
        world.close();
        assert!(world.nodes[0].away[USD], "the books refused the batch");
        world.close();
        if short_history {
            world.ledger.shorten_account_history();
        }
        world.restart(0);
        world.closes(2);
        let books = &world.nodes[0].books[USD];
        assert_eq!(books.finalized, [id], "{short_history}");
        assert!(books.pending.is_empty(), "{short_history}");
        assert!(books.withdrawals.is_empty(), "{short_history}");
        assert_eq!(books.suspense, 0, "{short_history}");
        assert_eq!(
            books.settlement,
            i128::from(usd(8_000_000)),
            "{short_history}"
        );
    }
}

// Found in review: a server's answer about a settlement the DSUSD scheme
// missed while away says it delivered more DSUSD than any line holds. Its
// metadata no longer proves: nothing of it is booked, and the account
// connects anew. Once a server proves it, it is posted once, by what was
// signed.
#[test]
fn a_settlement_whose_metadata_was_changed_books_nothing() {
    let mut world = World::new(&[true, false]);
    world.closes(3);
    let (usd_a, _) = world.peer(USD, 0, 1);
    world.mint(0, "10");
    world.close();
    let id = world.settle(USD, 0, 1, usd(2_000_000), &usd_a);
    world.leave(0, USD, Detach::Stopped);
    world.closes(2);
    // The metadata comes from `tx` in binary.
    world.tamper_lookup = Some(|answer| {
        let Some(meta) = answer["result"]["meta"].as_str() else {
            return;
        };
        let changed = meta.replacen("E5", "E6", 1);
        answer["result"]["meta"] = changed.into();
    });
    world.restart(0);
    world.closes(2);
    let lies: u64 = account::Doubt::ALL
        .iter()
        .map(|doubt| world.nodes[0].account.doubts(*doubt))
        .sum();
    assert!(lies > 0);
    assert!(world.nodes[0].books[USD].finalized.is_empty());
    world.tamper_lookup = None;
    world.wait(account::STALL_MS);
    world.closes(2);
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.finalized, [id]);
    assert_eq!(books.suspense, 0, "what suspense booked came back out");
    assert_eq!(books.settlement, i128::from(usd(8_000_000)));
}

// spec/areas/xrpl.md#the-account: while the books stopped
// `xrpl-testnet-xrp`, the account's XRP is still on its books: the DSUSD scheme
// spends none of it as gas, and settles nothing, until the XRP scheme is
// back after a restart. Removed by the owner, it leaves the XRP off the
// books, and only the reserve bounds the gas.
#[test]
fn no_gas_while_the_xrp_scheme_is_stopped() {
    let mut world = World::new(&[true, false]);
    world.closes(3);
    let (usd_a, _) = world.peer(USD, 0, 1);
    world.mint(0, "10");
    world.close();
    world.leave(0, XRP, Detach::Stopped);
    assert_eq!(
        world
            .prepare(USD, 0, 1, usd(1_000_000), &usd_a)
            .unwrap_err(),
        SchemeError::Unavailable
    );
    world.closes(2);
    world.restart(0);
    world.closes(2);
    let id = world.settle(USD, 0, 1, usd(1_000_000), &usd_a);
    world.close();
    assert_eq!(world.nodes[0].books[USD].finalized, [id]);
    world.leave(0, XRP, Detach::Removed);
    let id = world.settle(USD, 0, 1, usd(1_000_000), &usd_a);
    world.close();
    assert_eq!(world.nodes[0].books[USD].finalized.last(), Some(&id));
}

// spec/areas/xrpl.md#the-account: the account signs a Payment of a token
// only while the line holds it, after what the scheme has in flight, as it
// signs one of XRP only while the account holds it.
#[test]
fn a_token_payment_is_signed_only_within_the_line() {
    let (mut world, state_a, _) = synced(2);
    world.mint(0, "5");
    world.close();
    let unavailable = |world: &mut World, amount: u64| {
        world.prepare(USD, 0, 1, amount, &state_a).unwrap_err() == SchemeError::Unavailable
    };
    assert!(unavailable(&mut world, usd(5_000_001)));
    let first = world.prepare(USD, 0, 1, usd(3_000_000), &state_a).unwrap();
    assert!(unavailable(&mut world, usd(2_000_001)));
    let second = world.prepare(USD, 0, 1, usd(2_000_000), &state_a).unwrap();
    world.execute(USD, 0, first.id);
    world.execute(USD, 0, second.id);
    world.close();
    assert_eq!(world.nodes[0].books[USD].finalized, [first.id, second.id]);
}

// The stream is a hint: one that says a settlement delivered a token of
// another issuer, here the payee's own, changes nothing of what is booked,
// which is the transaction as its proof has it. (A Payment that does name
// the payee as its issuer is no settlement: the scheme's own tests hold
// that.)
#[test]
fn a_stream_that_names_another_issuer_is_a_hint() {
    let (mut world, state_a, _) = synced(2);
    world.mint(0, "10");
    world.close();
    let id = world.settle(USD, 0, 1, usd(4_000_000), &state_a);
    let payee = world.address(1).to_address();
    world.now += 4_000;
    for (connection, message) in world.ledger.close() {
        let Some(index) = world
            .nodes
            .iter()
            .position(|n| n.online && n.connection == connection)
        else {
            continue;
        };
        let mut message: serde_json::Value = serde_json::from_slice(&message).unwrap();
        if index == 1 && message["type"] == "transaction" {
            for (outer, field) in [("meta", "delivered_amount"), ("transaction", "Amount")] {
                let amount = &mut message[outer][field];
                if amount.is_object() {
                    amount["issuer"] = payee.clone().into();
                }
            }
        }
        let message = serde_json::to_vec(&message).unwrap();
        world.input(index, LedgerInput::Message(&message));
    }
    world.check();
    let payer = world.nodes[0].name;
    let books = &world.nodes[1].books[USD];
    assert_eq!(books.credited.len(), 1);
    assert_eq!(books.peers.get(&payer), Some(&i128::from(usd(4_000_000))));
    assert!(books.deposits.is_empty());
    assert_eq!(books.owner, 0);
    assert_eq!(world.nodes[0].books[USD].finalized, [id]);
}

// spec/areas/xrpl.md#the-peg-guard: a server that sends no ledger to anchor
// on, keeps proving the pool at the one anchor the account has, and keeps
// the connection alive with its replies, holds the account synced there,
// and the guard reads the pool there again and again. A reading counts only
// from an anchor that closed within the bound, so the guard pauses as one
// with no reading.
#[test]
fn readings_at_an_old_anchor_do_not_keep_the_guard_fresh() {
    let mut world = World::new(&[true]);
    world.closes(2);
    // A thousand XRP against 500 DSUSD, at two XRP to the dollar: par.
    world
        .ledger
        .set_pool(IssuedAsset::DSUSD, 1_000_000_000, value("500"), 0)
        .unwrap();
    world.close();
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    let kept = world.nodes[0].account.anchor().unwrap();
    // Twenty minutes, every ten seconds: a fresh rate, a reply that names
    // no ledger, a settlement check every minute, and the account's timer.
    for step in 0..=120 {
        let now = world.now;
        world.nodes[0].dsusd.as_mut().unwrap().on_rate(
            "XRP",
            Some(DollarRate {
                per_dollar: 2_000_000_000_000_000,
                precision: 15,
                as_of_ms: now,
            }),
        );
        world.input(
            0,
            LedgerInput::Message(br#"{"id":0,"status":"success","result":{}}"#),
        );
        if step % 6 == 0 {
            world.settlement_check(0);
        }
        world.nodes[0].account.on_timer(now);
        world.tell(0);
        assert_eq!(world.nodes[0].account.anchor(), Some(kept));
        if step == 0 {
            // Read at an anchor just taken: it counts, at par.
            let peg = world.nodes[0].dsusd().peg().unwrap();
            assert_eq!(peg.last, Some((1_000_000, kept.index)));
            assert_eq!(peg.paused, None);
        }
        world.now += 10_000;
    }
    assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced);
    assert_eq!(
        world.nodes[0].dsusd().peg().unwrap().paused,
        Some(PegPause::Stale),
        "twenty minutes of readings at one anchor are no reading"
    );
    assert!(!world.nodes[0].dsusd().converts());
}

/// What the issuer, the owners and the nodes do, in a property test.
#[derive(Clone, Debug)]
enum Op {
    Mint(usize, u64),
    ClawBack(usize, u64),
    Settle(usize, u64),
    Freeze(usize, bool),
    DeepFreeze(usize, bool),
    GlobalFreeze(bool),
    TransferFee(bool),
    Check(usize),
    /// Node `n` stops, and starts again after `closes` ledgers.
    Restart(usize, usize),
    Close,
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let node = 0..2_usize;
    // Millionths and tenths of them, so remainders below a unit occur.
    let tenths = 1..30_000_000_u64;
    let op = prop_oneof![
        3 => (node.clone(), tenths.clone()).prop_map(|(n, t)| Op::Mint(n, t)),
        1 => (node.clone(), tenths.clone()).prop_map(|(n, t)| Op::ClawBack(n, t)),
        4 => (node.clone(), tenths).prop_map(|(n, t)| Op::Settle(n, t)),
        1 => (node.clone(), any::<bool>()).prop_map(|(n, on)| Op::Freeze(n, on)),
        1 => (node.clone(), any::<bool>()).prop_map(|(n, on)| Op::DeepFreeze(n, on)),
        1 => any::<bool>().prop_map(Op::GlobalFreeze),
        1 => any::<bool>().prop_map(Op::TransferFee),
        1 => node.clone().prop_map(Op::Check),
        1 => (node, 0..3_usize).prop_map(|(n, closes)| Op::Restart(n, closes)),
        4 => Just(Op::Close),
    ];
    prop::collection::vec(op, 1..48)
}

/// Tenths of a millionth of DSUSD.
fn tenths(count: u64) -> TokenValue {
    TokenValue::from_units(count, 7).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Whatever the issuer does to the lines, and whatever the nodes settle
    /// and however often they restart, with ledgers closing while they are
    /// down, each node's books hold its line in
    /// whole millionths and, beside it on one account, its XRP less the
    /// reserve, at every ledger; every settlement is posted or voided once,
    /// and every fee its account paid is booked once, on the XRP ledger.
    #[test]
    fn the_books_follow_the_line(ops in ops()) {
        let mut world = World::new(&[true, false]);
        world.closes(3);
        let (to_1, to_0) = world.peer(USD, 0, 1);
        let states = [to_1, to_0];
        for op in &ops {
            match *op {
                Op::Mint(n, t) => {
                    let to = world.address(n);
                    world.ledger.issue(to, IssuedAsset::DSUSD.amount(tenths(t))).unwrap();
                }
                Op::ClawBack(n, t) => {
                    let from = world.address(n);
                    world.ledger.claw_back(from, IssuedAsset::DSUSD.amount(tenths(t))).unwrap();
                }
                Op::Settle(n, t) => {
                    let state = states[n].clone();
                    if let Ok(prepared) = world.prepare(USD, n, 1 - n, t * 100, &state) {
                        world.execute(USD, n, prepared.id);
                    }
                }
                Op::Freeze(n, on) => {
                    let holder = world.address(n);
                    world.ledger.freeze(holder, IssuedAsset::DSUSD, on).unwrap();
                }
                Op::DeepFreeze(n, on) => {
                    let holder = world.address(n);
                    world.ledger.deep_freeze(holder, IssuedAsset::DSUSD, on).unwrap();
                }
                Op::GlobalFreeze(on) => world.ledger.global_freeze(dsusd::ISSUER, on).unwrap(),
                Op::TransferFee(on) => {
                    let rate = if on { 1_002_000_000 } else { 0 };
                    world.ledger.set_transfer_rate(dsusd::ISSUER, rate).unwrap();
                }
                Op::Check(n) => world.settlement_check(n),
                Op::Restart(n, closes) => {
                    world.down(n);
                    world.closes(closes);
                    world.restart(n);
                }
                Op::Close => world.close(),
            }
        }
        // Everything on its way has applied or can never apply.
        world.closes(30);
        let address = world.address(0);
        let reply: serde_json::Value = serde_json::from_slice(&world.ledger.request(
            0,
            serde_json::json!({ "command": "account_tx", "account": address.to_address() })
                .to_string()
                .as_bytes(),
        ))
        .unwrap();
        let history = api::account_tx_result(&reply["result"]).unwrap().transactions;
        let paid: u64 = history.iter().filter(|t| t.account == address).map(|t| t.fee).sum();
        let node = &world.nodes[0];
        prop_assert_eq!(node.books[XRP].fees, i128::from(paid * UNITS_PER_DROP));
        prop_assert_eq!(node.books[USD].fees, 0);
        for node in &world.nodes {
            prop_assert!(node.books[USD].pending.is_empty(), "{:?}", node.books[USD].pending);
        }
    }
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: a settlement in
// a token changes the payee's line alone, never its account's root. One a
// server leaves out of the history, at the newest end or in the middle, or
// whose ledger it names wrong, keeps nothing from the payee, which finds
// it by the history the line it follows names, proven: each is credited
// once, and the books hold the line.
#[test]
fn a_token_settlement_left_out_of_the_history_is_found_by_the_line() {
    for lie in [
        Lie::HistoryNewestLeftOut,
        Lie::HistoryMiddleLeftOut,
        Lie::Transaction,
    ] {
        let (mut world, state_a, _) = synced(2);
        world.mint(0, "10");
        world.close();
        let connection = world.nodes[1].connection;
        world.ledger.disconnect(connection);
        world.input(1, LedgerInput::Disconnected);
        world.nodes[1].online = false;
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(world.settle(USD, 0, 1, usd(1_000_000), &state_a));
            world.close();
        }
        world.ledger.set_lie(world.next_connection + 1, Some(lie));
        world.connect(1);
        world.closes(2);
        assert!(world.ledger.lies_told(lie) > 0, "{lie:?}");
        let books = &world.nodes[1].books[USD];
        assert_eq!(books.credited, ids.iter().copied().collect(), "{lie:?}");
        assert!(books.reconciled.is_empty(), "{lie:?}");
        assert_eq!(world.nodes[0].books[USD].finalized, ids, "{lie:?}");
    }
}

// spec/designs/xrpl-validation.md#4-what-is-proven-and-how: the line, its
// issuer's account and the pool are proven at the anchor. A server that
// alters them, says they are absent, or answers with another's branch, has
// nothing booked, frozen or written off: the scheme waits until an honest
// server answers, and then books what the line holds.
#[test]
fn a_line_a_server_alters_books_nothing() {
    for lie in [Lie::AlteredObject, Lie::FalseAbsence, Lie::OtherBranch] {
        let (mut world, _, _) = synced(2);
        world.mint(0, "10");
        world.close();
        let held = world.nodes[0].books[USD].settlement;
        let write_offs = world.nodes[0].account.write_offs();
        world.lagging = true;
        world.ledger.set_lie_for_all(Some(lie));
        world.mint(0, "5");
        world.closes(2);
        world.settlement_check(0);
        assert!(world.ledger.lies_told(lie) > 0, "{lie:?}");
        let books = &world.nodes[0].books[USD];
        assert_eq!(books.settlement, held, "{lie:?}");
        assert!(
            books.reconciled.is_empty() && books.losses.is_empty(),
            "{lie:?}"
        );
        assert!(world.nodes[0].dsusd().can_receive(), "{lie:?}");
        assert_eq!(world.nodes[0].account.write_offs(), write_offs, "{lie:?}");
        world.ledger.set_lie_for_all(None);
        world.wait(account::STALL_MS);
        world.closes(2);
        world.lagging = false;
        world.check();
        assert_eq!(
            world.nodes[0].books[USD].settlement,
            held + i128::from(usd(5_000_000)),
            "{lie:?}"
        );
        assert_eq!(world.nodes[0].status(USD), LedgerStatus::Synced, "{lie:?}");
    }
}

/// What the ledger holds under `hash`, validated.
fn validated(world: &mut World, hash: Name) -> api::TxRecord {
    let hash: [u8; 32] = dassie_xrpl::from_hex(hash.as_str())
        .unwrap()
        .try_into()
        .unwrap();
    let reply = world.ledger.request(999, &api::tx_anywhere(1, &hash));
    let outcome = match api::parse(&reply) {
        api::Message::Response { outcome, .. } => Some(outcome),
        _ => None,
    };
    api::tx_result(&outcome.unwrap().unwrap()).unwrap()
}

/// An owner's payment of `amount` to `destination`, with no tag or memo.
fn owner_payment(destination: AccountId, amount: u64) -> dassie_settlement::OwnerPayment {
    dassie_settlement::OwnerPayment {
        destination: destination.to_address(),
        tag: None,
        memo: None,
        amount,
    }
}

// spec/areas/xrpl.md#deposits-and-withdrawals: the owner pays DSUSD out of
// the line, to a destination with a tag and one memo, as a token hub is
// funded. Once the ledger validated it, it is the owner's withdrawal; its fee
// is gas, never on the DSUSD books; the account it reached books it as a
// deposit, never as a settlement. One at a time; whole millionths, never to
// the account itself, and a memo of at most its bound.
#[test]
fn the_owner_pays_dsusd_out_of_the_line() {
    use dassie_settlement::OwnerPaymentState;

    let (mut world, _, _) = synced(2);
    world.mint(0, "10");
    world.close();
    world.settlement_check(0);
    let to = world.address(1);
    let own = world.address(0);
    for refused in [
        owner_payment(own, usd(1_000_000)),
        owner_payment(to, 0),
        owner_payment(to, usd(1_000_000) + 1),
        dassie_settlement::OwnerPayment {
            memo: Some(dassie_settlement::OwnerMemo {
                memo_type: b"type".to_vec(),
                data: vec![1; dassie_settlement::MAX_OWNER_MEMO_BYTES + 1],
            }),
            ..owner_payment(to, usd(1_000_000))
        },
        dassie_settlement::OwnerPayment {
            memo: Some(dassie_settlement::OwnerMemo {
                memo_type: dassie_xrpl::dsusd::MEMO_TYPE.to_vec(),
                data: b"node1".to_vec(),
            }),
            ..owner_payment(to, usd(1_000_000))
        },
    ] {
        assert_eq!(
            world.pay_owner(0, &refused),
            Err(SchemeError::InvalidData),
            "{refused:?}"
        );
    }
    let payment = dassie_settlement::OwnerPayment {
        tag: Some(42),
        memo: Some(dassie_settlement::OwnerMemo {
            memo_type: b"https://example.com/memo".to_vec(),
            data: vec![7; 32],
        }),
        ..owner_payment(to, usd(4_000_000))
    };
    let id = world.pay_owner(0, &payment).unwrap();
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(id),
        OwnerPaymentState::InFlight
    );
    assert_eq!(
        world.pay_owner(0, &payment),
        Err(SchemeError::Unavailable),
        "one at a time"
    );
    world.close();
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(id),
        OwnerPaymentState::Applied
    );
    let (a, b) = (&world.nodes[0].books[USD], &world.nodes[1].books[USD]);
    assert_eq!(a.owner, i128::from(usd(6_000_000)));
    assert!(a.owner_holds.is_empty());
    assert_eq!(a.fees, 0, "its fee is gas");
    assert_eq!(b.owner, i128::from(usd(4_000_000)));
    assert!(b.credited.is_empty(), "no settlement");
    assert_eq!(a.suspense + b.suspense, 0);
    let record = validated(&mut world, id);
    assert_eq!(record.destination, Some(to));
    assert_eq!(record.destination_tag, Some(42));
    assert_eq!(
        record.delivered_token,
        Some(IssuedAsset::DSUSD.amount(value("4")))
    );
    assert_eq!(
        record.memo,
        Some((b"https://example.com/memo".to_vec(), vec![7; 32]))
    );
    // The next may go once it is done.
    let next = world
        .pay_owner(0, &owner_payment(to, usd(1_000_000)))
        .unwrap();
    world.close();
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(next),
        OwnerPaymentState::Applied
    );
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(id),
        OwnerPaymentState::Unknown,
        "only the last is the scheme's to tell"
    );
    assert_eq!(world.nodes[0].books[USD].owner, i128::from(usd(5_000_000)));
}

// spec/areas/xrpl.md#deposits-and-withdrawals: the owner's DSUSD payment on
// its way is kept across a restart and booked once after; what became of it
// outlives the next restart.
#[test]
fn the_owners_dsusd_payment_outlives_a_restart() {
    use dassie_settlement::OwnerPaymentState;

    let (mut world, _, _) = synced(2);
    world.mint(0, "10");
    world.close();
    world.settlement_check(0);
    let to = world.address(1);
    let id = world
        .pay_owner(0, &owner_payment(to, usd(3_000_000)))
        .unwrap();
    world.restart(0);
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(id),
        OwnerPaymentState::InFlight
    );
    world.closes(2);
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(id),
        OwnerPaymentState::Applied
    );
    world.restart(0);
    world.closes(1);
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(id),
        OwnerPaymentState::Applied
    );
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.owner, i128::from(usd(7_000_000)));
    assert_eq!(books.suspense, 0);
    assert!(books.owner_holds.is_empty());
    assert_eq!(world.nodes[1].books[USD].owner, i128::from(usd(3_000_000)));
}

// spec/areas/xrpl.md#deposits-and-withdrawals: a frozen line pays nothing out
// for the owner, as it settles nothing; one the ledger refuses, to an account
// without a line for the token, costs only its fee, in gas, and moves nothing
// on the DSUSD books.
#[test]
fn a_frozen_line_pays_the_owner_nothing() {
    use dassie_settlement::{OwnerPaymentState, PaymentFailure};

    let (mut world, _, _) = synced(2);
    world.mint(0, "10");
    world.close();
    world.settlement_check(0);
    let to = world.address(1);
    let address = world.address(0);
    world
        .ledger
        .freeze(address, IssuedAsset::DSUSD, true)
        .unwrap();
    world.close();
    assert_eq!(
        world.pay_owner(0, &owner_payment(to, usd(1_000_000))),
        Err(SchemeError::Unavailable)
    );
    world
        .ledger
        .freeze(address, IssuedAsset::DSUSD, false)
        .unwrap();
    world.close();
    // An account with XRP and no DSUSD line: the ledger fails it.
    let nobody = Wallet::from_seed([77; 16]).account();
    world.ledger.fund(nobody);
    world.close();
    let id = world
        .pay_owner(0, &owner_payment(nobody, usd(1_000_000)))
        .unwrap();
    world.close();
    assert_eq!(
        world.nodes[0].dsusd().owner_payment(id),
        OwnerPaymentState::Failed(PaymentFailure::Other)
    );
    let books = &world.nodes[0].books[USD];
    assert_eq!(books.owner, i128::from(usd(10_000_000)));
    assert!(books.owner_holds.is_empty());
    assert_eq!(books.suspense, 0);
}
