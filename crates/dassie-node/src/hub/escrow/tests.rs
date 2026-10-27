//! The watcher against the fake XRP Ledger: it checks the escrow as the
//! sponsor set it up, attests what reaches it, follows across reconnects
//! without attesting twice, and attests a reset of the test network
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals,
//! spec/decisions/ledger-resets.md); all of it only from what its verifier
//! proved, so that a lying server makes it attest nothing no validated
//! ledger holds, however it lies; and a token hub's issuer's acts, from
//! proofs (spec/decisions/token-hubs.md).

use dassie_hub::escrow::{escrow_master, setup};
use dassie_hub::ledger::TESTNET_NETWORK;
use dassie_hub::operation::ValidatorOperation;
use dassie_identity::NodeKey;
use dassie_xrpl::Wallet;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::api::{TRANSFER_RATE_PAR, account_flags};
use dassie_xrpl::codec::{Amount, Payment, Signing, TokenValue, Transact, TrustSet};
use dassie_xrpl::fake::{
    CLOSE_INTERVAL_S, DSUSD_CURRENCY, DSUSD_ISSUER, FEE_BASE, FakeLedger, KeyedSignatures,
    LIST_SITE, Lie,
};
use dassie_xrpl::unl::ledger_seconds;

use super::*;
use crate::hub::config::tests::{NONCE, rules};

/// The secret the fake signs its validations under, and the watcher checks
/// them with.
const SECRET: [u8; 32] = *b"the watcher's tests' validations";

/// The connection the scene is set up over: never the watcher's.
const SETUP: u64 = 1_000;

/// The most outputs one input may take to settle.
const MAX_TURNS: usize = 100_000;

/// DSUSD, the token hub's asset.
const DSUSD: IssuedAsset = IssuedAsset {
    issuer: DSUSD_ISSUER,
    currency: DSUSD_CURRENCY,
};

struct Setup {
    xrpl: FakeLedger,
    view: EscrowView,
    watcher: Watcher,
    /// The watcher's connection.
    connection: u64,
    /// The lie each of its connections tells.
    lie: Option<Lie>,
    attested: Vec<ValidatorOperation>,
    cursor: Option<EscrowCursor>,
    now: u64,
    /// Whether the validator takes each attestation at once.
    taking: bool,
    /// Attestations it has not taken yet.
    untaken: Vec<(ValidatorOperation, Option<[u8; 32]>)>,
    /// Whether the hub takes the escrow's check and the issuer's hold as
    /// soon as they are attested, as a quorum would.
    quorum: bool,
    /// Whether the hub records each deposit's attestation the validator
    /// takes, as it does unless a full share refuses it.
    recording: bool,
    /// The escrow transactions the watcher submitted to its server.
    submitted: Vec<Vec<u8>>,
    /// How often the watcher asked for a new connection.
    reconnects: u32,
    /// The account that holds DSUSD and deposits it, in a token hub.
    holder: Wallet,
    /// The issuer's transfer rate, as set.
    rate: u32,
    /// The issuer's hold, by the ledger after which it held so.
    truth: BTreeMap<u32, (bool, bool)>,
}

/// The watcher of `escrow` on the testnet, trusting the fake's validators.
fn watcher(escrow: AccountId, cursor: Option<EscrowCursor>) -> Watcher {
    let xrpl = FakeLedger::new();
    let unl = UnlConfig::for_network(TESTNET_NETWORK)
        .and_then(|own| own.with_override(&xrpl.publishers(LIST_SITE)))
        .unwrap();
    Watcher::new(
        escrow,
        TESTNET_NETWORK,
        cursor,
        unl,
        Box::new(KeyedSignatures::new(SECRET)),
    )
}

impl Setup {
    /// An XRP hub whose sponsor set up its escrow on the fake ledger.
    fn new() -> Self {
        Self::of(None)
    }

    /// A hub of `token`, or of XRP, whose sponsor set up its escrow.
    fn of(token: Option<IssuedAsset>) -> Self {
        let mut xrpl = FakeLedger::new();
        xrpl.sign_validations_with(SECRET);
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let nodes: Vec<(NodeKey, String)> = (1..=4)
            .map(|i| (NodeKey::from_bytes(&[i; 32]), format!("v{i}.example:443")))
            .collect();
        let mut rules = rules(&sponsor, &nodes);
        if let Some(token) = token {
            rules.terms.asset = token.asset();
        }
        let master = escrow_master(&sponsor, TESTNET_NETWORK, &NONCE);
        let holder = Wallet::from_seed([2; 16]);
        xrpl.fund(master.account());
        xrpl.deposit(holder.account(), 100_000_000);
        xrpl.close();
        let sequence = xrpl.sequence(master.account()).unwrap();
        for transaction in setup(&rules, &master, sequence, FEE_BASE) {
            xrpl.request(SETUP, &api::submit(2, transaction.blob()));
        }
        xrpl.close();
        // What the hub takes from the check: the escrow's sequences before
        // it are its setup's.
        let next_sequence = xrpl.sequence(master.account()).unwrap();
        let view = EscrowView {
            rules,
            checked: false,
            next_sequence,
            withdrawals: Vec::new(),
            signer_list: None,
            written_off_at: None,
            issuer: Issuer::default(),
            deposit_room: 4,
            deposits_recorded: Vec::new(),
        };
        let watcher = watcher(master.account(), None);
        let mut setup = Self {
            now: xrpl.now_ms() + 1_000,
            xrpl,
            view,
            watcher,
            connection: 1,
            lie: None,
            attested: Vec::new(),
            cursor: None,
            taking: true,
            untaken: Vec::new(),
            quorum: false,
            recording: true,
            submitted: Vec::new(),
            reconnects: 0,
            holder,
            rate: 0,
            truth: BTreeMap::new(),
        };
        if token.is_some() {
            setup.hold_dsusd();
        }
        setup
    }

    fn escrow(&self) -> AccountId {
        AccountId(self.view.rules.terms.escrow)
    }

    /// The holder opens a line to DSUSD's issuer and is issued some.
    fn hold_dsusd(&mut self) {
        let holder = &self.holder;
        let trust = TrustSet {
            account: holder.account(),
            limit: DSUSD.amount(TokenValue::parse("1000").unwrap()),
            fee: FEE_BASE,
            sequence: self.xrpl.sequence(holder.account()).unwrap(),
            last_ledger_sequence: None,
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(holder.public_key()),
        }
        .sign(holder);
        self.xrpl.request(SETUP, &api::submit(3, &trust.blob));
        self.xrpl.close();
        let held = DSUSD.amount(TokenValue::parse("50").unwrap());
        self.xrpl.issue(holder.account(), held).unwrap();
        self.xrpl.close();
    }

    /// The holder deposits `amount` DSUSD with tag `tag`, in the next
    /// ledger.
    fn deposit_dsusd(&mut self, amount: &str, tag: u32) {
        let holder = &self.holder;
        let paid = Payment {
            account: holder.account(),
            destination: self.escrow(),
            destination_tag: Some(tag),
            amount: Amount::Token(DSUSD.amount(TokenValue::parse(amount).unwrap())),
            send_max: None,
            fee: FEE_BASE,
            sequence: self.xrpl.sequence(holder.account()).unwrap(),
            last_ledger_sequence: None,
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(holder.public_key()),
        }
        .sign(holder);
        let answer = self.xrpl.request(SETUP, &api::submit(4, &paid.blob));
        let Message::Response { outcome, .. } = api::parse(&answer) else {
            panic!("no answer");
        };
        assert_eq!(
            api::submit_result(&outcome.unwrap()).as_deref(),
            Some("tesSUCCESS")
        );
    }

    /// Carries out what the watcher asks, as its host does, until it asks
    /// nothing more.
    fn run(&mut self) {
        for _ in 0..MAX_TURNS {
            self.watcher.check_invariants();
            let Some(out) = self.watcher.poll() else {
                return;
            };
            match out {
                Out::Send(bytes) => {
                    if String::from_utf8_lossy(&bytes).contains("\"submit\"") {
                        self.submitted.push(bytes.clone());
                    }
                    let answer = self.xrpl.request(self.connection, &bytes);
                    self.watcher.message(self.now, &answer, &self.view);
                }
                Out::Fetch(url) => {
                    let body = self.xrpl.fetch(&url);
                    self.watcher.fetched(self.now, &url, body.as_deref());
                }
                Out::Reconnect => {
                    self.reconnects += 1;
                    self.reconnect();
                }
                Out::Attest(operation, transaction) => {
                    self.take(&operation);
                    let deposit = matches!(operation, ValidatorOperation::AttestDeposit(_));
                    self.attested.push(operation.clone());
                    if self.taking {
                        self.watcher.taken(transaction);
                        if deposit {
                            self.record(transaction);
                        }
                    } else {
                        self.untaken.push((operation, transaction));
                    }
                }
                Out::Cursor(cursor) => self.cursor = Some(cursor),
                Out::Problem(_) => {}
            }
        }
        panic!("the watcher does not settle");
    }

    /// The hub records the attestation of a deposit the validator took,
    /// unless it is not `recording`.
    fn record(&mut self, transaction: Option<[u8; 32]>) {
        if let Some(hash) = transaction
            && self.recording
            && !self.view.deposits_recorded.contains(&hash)
        {
            self.view.deposits_recorded.push(hash);
        }
    }

    /// What the hub makes of an attestation, as if a quorum attested it.
    fn take(&mut self, operation: &ValidatorOperation) {
        if !self.quorum {
            return;
        }
        match operation {
            ValidatorOperation::AttestEscrow { .. } => self.view.checked = true,
            ValidatorOperation::AttestIssuer(issuer) if issuer.ledger > self.view.issuer.ledger => {
                self.view.issuer = *issuer;
            }
            _ => {}
        }
    }

    /// A new connection, which tells the lie set for it.
    fn reconnect(&mut self) {
        self.watcher.disconnected();
        self.xrpl.disconnect(self.connection);
        self.connection += 1;
        self.xrpl.set_lie(self.connection, self.lie);
        self.watcher.connected(self.now);
    }

    /// A ledger closes, [`CLOSE_INTERVAL_S`] on, at the clock's time, and
    /// what the streams carry is passed on.
    fn close(&mut self) {
        self.now += CLOSE_INTERVAL_S * 1_000;
        let streamed = self.xrpl.close_at(ledger_seconds(self.now));
        self.note_truth();
        for (connection, bytes) in streamed {
            if connection == self.connection {
                self.watcher.message(self.now, &bytes, &self.view);
            }
        }
        self.run();
        self.watcher.tick(self.now, &self.view);
        self.run();
    }

    /// `ledgers` ledgers close, one every [`CLOSE_INTERVAL_S`].
    fn closes(&mut self, ledgers: u32) {
        for _ in 0..ledgers {
            self.close();
        }
    }

    /// Time passes with no ledger closing: the server stops.
    fn tick(&mut self, ms: u64) {
        self.now += ms;
        self.watcher.tick(self.now, &self.view);
        self.run();
    }

    fn connect(&mut self) {
        self.xrpl.set_lie(self.connection, self.lie);
        self.watcher.connected(self.now);
        self.run();
        self.watcher.tick(self.now, &self.view);
        self.run();
    }

    /// The issuer's hold on the escrow after the ledger that just closed,
    /// as the fake holds it.
    fn note_truth(&mut self) {
        let line = self
            .xrpl
            .line(self.escrow(), DSUSD_ISSUER, DSUSD_CURRENCY)
            .and_then(|line| line.view(self.escrow()));
        let global = self
            .xrpl
            .account_flags(DSUSD_ISSUER)
            .is_some_and(|flags| flags & account_flags::GLOBAL_FREEZE != 0);
        let frozen = line.is_some_and(|view| view.theirs.freeze || view.theirs.deep_freeze);
        let fee = self.rate != 0 && self.rate != TRANSFER_RATE_PAR;
        self.truth
            .insert(self.xrpl.ledger(), (frozen || global, fee));
    }

    /// What it attested of the issuer's hold, in order.
    fn issuer_states(&self) -> Vec<(bool, bool)> {
        self.attested
            .iter()
            .filter_map(|op| match op {
                ValidatorOperation::AttestIssuer(issuer) => Some((issuer.frozen, issuer.fee)),
                _ => None,
            })
            .collect()
    }
}

/// Whether `operations` hold any of `kind`.
fn any(operations: &[ValidatorOperation], kind: impl Fn(&ValidatorOperation) -> bool) -> bool {
    operations.iter().any(kind)
}

#[test]
fn the_escrow_is_checked_and_its_deposits_attested() {
    let mut setup = Setup::new();
    setup.connect();
    assert!(setup.watcher.following());
    assert!(setup.watcher.anchor().is_some(), "an anchor is proven");
    setup.tick(1);
    let escrow_attested = any(&setup.attested, |op| {
        matches!(op, ValidatorOperation::AttestEscrow { .. })
    });
    assert!(escrow_attested, "the escrow as set up passes its check");
    // Checked once; a quorum's attestations make it so.
    setup.view.checked = true;
    setup.attested.clear();

    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    setup.closes(2);
    assert!(matches!(
        setup.attested.as_slice(),
        [ValidatorOperation::AttestDeposit(deposit)] if deposit.tag == 7 && deposit.amount == 5_000_000
    ));
    let cursor = setup.cursor.unwrap();
    assert!(cursor.scanned_to >= cursor.newest_ledger.saturating_sub(1));
    assert!(setup.watcher.verifier().proofs().transactions >= 1);

    // A reconnect reads the history again from a window back, and attests
    // nothing twice.
    setup.watcher.disconnected();
    setup.attested.clear();
    setup.connect();
    assert!(setup.attested.is_empty(), "{:?}", setup.attested);

    // A watcher that starts again, from its cursor, attests again what may
    // not have reached a block before the crash: its driver, started again
    // too, names no deposit recorded.
    setup.view.deposits_recorded.clear();
    setup.watcher = watcher(escrow, setup.cursor);
    setup.connect();
    setup.closes(2);
    let deposits = setup
        .attested
        .iter()
        .filter(|op| matches!(op, ValidatorOperation::AttestDeposit(_)))
        .count();
    assert_eq!(deposits, 1);
}

#[test]
fn a_reset_of_the_test_network_is_attested_and_a_server_behind_is_not() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    let newest = setup.cursor.unwrap();

    // A server behind: a lower ledger that closed before the newest.
    let mut behind = setup.xrpl.clone();
    setup.closes(3);
    let mut watcher = watcher(setup.escrow(), setup.cursor);
    std::mem::swap(&mut setup.xrpl, &mut behind);
    std::mem::swap(&mut setup.watcher, &mut watcher);
    setup.attested.clear();
    setup.connect();
    assert!(
        !setup.watcher.following(),
        "a server behind is not followed"
    );
    assert!(setup.attested.is_empty());
    std::mem::swap(&mut setup.xrpl, &mut behind);
    std::mem::swap(&mut setup.watcher, &mut watcher);
    assert!(setup.cursor.unwrap().newest_ledger > newest.newest_ledger);

    // A reset: a lower ledger that closed later, proven under the UNL.
    setup.xrpl.reset();
    setup.close();
    setup.watcher.disconnected();
    setup.connect();
    setup.closes(2);
    let reset = setup
        .attested
        .iter()
        .find_map(|op| match op {
            ValidatorOperation::AttestReset { ledger } => Some(*ledger),
            _ => None,
        })
        .expect("the reset is attested");
    assert!(reset < newest.newest_ledger, "a ledger of the new chain");
    assert!(setup.watcher.following());

    // Seen again, after another reconnect: attested no more.
    setup.attested.clear();
    setup.watcher.disconnected();
    setup.connect();
    setup.closes(2);
    assert!(
        !any(&setup.attested, |op| matches!(
            op,
            ValidatorOperation::AttestReset { .. }
        )),
        "{:?}",
        setup.attested
    );
}

#[test]
fn a_reset_the_hub_wrote_off_already_is_not_attested() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    setup.xrpl.reset();
    // A quorum saw the new chain first, and the hub wrote it off.
    setup.view.written_off_at = Some(u64::MAX);
    setup.close();
    setup.watcher.disconnected();
    setup.attested.clear();
    setup.connect();
    setup.closes(2);
    assert!(setup.watcher.following());
    assert!(
        !any(&setup.attested, |op| matches!(
            op,
            ValidatorOperation::AttestReset { .. }
        )),
        "{:?}",
        setup.attested
    );
}

// A server that says the network reset, with a header it made up, makes
// the watcher wait, never attest: the verifier proves no such chain.
#[test]
fn a_made_up_reset_is_never_attested() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    setup.lie = Some(Lie::MadeUpReset);
    setup.reconnect();
    for _ in 0..4 {
        setup.closes(3);
        setup.tick(STALL_MS);
    }
    assert!(
        !any(&setup.attested, |op| matches!(
            op,
            ValidatorOperation::AttestReset { .. }
        )),
        "{:?}",
        setup.attested
    );
}

#[test]
fn a_server_of_another_network_is_not_followed() {
    let mut setup = Setup::new();
    setup.xrpl.set_network_id(Some(0));
    setup.connect();
    assert!(!setup.watcher.following());
    setup.xrpl.set_network_id(Some(1));
    setup.tick(RETRY_MS);
    assert!(setup.watcher.following());
}

#[test]
fn a_reset_counts_only_where_the_network_resets() {
    assert_eq!(chain(Some((100, 50)), 1, 10, 60), Chain::Restarted);
    assert_eq!(chain(Some((100, 50)), 0, 10, 60), Chain::WentBack);
    assert_eq!(chain(Some((100, 50)), 1, 10, 40), Chain::Behind);
    assert_eq!(chain(Some((100, 50)), 1, 100, 40), Chain::Current);
    assert_eq!(chain(None, 1, 1, 1), Chain::Current);
}

// An attestation the validator has not taken yet is attested no second
// time, and holds the cursor back, so a crash reads its transaction again.
#[test]
fn an_attestation_counts_once_the_validator_took_it() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    setup.taking = false;
    setup.attested.clear();
    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    let deposit_ledger = setup.xrpl.ledger();
    setup.closes(3);
    assert_eq!(setup.attested.len(), 1);
    assert!(
        setup.cursor.unwrap().scanned_to < deposit_ledger,
        "the cursor waits"
    );
    // Read again, from a reconnect, it is not attested twice.
    setup.watcher.disconnected();
    setup.connect();
    setup.closes(2);
    assert_eq!(setup.attested.len(), 1);
    // Once taken, and the hub recorded it, the cursor moves on.
    for (_, transaction) in core::mem::take(&mut setup.untaken) {
        setup.watcher.taken(transaction);
        setup.record(transaction);
    }
    setup.tick(1);
    assert!(setup.cursor.unwrap().scanned_to >= deposit_ledger);
}

// An attestation the host drops waits no more: the cursor moves on, but
// never past the dropped transaction's ledger, and the transaction is
// attested again when read again.
#[test]
fn a_dropped_attestation_holds_the_cursor_only_below_it() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    setup.taking = false;
    setup.attested.clear();
    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    let deposit_ledger = setup.xrpl.ledger();
    setup.closes(2);
    for (operation, transaction) in core::mem::take(&mut setup.untaken) {
        setup.watcher.dropped(&operation, transaction);
    }
    setup.run();
    setup.closes(2);
    let cursor = setup.cursor.unwrap();
    assert!(
        cursor.scanned_to < deposit_ledger,
        "never past the dropped one"
    );
    assert!(cursor.newest_ledger > deposit_ledger);
    // Read again, by a watcher that starts again, it is attested again.
    setup.attested.clear();
    setup.taking = true;
    setup.watcher = watcher(escrow, setup.cursor);
    setup.connect();
    setup.closes(2);
    assert_eq!(setup.attested.len(), 1, "{:?}", setup.attested);
}

// However many small payments reach the escrow at once, more than its
// hints hold, none is dropped: the history is paged through again.
#[test]
fn many_small_payments_drop_no_deposit() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    setup.attested.clear();
    let escrow = setup.escrow();
    let count = MAX_HINTS + PAGE as usize + 7;
    for tag in 0..count {
        setup
            .xrpl
            .deposit_tagged(escrow, 1_000, Some(u32::try_from(tag).unwrap() + 1));
    }
    setup.close();
    let paid_in = setup.xrpl.ledger();
    for _ in 0..40 {
        setup.closes(1);
    }
    let tags: BTreeSet<u32> = setup
        .attested
        .iter()
        .filter_map(|op| match op {
            ValidatorOperation::AttestDeposit(deposit) => Some(deposit.tag),
            _ => None,
        })
        .collect();
    assert_eq!(tags.len(), count, "every deposit, once");
    assert_eq!(setup.attested.len(), count, "nothing attested twice");
    assert!(setup.cursor.unwrap().scanned_to >= paid_in);
}

// A server that stops closing ledgers leaves the watcher's anchor to age:
// nothing is judged from it, and judging resumes with a newer anchor.
#[test]
fn an_old_anchor_judges_nothing() {
    let mut setup = Setup::of(Some(DSUSD));
    setup.view.checked = true;
    setup.quorum = true;
    setup.connect();
    setup.closes(2);
    setup.attested.clear();

    // The server stops; the escrow is to be checked again, and the hub
    // holds a freeze the ledger has not.
    setup.tick(ANCHOR_FRESH_MS + 1);
    assert!(!setup.watcher.fresh());
    setup.view.checked = false;
    setup.view.issuer = Issuer {
        ledger: 1,
        frozen: true,
        fee: false,
    };
    for _ in 0..ESCROW_CHECK_EVERY_MS / 1_000 + 1 {
        setup.tick(1_000);
    }
    assert!(setup.attested.is_empty(), "{:?}", setup.attested);

    // It closes ledgers again: a newer anchor, and both are judged.
    setup.closes(1);
    setup.tick(ESCROW_CHECK_EVERY_MS);
    setup.closes(1);
    assert!(setup.watcher.fresh());
    assert!(any(&setup.attested, |op| matches!(
        op,
        ValidatorOperation::AttestEscrow { .. }
    )));
    assert_eq!(setup.issuer_states(), [(false, false)]);
}

// What was to go on a connection, or to end it, goes with it: the next
// connection is neither sent the old one's requests nor ended for its
// sake. What it decided for the host stays.
#[test]
fn a_connection_gone_takes_what_was_to_go_on_it() {
    let mut setup = Setup::new();
    setup.watcher.connected(setup.now);
    // Not following yet: submits wait, and one past the bound is a
    // problem for the log.
    for i in 0..=MAX_QUEUED_SUBMITS {
        setup.watcher.submit(vec![u8::try_from(i % 256).unwrap()]);
    }
    setup.watcher.disconnected();
    let mut left = Vec::new();
    while let Some(out) = setup.watcher.poll() {
        left.push(out);
    }
    assert!(
        left.iter()
            .all(|out| !matches!(out, Out::Send(_) | Out::Reconnect)),
        "{left:?}"
    );
    assert_eq!(
        left.iter()
            .filter(|out| matches!(out, Out::Problem(_)))
            .count(),
        1
    );

    // The next connection follows the escrow as any.
    setup.connect();
    setup.closes(2);
    assert!(setup.watcher.following());
    assert!(any(&setup.attested, |op| matches!(
        op,
        ValidatorOperation::AttestEscrow { .. }
    )));
}

// A withdrawal whose answer the connection never brought goes again on
// the next connection, as one made while disconnected does: a validator's
// quorum-signed withdrawal is not lost to a reconnect.
#[test]
fn a_submit_a_connection_lost_goes_again_on_the_next() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    assert!(setup.watcher.following());
    let sequence = setup.view.next_sequence;
    setup.watcher.submit(withdrawal_blob(&setup, sequence));
    let mut sent = 0;
    while let Some(out) = setup.watcher.poll() {
        sent += usize::from(matches!(out, Out::Send(_)));
    }
    assert_eq!(sent, 1, "it went, and no answer came");
    setup.reconnect();
    setup.closes(2);
    assert!(setup.watcher.following());
    assert_eq!(setup.submitted.len(), 1, "it went again");
    setup.watcher.check_invariants();
}

/// A token hub's escrow through every act of its issuer: a deposit, XRP
/// sent as gas, a freeze, a deep freeze and a global freeze of it, each
/// lifted, a transfer fee set and removed, and a clawback; each act a few
/// ledgers apart. The watcher's server tells `lie` on every connection.
fn issuer_scene(lie: Option<Lie>) -> Setup {
    let mut setup = Setup::of(Some(DSUSD));
    setup.lie = lie;
    setup.quorum = true;
    setup.connect();
    let escrow = setup.escrow();
    let pause = |setup: &mut Setup| {
        for _ in 0..ISSUER_CHECK_EVERY_MS / 4_000 + 3 {
            setup.closes(1);
        }
        setup.tick(STALL_MS);
    };
    pause(&mut setup);
    setup.deposit_dsusd("5", 7);
    setup.xrpl.deposit_tagged(escrow, 2_000_000, Some(9));
    pause(&mut setup);
    let acts: [&dyn Fn(&mut FakeLedger); 8] = [
        &|xrpl| xrpl.freeze(escrow, DSUSD, true).unwrap(),
        &|xrpl| xrpl.freeze(escrow, DSUSD, false).unwrap(),
        &|xrpl| xrpl.deep_freeze(escrow, DSUSD, true).unwrap(),
        &|xrpl| xrpl.freeze(escrow, DSUSD, false).unwrap(),
        &|xrpl| xrpl.global_freeze(DSUSD_ISSUER, true).unwrap(),
        &|xrpl| xrpl.global_freeze(DSUSD_ISSUER, false).unwrap(),
        &|xrpl| xrpl.set_transfer_rate(DSUSD_ISSUER, 1_002_000_000).unwrap(),
        &|xrpl| xrpl.set_transfer_rate(DSUSD_ISSUER, 0).unwrap(),
    ];
    for (index, act) in acts.iter().enumerate() {
        act(&mut setup.xrpl);
        setup.rate = match index {
            6 => 1_002_000_000,
            _ => 0,
        };
        pause(&mut setup);
    }
    let clawed = DSUSD.amount(TokenValue::parse("1.5").unwrap());
    setup.xrpl.claw_back(escrow, clawed).unwrap();
    pause(&mut setup);
    setup
}

#[test]
fn the_issuers_acts_are_attested_from_proofs() {
    let setup = issuer_scene(None);
    assert!(setup.view.checked, "a token hub's escrow passes its check");
    let deposits: Vec<_> = setup
        .attested
        .iter()
        .filter_map(|op| match op {
            ValidatorOperation::AttestDeposit(deposit) => Some((deposit.tag, deposit.amount)),
            _ => None,
        })
        .collect();
    assert_eq!(deposits, [(7, 5_000_000)], "XRP sent to it is gas");
    assert_eq!(
        setup.issuer_states(),
        [
            (true, false),
            (false, false),
            (true, false),
            (false, false),
            (true, false),
            (false, false),
            (false, true),
            (false, false),
        ]
    );
    for op in &setup.attested {
        if let ValidatorOperation::AttestIssuer(issuer) = op {
            assert_eq!(
                setup.truth.get(&issuer.ledger),
                Some(&(issuer.frozen, issuer.fee)),
                "as of the ledger it names"
            );
        }
    }
    let clawbacks: Vec<_> = setup
        .attested
        .iter()
        .filter_map(|op| match op {
            ValidatorOperation::AttestClawback(clawback) => Some(clawback.amount),
            _ => None,
        })
        .collect();
    assert_eq!(clawbacks, [1_500_000]);
}

// Under every lie a server can tell, the watcher attests nothing that no
// validated ledger holds: what it attests is what it attests on an honest
// server, or an issuer's hold the ledger held as of the ledger it names.
#[test]
fn a_lying_server_makes_it_attest_nothing_false() {
    let honest = issuer_scene(None);
    for lie in Lie::ALL {
        let lied = issuer_scene(Some(lie));
        for op in &lied.attested {
            match op {
                ValidatorOperation::AttestIssuer(issuer) => assert_eq!(
                    lied.truth.get(&issuer.ledger),
                    Some(&(issuer.frozen, issuer.fee)),
                    "{lie:?}: a hold the ledger held"
                ),
                ValidatorOperation::AttestReset { .. } => panic!("{lie:?}: a reset"),
                other => assert!(honest.attested.contains(other), "{lie:?}: {other:?}"),
            }
        }
        assert!(
            lied.xrpl.lies_told(lie) > 0 || lie == Lie::NoProofs,
            "{lie:?} was told"
        );
    }
}

/// A withdrawal's Payment from the escrow at `sequence`, signed by a key
/// of no signer list: the server refuses it, which the watcher reads as
/// nothing.
fn withdrawal_blob(setup: &Setup, sequence: u32) -> Vec<u8> {
    let key = Wallet::from_seed([3; 16]);
    Payment {
        account: setup.escrow(),
        destination: key.account(),
        destination_tag: None,
        amount: Amount::Xrp(1_000_000),
        send_max: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(key.public_key()),
    }
    .sign(&key)
    .blob
}

/// The deposits it attested, by transaction, in order.
fn deposits(setup: &Setup) -> Vec<[u8; 32]> {
    setup
        .attested
        .iter()
        .filter_map(|op| match op {
            ValidatorOperation::AttestDeposit(deposit) => Some(deposit.transaction.0),
            _ => None,
        })
        .collect()
}

/// The issuer's hold as of `ledger`: a freeze, or none.
const fn hold(ledger: u32, frozen: bool) -> Issuer {
    Issuer {
        ledger,
        frozen,
        fee: false,
    }
}

// spec/decisions/token-hubs.md: an escrow transaction handed over before a
// quorum attested a freeze, while the connection was down, goes to no
// server during the freeze, on a new connection or later; it goes once the
// hub thaws.
#[test]
fn a_withdrawal_queued_before_a_freeze_waits_for_the_thaw() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    let sequence = setup.view.next_sequence;
    setup.watcher.submit(withdrawal_blob(&setup, sequence));
    setup.view.issuer = hold(1, true);
    setup.connect();
    setup.closes(3);
    assert!(setup.watcher.following());
    assert!(setup.submitted.is_empty(), "nothing goes while frozen");
    setup.reconnect();
    setup.run();
    setup.closes(3);
    assert!(setup.watcher.following());
    assert!(setup.submitted.is_empty(), "nor on a new connection");

    setup.view.issuer = hold(2, false);
    setup.tick(1);
    assert_eq!(setup.submitted.len(), 1, "it goes on the thaw");
}

// A withdrawal that stays open is sent again every RESUBMIT_MS without a
// reconnect, at once on a new connection, and no more once it settled.
#[test]
fn an_open_withdrawal_is_sent_again_until_it_settles() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    let sequence = setup.view.next_sequence;
    setup.watcher.submit(withdrawal_blob(&setup, sequence));
    setup.run();
    assert_eq!(setup.submitted.len(), 1);
    setup.tick(RESUBMIT_MS - 1);
    assert_eq!(setup.submitted.len(), 1, "not before its time");
    setup.tick(1);
    assert_eq!(setup.submitted.len(), 2, "sent again");
    setup.watcher.disconnected();
    setup.connect();
    assert_eq!(setup.submitted.len(), 3, "and on a new connection");

    // The hub settled it: its sequence was given out, and it is not open.
    setup.view.next_sequence = sequence + 1;
    for _ in 0..3 {
        setup.tick(RESUBMIT_MS);
    }
    assert_eq!(setup.submitted.len(), 3, "no more once settled");
}

// A deposit's attestation holds the cursor until the hub recorded it, so a
// restart reads it again; once recorded, the cursor moves on.
#[test]
fn a_deposit_holds_the_cursor_until_the_hub_records_it() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.view.deposit_room = 0;
    setup.recording = false;
    setup.connect();
    setup.closes(2);
    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    let deposit_ledger = setup.xrpl.ledger();
    setup.closes(3);
    assert_eq!(deposits(&setup).len(), 1);
    assert!(
        setup.cursor.unwrap().scanned_to < deposit_ledger,
        "it waits"
    );

    // A restart reads it again.
    setup.attested.clear();
    setup.watcher = watcher(escrow, setup.cursor);
    setup.connect();
    setup.closes(2);
    let attested = deposits(&setup);
    assert_eq!(attested.len(), 1, "read again after a restart");
    assert!(setup.cursor.unwrap().scanned_to < deposit_ledger);

    setup.view.deposits_recorded.push(attested[0]);
    setup.tick(1);
    assert!(
        setup.cursor.unwrap().scanned_to >= deposit_ledger,
        "recorded"
    );
}

// A deposit a full share refused during a freeze is attested again once
// the hub thaws and the share has room, by the watcher itself, whatever
// the hub's service kept; then it is recorded, and attested no more.
#[test]
fn a_deposit_refused_during_a_freeze_is_attested_again_after_the_thaw() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.view.issuer = hold(1, true);
    setup.view.deposit_room = 0;
    setup.recording = false;
    setup.connect();
    setup.closes(2);
    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    let deposit_ledger = setup.xrpl.ledger();
    setup.closes(3);
    for _ in 0..10 {
        setup.tick(10_000);
    }
    assert_eq!(deposits(&setup).len(), 1, "not again while frozen");

    let margin = setup.view.rules.terms.timers.margin;
    setup.view.issuer = hold(2, false);
    setup.view.deposit_room = 4;
    setup.tick(1);
    assert_eq!(deposits(&setup).len(), 1, "not before the margin");
    setup.tick(margin);
    let attested = deposits(&setup);
    assert_eq!(attested.len(), 2, "again after the thaw");
    assert_eq!(attested[0], attested[1]);

    setup.view.deposits_recorded.push(attested[0]);
    setup.tick(1);
    for _ in 0..10 {
        setup.tick(margin);
    }
    assert_eq!(deposits(&setup).len(), 2, "no more once recorded");
    assert!(setup.cursor.unwrap().scanned_to >= deposit_ledger);
}

// spec/areas/hubs.md#payout-bundles: once a hub stopped while the issuer
// held it, no block shows a thaw. The watcher still proves the escrow's
// line, and tells the validator of each change: the thaw, and a freeze
// again after it.
#[test]
fn a_stopped_hubs_validator_hears_of_each_change_it_proved() {
    let mut setup = Setup::of(Some(DSUSD));
    setup.view.checked = true;
    setup.view.issuer = hold(1, true);
    let escrow = setup.escrow();
    setup.xrpl.freeze(escrow, DSUSD, true).unwrap();
    setup.close();
    setup.connect();
    let pause = |setup: &mut Setup| {
        for _ in 0..ISSUER_CHECK_EVERY_MS / 4_000 + 3 {
            setup.closes(1);
        }
        setup.tick(STALL_MS);
    };
    pause(&mut setup);
    assert!(setup.issuer_states().is_empty(), "as the hub holds it");
    assert!(setup.watcher.proven_issuer().unwrap().frozen);

    setup.xrpl.freeze(escrow, DSUSD, false).unwrap();
    pause(&mut setup);
    assert_eq!(setup.issuer_states(), [(false, false)]);
    assert!(!setup.watcher.proven_issuer().unwrap().holds());

    setup.xrpl.freeze(escrow, DSUSD, true).unwrap();
    pause(&mut setup);
    assert_eq!(setup.issuer_states(), [(false, false), (true, false)]);
    assert!(setup.watcher.proven_issuer().unwrap().frozen);
}

/// How often the watcher submitted `blob`.
fn times_sent(setup: &Setup, blob: &[u8]) -> usize {
    let hex: String = blob.iter().map(|byte| format!("{byte:02X}")).collect();
    (setup.submitted.iter())
        .filter(|request| String::from_utf8_lossy(request).contains(&hex))
        .count()
}

// More withdrawals open than the connection keeps each go once: one sent
// already makes room for the next, and is only not sent again. The one
// that makes room is of the highest sequence: the escrow's sequences apply
// in order, so the lowest, which every later one waits on, is still sent
// again.
#[test]
fn more_open_withdrawals_than_kept_each_go() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    let count = MAX_QUEUED_SUBMITS + 8;
    let first = setup.view.next_sequence;
    let last = first + u32::try_from(count - 1).unwrap();
    for offset in 0..count {
        let sequence = first + u32::try_from(offset).unwrap();
        setup.watcher.submit(withdrawal_blob(&setup, sequence));
        setup.run();
    }
    assert_eq!(setup.submitted.len(), count);
    setup.tick(RESUBMIT_MS);
    assert_eq!(
        times_sent(&setup, &withdrawal_blob(&setup, first)),
        2,
        "the lowest goes again"
    );
    assert_eq!(times_sent(&setup, &withdrawal_blob(&setup, last)), 2);
    assert_eq!(
        times_sent(&setup, &withdrawal_blob(&setup, last - 1)),
        1,
        "one of the highest made room"
    );
}

// In a burst of more withdrawals than are kept, the lowest go out at once,
// as many as may be in flight, and the rest wait to be sent: no withdrawal
// in flight makes room while a higher one waits, since that one would wait
// on it for good were it lost. The highest, which none waits on, is
// dropped instead, and the others each go.
#[test]
fn a_burst_drops_its_highest_and_keeps_those_in_flight() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    let count = MAX_QUEUED_SUBMITS + 1;
    let first = setup.view.next_sequence;
    let last = first + u32::try_from(count - 1).unwrap();
    for offset in 0..count {
        let sequence = first + u32::try_from(offset).unwrap();
        setup.watcher.submit(withdrawal_blob(&setup, sequence));
    }
    let kept: Vec<Option<u32>> = setup
        .watcher
        .submits
        .iter()
        .map(|kept| kept.sequence)
        .collect();
    let expected: Vec<Option<u32>> = (first..last).map(Some).collect();
    assert_eq!(kept, expected, "one in flight made room");
    setup.run();
    for _ in 0..count {
        setup.tick(RESUBMIT_MS);
    }
    for sequence in first..last {
        assert!(
            times_sent(&setup, &withdrawal_blob(&setup, sequence)) >= 1,
            "{sequence} never went"
        );
    }
    assert_eq!(times_sent(&setup, &withdrawal_blob(&setup, last)), 0);
}

// spec/decisions/ledger-resets.md: a reset of the test network writes off
// the old chain's withdrawals. What the escrow's signers signed for it is
// not sent to the new chain, however long it stays.
#[test]
fn the_old_chains_withdrawals_are_not_sent_to_the_new_one() {
    let mut setup = Setup::new();
    setup.view.checked = true;
    setup.connect();
    setup.closes(2);
    let blob = withdrawal_blob(&setup, setup.view.next_sequence);
    setup.watcher.submit(blob.clone());
    setup.run();
    assert_eq!(times_sent(&setup, &blob), 1);

    setup.xrpl.reset();
    setup.close();
    setup.watcher.disconnected();
    setup.connect();
    setup.closes(2);
    assert!(
        any(&setup.attested, |op| matches!(
            op,
            ValidatorOperation::AttestReset { .. }
        )),
        "the reset is attested"
    );
    for _ in 0..3 {
        setup.tick(RESUBMIT_MS);
    }
    assert_eq!(times_sent(&setup, &blob), 1, "not on the new chain");
}

// A freeze proven after a thaw the validator heard of, whose attestation
// the host dropped, is attested again once proven again: else the
// validator of a stopped hub would go on signing by the thaw.
#[test]
fn a_dropped_freeze_is_attested_again() {
    let mut setup = Setup::of(Some(DSUSD));
    setup.view.checked = true;
    setup.view.issuer = hold(1, true);
    let escrow = setup.escrow();
    setup.xrpl.freeze(escrow, DSUSD, true).unwrap();
    setup.close();
    setup.connect();
    let pause = |setup: &mut Setup| {
        for _ in 0..ISSUER_CHECK_EVERY_MS / 4_000 + 3 {
            setup.closes(1);
        }
        setup.tick(STALL_MS);
    };
    pause(&mut setup);
    setup.xrpl.freeze(escrow, DSUSD, false).unwrap();
    pause(&mut setup);
    assert_eq!(setup.issuer_states(), [(false, false)]);

    setup.taking = false;
    setup.xrpl.freeze(escrow, DSUSD, true).unwrap();
    pause(&mut setup);
    assert_eq!(setup.issuer_states(), [(false, false), (true, false)]);
    for (operation, transaction) in core::mem::take(&mut setup.untaken) {
        setup.watcher.dropped(&operation, transaction);
    }
    setup.taking = true;
    pause(&mut setup);
    assert_eq!(
        setup.issuer_states().last(),
        Some(&(true, false)),
        "attested again"
    );
    assert_eq!(
        setup.issuer_states().iter().filter(|s| s.0).count(),
        2,
        "{:?}",
        setup.issuer_states()
    );
}
