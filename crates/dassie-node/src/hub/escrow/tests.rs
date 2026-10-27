//! The watcher against the fake XRP Ledger: it checks the escrow as the
//! sponsor set it up, attests what reaches it, follows across reconnects
//! without attesting twice, and attests a reset of the test network
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals,
//! spec/decisions/ledger-resets.md).

use dassie_hub::escrow::{escrow_master, setup};
use dassie_hub::ledger::TESTNET_NETWORK;
use dassie_hub::operation::ValidatorOperation;
use dassie_identity::NodeKey;
use dassie_xrpl::fake::{FEE_BASE, FakeLedger};

use super::*;
use crate::hub::config::tests::{NONCE, rules};

const CONNECTION: u64 = 1;

struct Setup {
    xrpl: FakeLedger,
    view: EscrowView,
    watcher: Watcher,
    attested: Vec<ValidatorOperation>,
    cursor: Option<EscrowCursor>,
    now: u64,
    /// Whether the validator takes each attestation at once.
    taking: bool,
    /// Attestations it has not taken yet.
    untaken: Vec<Option<[u8; 32]>>,
}

impl Setup {
    /// A hub whose sponsor set up its escrow on the fake ledger.
    fn new() -> Self {
        let mut xrpl = FakeLedger::new();
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let nodes: Vec<(NodeKey, String)> = (1..=4)
            .map(|i| (NodeKey::from_bytes(&[i; 32]), format!("v{i}.example:443")))
            .collect();
        let rules = rules(&sponsor, &nodes);
        let master = escrow_master(&sponsor, TESTNET_NETWORK, &NONCE);
        xrpl.fund(master.account());
        xrpl.close();
        let sequence = {
            let answer = xrpl.request(CONNECTION, &api::account_info(1, master.account()));
            let Message::Response { outcome, .. } = api::parse(&answer) else {
                panic!("no answer");
            };
            api::account_info_result(&outcome.unwrap())
                .unwrap()
                .sequence
        };
        for transaction in setup(&rules, &master, sequence, FEE_BASE) {
            xrpl.request(CONNECTION, &api::submit(2, transaction.blob()));
        }
        xrpl.close();
        let view = EscrowView {
            rules,
            checked: false,
            next_sequence: 0,
            withdrawals: Vec::new(),
            signer_list: None,
            written_off_at: None,
        };
        let watcher = Watcher::new(master.account(), 1, None);
        Self {
            xrpl,
            view,
            watcher,
            attested: Vec::new(),
            cursor: None,
            now: 1_000_000,
            taking: true,
            untaken: Vec::new(),
        }
    }

    fn escrow(&self) -> AccountId {
        AccountId(self.view.rules.terms.escrow)
    }

    /// Carries out what the watcher asks, as its host does, until it asks
    /// nothing more.
    fn run(&mut self) {
        while let Some(out) = self.watcher.poll() {
            match out {
                Out::Send(bytes) => {
                    let answer = self.xrpl.request(CONNECTION, &bytes);
                    self.watcher.message(self.now, &answer, &self.view);
                }
                Out::Attest(operation, transaction) => {
                    self.attested.push(operation);
                    if self.taking {
                        self.watcher.taken(transaction);
                    } else {
                        self.untaken.push(transaction);
                    }
                }
                Out::Cursor(cursor) => self.cursor = Some(cursor),
                Out::Problem(_) => {}
            }
        }
    }

    /// Closes a ledger, and passes on what the streams carry.
    fn close(&mut self) {
        for (connection, bytes) in self.xrpl.close() {
            if connection == CONNECTION {
                self.watcher.message(self.now, &bytes, &self.view);
            }
        }
        self.run();
    }

    fn tick(&mut self, ms: u64) {
        self.now += ms;
        self.watcher.tick(self.now, &self.view);
        self.run();
    }

    fn connect(&mut self) {
        self.watcher.connected();
        self.run();
    }
}

#[test]
fn the_escrow_is_checked_and_its_deposits_attested() {
    let mut setup = Setup::new();
    setup.connect();
    assert!(setup.watcher.following());
    setup.tick(1);
    let escrow_attested = setup
        .attested
        .iter()
        .any(|op| matches!(op, ValidatorOperation::AttestEscrow { .. }));
    assert!(escrow_attested, "the escrow as set up passes its check");
    // Checked once; a quorum's attestations make it so.
    setup.view.checked = true;
    setup.attested.clear();

    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    assert!(matches!(
        setup.attested.as_slice(),
        [ValidatorOperation::AttestDeposit(deposit)] if deposit.tag == 7 && deposit.amount == 5_000_000
    ));
    let cursor = setup.cursor.unwrap();
    assert_eq!(cursor.scanned_to, cursor.newest_ledger);

    // A reconnect reads the history again from a window back, and attests
    // nothing twice.
    setup.watcher.disconnected();
    setup.attested.clear();
    setup.connect();
    assert!(setup.attested.is_empty(), "{:?}", setup.attested);

    // A watcher that starts again, from its cursor, attests again what may
    // not have reached a block before the crash.
    setup.watcher = Watcher::new(escrow, 1, setup.cursor);
    setup.connect();
    assert_eq!(setup.attested.len(), 1);
}

#[test]
fn a_reset_of_the_test_network_is_attested_and_a_server_behind_is_not() {
    let mut setup = Setup::new();
    setup.connect();
    setup.view.checked = true;
    let newest = setup.cursor.unwrap();

    // A server behind: a lower ledger that closed before the newest.
    let mut behind = setup.xrpl.clone();
    setup.xrpl.close();
    setup.xrpl.close();
    setup.close();
    let mut watcher = Watcher::new(setup.escrow(), 1, setup.cursor);
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

    // A reset: a lower ledger that closed later.
    setup.xrpl.reset();
    setup.watcher.disconnected();
    setup.connect();
    let reset = setup
        .attested
        .iter()
        .find_map(|op| match op {
            ValidatorOperation::AttestReset { ledger } => Some(*ledger),
            _ => None,
        })
        .expect("the reset is attested");
    assert_eq!(
        reset,
        setup.cursor.unwrap().newest_ledger,
        "the ledger it saw"
    );
    assert!(setup.watcher.following());

    // Seen again, after another reconnect: attested no more.
    setup.attested.clear();
    setup.watcher.disconnected();
    setup.connect();
    assert!(setup.attested.is_empty(), "{:?}", setup.attested);
}

#[test]
fn a_reset_the_hub_wrote_off_already_is_not_attested() {
    let mut setup = Setup::new();
    setup.connect();
    setup.view.checked = true;
    setup.xrpl.reset();
    // A quorum saw the new chain first, and the hub wrote it off.
    setup.view.written_off_at = Some(u64::MAX);
    setup.watcher.disconnected();
    setup.attested.clear();
    setup.connect();
    assert!(setup.watcher.following());
    assert!(
        !setup
            .attested
            .iter()
            .any(|op| matches!(op, ValidatorOperation::AttestReset { .. })),
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
    setup.connect();
    setup.view.checked = true;
    let before = setup.cursor.unwrap();
    setup.taking = false;
    setup.attested.clear();
    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    setup.close();
    assert_eq!(setup.attested.len(), 1);
    assert_eq!(setup.cursor, Some(before), "the cursor waits");
    // Read again, from a reconnect, it is not attested twice.
    setup.watcher.disconnected();
    setup.connect();
    assert_eq!(setup.attested.len(), 1);
    // Once taken, the cursor moves on.
    for transaction in core::mem::take(&mut setup.untaken) {
        setup.watcher.taken(transaction);
    }
    setup.run();
    assert!(setup.cursor.unwrap().scanned_to > before.scanned_to);
}

// An attestation the host drops waits no more: the cursor moves on, but
// never past the dropped transaction's ledger, and the transaction is
// attested again when read again.
#[test]
fn a_dropped_attestation_holds_the_cursor_only_below_it() {
    let mut setup = Setup::new();
    setup.connect();
    setup.view.checked = true;
    setup.taking = false;
    setup.attested.clear();
    let escrow = setup.escrow();
    setup.xrpl.deposit_tagged(escrow, 5_000_000, Some(7));
    setup.close();
    let deposit_ledger = setup.watcher.cursor().unwrap().newest_ledger;
    setup.close();
    setup.close();
    let held = setup.cursor;
    for transaction in core::mem::take(&mut setup.untaken) {
        setup.watcher.dropped(transaction);
    }
    setup.run();
    let cursor = setup.cursor.unwrap();
    assert_ne!(setup.cursor, held, "the cursor is written again");
    assert!(
        cursor.scanned_to < deposit_ledger,
        "never past the dropped one"
    );
    assert!(cursor.newest_ledger > deposit_ledger);
    // Later ledgers move it no further than that.
    setup.close();
    assert_eq!(setup.cursor.unwrap().scanned_to, cursor.scanned_to);
    // Read again, it is attested again.
    setup.attested.clear();
    setup.watcher.disconnected();
    setup.connect();
    assert_eq!(setup.attested.len(), 1);
}
