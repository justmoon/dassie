//! Random sequences of operations against the ledger
//! (spec/areas/testing.md#property-tests):
//! the invariants hold after every step, no transfer that checks limits
//! breaks one that held, and the journal alone rebuilds the ledger. And the
//! decoders of what the network and the database hand the ledger never
//! panic on arbitrary input (spec/decisions/tigerstyle.md): names, ledger
//! IDs, account paths, and damaged rows to restore from.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

mod support;

use dassie_ledger::{
    AccountId, AccountName, AccountPath, Config, Ledger, LedgerId, MAX_TOTAL, Name, OwnerSub,
    Reference, StoredAccount, TraceId, Transfer, TransferId, TransferSpec,
};
use proptest::prelude::*;
use support::{Mirror, path};

#[derive(Clone, Debug)]
enum Op {
    /// Open the token sub-account `n`.
    OpenToken(u8),
    /// Create a transfer between the `from`-th and `to`-th open accounts.
    Create {
        from: u8,
        to: u8,
        amount: u64,
        pending: bool,
        kind: u8,
    },
    /// Post or void the `n`-th pending transfer.
    Resolve { n: u8, post: bool },
    /// Persist the journal.
    Drain,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        1 => (0u8..6).prop_map(Op::OpenToken),
        6 => (any::<u8>(), any::<u8>(), amount(), any::<bool>(), 0u8..4).prop_map(
            |(from, to, amount, pending, kind)| Op::Create { from, to, amount, pending, kind }
        ),
        3 => (any::<u8>(), any::<bool>()).prop_map(|(n, post)| Op::Resolve { n, post }),
        1 => Just(Op::Drain),
    ]
}

fn amount() -> impl Strategy<Value = u64> {
    prop_oneof![0u64..20, 0u64..1_000_000, Just(MAX_TOTAL), any::<u64>()]
}

fn reference(kind: u8, sequence: u64) -> Reference {
    match kind {
        0 | 1 => Reference::Packet(TraceId { epoch: 0, sequence }),
        2 => Reference::Deposit,
        _ => Reference::Settlement {
            scheme: Name::new("stub").unwrap(),
            id: Name::new(&format!("s{sequence}")).unwrap(),
        },
    }
}

fn pick<T: Copy>(items: &[T], n: u8) -> Option<T> {
    (!items.is_empty()).then(|| items[usize::from(n) % items.len()])
}

/// Whether every limit holds now, per account and per group.
fn limits_held(ledger: &Ledger) -> Vec<bool> {
    let mut held = Vec::new();
    for (id, account) in ledger.accounts() {
        held.push(account.balance().within(account.path().limit()));
        if let Some(group) = ledger.group_balance(id) {
            held.push(group.within(account.path().group_limit()));
        }
    }
    held
}

const CONFIG: Config = Config {
    max_accounts: 32,
    max_pending: 16,
    max_journal: 64,
};

fn setup() -> (Ledger, Vec<AccountId>) {
    let mut ledger = Ledger::new(CONFIG);
    let peer = Name::new("peer").unwrap();
    let mut accounts = vec![
        ledger.open(path(AccountName::AssetsSettlement)).unwrap(),
        ledger.open(path(AccountName::EquityOwner)).unwrap(),
        ledger.open(path(AccountName::EquitySuspense)).unwrap(),
    ];
    let (interledger, _) = ledger.open_peer(0, LedgerId::StubUsd, peer, 50).unwrap();
    accounts.push(interledger);
    accounts.push(ledger.find(&path(AccountName::ContraTrust(peer))).unwrap());
    (ledger, accounts)
}

/// Plays `ops`, checking the ledger after each, and returns the rows the
/// journal persisted.
fn run(ops: &[Op]) -> Mirror {
    let (mut ledger, mut accounts) = setup();
    let mut mirror = Mirror::default();
    mirror.drain(&mut ledger);
    for (step, op) in ops.iter().enumerate() {
        let before = limits_held(&ledger);
        let checked = match *op {
            Op::OpenToken(n) => {
                let token = path(AccountName::EquityOwnerSub(OwnerSub::Btp(u64::from(n))));
                if let Ok(id) = ledger.open(token)
                    && !accounts.contains(&id)
                {
                    accounts.push(id);
                }
                true
            }
            Op::Create {
                from,
                to,
                amount,
                pending,
                kind,
            } => {
                let (Some(debit), Some(credit)) = (pick(&accounts, from), pick(&accounts, to))
                else {
                    continue;
                };
                let reference = reference(kind, step as u64);
                let _ = ledger.create(
                    step as u64,
                    &TransferSpec {
                        debit,
                        credit,
                        amount,
                        pending,
                        reference,
                    },
                );
                !reference.is_report()
            }
            Op::Resolve { n, post } => {
                let pending: Vec<_> = ledger.pending_transfers().map(|t| t.id).collect();
                if let Some(id) = pick(&pending, n) {
                    let _ = if post {
                        ledger.post(step as u64, id)
                    } else {
                        ledger.void(step as u64, id)
                    };
                }
                true
            }
            Op::Drain => {
                mirror.drain(&mut ledger);
                true
            }
        };
        ledger.check_invariants();
        if checked {
            // Accounts opened in this step held trivially before it.
            let after = limits_held(&ledger);
            for (index, held) in before.iter().enumerate() {
                assert!(!held || after[index], "step {step} broke a limit: {op:?}");
            }
        }
    }
    mirror.drain(&mut ledger);
    let restored = mirror.restore(ledger.config());
    restored.check_invariants();
    for (id, account) in ledger.accounts() {
        let other = restored.account(id).unwrap();
        assert_eq!(other.path(), account.path());
        let (mine, theirs) = (account.balance(), other.balance());
        assert_eq!(
            theirs.debits_posted,
            mine.debits_posted,
            "{}",
            account.path()
        );
        assert_eq!(
            theirs.credits_posted,
            mine.credits_posted,
            "{}",
            account.path()
        );
    }
    let durable: Vec<_> = ledger
        .pending_transfers()
        .filter(|t| t.reference.is_durable_while_pending())
        .copied()
        .collect();
    let restored_pending: Vec<_> = restored.pending_transfers().copied().collect();
    assert_eq!(restored_pending, durable);
    mirror
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_operations_keep_the_ledger_sound(ops in prop::collection::vec(op(), 0..120)) {
        run(&ops);
    }
}

// What the decoders get: text from peers, RPC callers and the database.

/// Characters that are not in a name, or look like ones that are.
const ODD: &[char] = &[
    ':', '/', '+', '-', '_', '.', ' ', '0', 'a', 'Z', 'é', '\0', '😀', '\u{212a}', 'ſ', '\u{301}',
    '／', '：',
];

/// A change to a text: a character inserted, removed or replaced, or one
/// repeated `count` more times, which makes parts too long.
#[derive(Clone, Debug)]
enum Edit {
    Insert(u8, char),
    Remove(u8),
    Replace(u8, char),
    Stretch(u8, u8),
}

fn edit() -> impl Strategy<Value = Edit> {
    let odd = prop::sample::select(ODD);
    prop_oneof![
        (any::<u8>(), odd.clone()).prop_map(|(at, c)| Edit::Insert(at, c)),
        any::<u8>().prop_map(Edit::Remove),
        (any::<u8>(), odd).prop_map(|(at, c)| Edit::Replace(at, c)),
        (any::<u8>(), 1u8..80).prop_map(|(at, count)| Edit::Stretch(at, count)),
    ]
}

fn apply(text: &str, edits: &[Edit]) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for edit in edits {
        let len = chars.len();
        let at = |n: u8| usize::from(n) % len.max(1);
        match *edit {
            Edit::Insert(n, c) => chars.insert(usize::from(n) % (len + 1), c),
            Edit::Remove(n) if len > 0 => drop(chars.remove(at(n))),
            Edit::Replace(n, c) if len > 0 => chars[at(n)] = c,
            Edit::Stretch(n, count) if len > 0 => {
                let c = chars[at(n)];
                chars.splice(at(n)..at(n), std::iter::repeat_n(c, usize::from(count)));
            }
            _ => {}
        }
    }
    chars.into_iter().collect()
}

/// `valid` texts with a few edits, or none.
fn edited(valid: impl Strategy<Value = String>) -> impl Strategy<Value = String> {
    (valid, prop::collection::vec(edit(), 0..4)).prop_map(|(text, edits)| apply(&text, &edits))
}

fn name() -> impl Strategy<Value = Name> {
    "[A-Za-z0-9_-]{1,64}".prop_map(|text| Name::new(&text).unwrap())
}

fn ledger_id() -> impl Strategy<Value = LedgerId> {
    prop::sample::select(LedgerId::ALL.to_vec())
}

fn account_path() -> impl Strategy<Value = AccountPath> {
    let fixed = prop::sample::select(vec![
        AccountName::AssetsSettlement,
        AccountName::EquityOwner,
        AccountName::EquitySuspense,
        AccountName::RevenueFees,
        AccountName::RevenueFx,
        AccountName::ExpensesFees,
        AccountName::ExpensesFx,
        AccountName::ExpensesLosses,
    ]);
    let account = prop_oneof![
        fixed,
        name().prop_map(AccountName::AssetsInterledger),
        name().prop_map(AccountName::ContraTrust),
        any::<u64>().prop_map(|id| AccountName::EquityOwnerSub(OwnerSub::Btp(id))),
        name().prop_map(|name| AccountName::EquityOwnerSub(OwnerSub::Local(name))),
    ];
    (ledger_id(), account).prop_map(|(ledger, name)| AccountPath::new(ledger, name))
}

/// Text put together from the pieces paths are made of, mostly not a path.
fn pieced_path() -> impl Strategy<Value = String> {
    let words = prop::sample::select(vec![
        "assets",
        "interledger",
        "settlement",
        "contra",
        "trust",
        "equity",
        "owner",
        "local",
        "btp",
        "suspense",
        "revenue",
        "expenses",
        "fees",
        "fx",
        "losses",
        "liabilities",
    ]);
    let piece = prop_oneof![
        1 => ledger_id().prop_map(|ledger| ledger.as_str().to_owned()),
        2 => words.prop_map(str::to_owned),
        1 => Just(":".to_owned()),
        3 => Just("/".to_owned()),
        1 => "[A-Za-z0-9_-]{0,70}",
        1 => "0|[1-9][0-9]{0,21}|0[0-9]{1,3}",
        1 => prop::sample::select(ODD).prop_map(String::from),
    ];
    prop::collection::vec(piece, 0..10).prop_map(|pieces| pieces.concat())
}

fn check_name(text: &str) {
    let expected = (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let name = Name::new(text);
    assert_eq!(name.is_ok(), expected, "{text:?}");
    if let Ok(name) = name {
        assert_eq!(name.as_str(), text);
        assert_eq!(name.to_string(), text);
    }
}

fn check_ledger_id(text: &str) {
    if let Some(ledger) = LedgerId::parse(text) {
        assert_eq!(ledger.as_str(), text);
        assert_eq!(ledger.to_string(), text);
    }
}

fn check_path(text: &str) {
    if let Ok(path) = AccountPath::parse(text) {
        assert_eq!(path.to_string(), text);
    }
}

// Rows a damaged or foreign database could hold.

/// A pending settlement, which the database keeps: most runs end without
/// one, and damage to them is what reaches the pending transfers' checks.
fn settlement() -> impl Strategy<Value = Op> {
    (any::<u8>(), any::<u8>(), 1u64..1000).prop_map(|(from, to, amount)| Op::Create {
        from,
        to,
        amount,
        pending: true,
        kind: 3,
    })
}

/// The rows to restore from: lists rather than the mirror's maps, so that
/// damage can reorder and repeat them.
#[derive(Clone, Debug)]
struct Rows {
    accounts: Vec<StoredAccount>,
    pending: Vec<Transfer>,
    next_transfer: u64,
}

/// One kind of damage to the rows. `n` and `m` pick rows, as [`pick`] does.
#[derive(Clone, Debug)]
enum Damage {
    /// Set an account's posted totals.
    Totals {
        n: u8,
        debits: u64,
        credits: u64,
    },
    /// Post `amount` more from the `n`-th account to the `m`-th: balanced
    /// if they share a ledger, so that restoring gets as far as the
    /// pending transfers.
    Shift {
        n: u8,
        m: u8,
        amount: u64,
    },
    /// Give an account another ID.
    AccountId {
        n: u8,
        id: u32,
    },
    /// Give an account another path.
    AccountPath {
        n: u8,
        path: AccountPath,
    },
    /// Give the `m`-th account the `n`-th's path.
    CopyPath {
        n: u8,
        m: u8,
    },
    /// Swap two accounts' paths and totals, keeping their IDs: a group
    /// member may come before its root, or a transfer cross ledgers.
    SwapPaths(u8, u8),
    /// Swap two account rows, IDs and all.
    SwapAccounts(u8, u8),
    RepeatAccount(u8),
    DropAccount(u8),
    /// Point a pending transfer's debit or credit at another account ID.
    PendingAccount {
        n: u8,
        debit: bool,
        id: u32,
    },
    /// Credit a pending transfer to the account it debits.
    PendingSelf(u8),
    /// Give a pending transfer the ID `offset` from the next one.
    PendingId {
        n: u8,
        offset: i8,
    },
    PendingIdTo {
        n: u8,
        id: u64,
    },
    PendingAmount {
        n: u8,
        amount: u64,
    },
    PendingReference {
        n: u8,
        kind: u8,
    },
    SwapPending(u8, u8),
    RepeatPending(u8),
    DropPending(u8),
    /// Add a settlement from scratch, `back` IDs before the next one.
    AddPending {
        debit: u32,
        credit: u32,
        amount: u64,
        back: u8,
    },
    /// Move the next transfer ID by `offset`.
    NextTransfer(i8),
    NextTransferTo(u64),
}

/// Amounts and totals, many of them at the edge of [`MAX_TOTAL`].
fn total() -> impl Strategy<Value = u64> {
    prop_oneof![
        amount(),
        (MAX_TOTAL - 2)..=(MAX_TOTAL + 2),
        (MAX_TOTAL / 2 - 2)..=(MAX_TOTAL / 2 + 2),
        Just(u64::MAX),
    ]
}

fn stored_id() -> impl Strategy<Value = u32> {
    prop_oneof![8 => 0u32..6, 1 => 0u32..40, 1 => Just(u32::MAX), 1 => any::<u32>()]
}

fn transfer_id() -> impl Strategy<Value = u64> {
    prop_oneof![0u64..200, Just(u64::MAX), any::<u64>()]
}

/// Damage to the accounts is rarer: most of it stops restoring before it
/// gets to the pending transfers.
fn damage() -> impl Strategy<Value = Damage> {
    let pair = || (any::<u8>(), any::<u8>());
    prop_oneof![
        1 => (any::<u8>(), total(), total())
            .prop_map(|(n, debits, credits)| Damage::Totals { n, debits, credits }),
        3 => (any::<u8>(), any::<u8>(), total()).prop_map(|(n, m, amount)| Damage::Shift {
            n,
            m,
            amount
        }),
        1 => (any::<u8>(), stored_id()).prop_map(|(n, id)| Damage::AccountId { n, id }),
        1 => (any::<u8>(), account_path()).prop_map(|(n, path)| Damage::AccountPath { n, path }),
        1 => pair().prop_map(|(n, m)| Damage::CopyPath { n, m }),
        1 => pair().prop_map(|(n, m)| Damage::SwapPaths(n, m)),
        1 => pair().prop_map(|(n, m)| Damage::SwapAccounts(n, m)),
        1 => any::<u8>().prop_map(Damage::RepeatAccount),
        1 => any::<u8>().prop_map(Damage::DropAccount),
        2 => (any::<u8>(), any::<bool>(), stored_id())
            .prop_map(|(n, debit, id)| Damage::PendingAccount { n, debit, id }),
        2 => any::<u8>().prop_map(Damage::PendingSelf),
        2 => (any::<u8>(), -3i8..3).prop_map(|(n, offset)| Damage::PendingId { n, offset }),
        1 => (any::<u8>(), transfer_id()).prop_map(|(n, id)| Damage::PendingIdTo { n, id }),
        2 => (any::<u8>(), total()).prop_map(|(n, amount)| Damage::PendingAmount { n, amount }),
        2 => (any::<u8>(), 0u8..9).prop_map(|(n, kind)| Damage::PendingReference { n, kind }),
        2 => pair().prop_map(|(n, m)| Damage::SwapPending(n, m)),
        2 => any::<u8>().prop_map(Damage::RepeatPending),
        1 => any::<u8>().prop_map(Damage::DropPending),
        8 => (stored_id(), stored_id(), prop_oneof![1u64..1000, total()], 0u8..20).prop_map(
            |(debit, credit, amount, back)| Damage::AddPending {
                debit,
                credit,
                amount,
                back
            }
        ),
        1 => (-3i8..3).prop_map(Damage::NextTransfer),
        1 => transfer_id().prop_map(Damage::NextTransferTo),
    ]
}

/// Any reference, durable while pending or not.
fn any_reference(kind: u8) -> Reference {
    let trace = TraceId {
        epoch: 1,
        sequence: 2,
    };
    let stub = Name::new("stub").unwrap();
    match kind {
        0 => Reference::Packet(trace),
        1 => Reference::Trust,
        2 => Reference::Deposit,
        3 => Reference::Withdrawal,
        4 => Reference::Reconciliation,
        5 => Reference::Fee {
            scheme: stub,
            id: stub,
        },
        6 => Reference::Loss {
            scheme: stub,
            id: stub,
        },
        7 => Reference::OwnerPayment {
            scheme: stub,
            id: stub,
        },
        _ => Reference::Settlement {
            scheme: stub,
            id: Name::new("damaged").unwrap(),
        },
    }
}

/// The index `n` picks from `len` rows.
fn index(len: usize, n: u8) -> Option<usize> {
    (len > 0).then(|| usize::from(n) % len)
}

impl Rows {
    fn new(mirror: &Mirror) -> Self {
        Self {
            accounts: mirror.accounts.clone(),
            pending: mirror.durable_pending.values().copied().collect(),
            next_transfer: mirror.next_transfer,
        }
    }

    fn damage(&mut self, damage: &Damage) {
        let accounts = self.accounts.len();
        let account = |n| index(accounts, n);
        match *damage {
            Damage::Totals { n, debits, credits } => {
                if let Some(i) = account(n) {
                    self.accounts[i].debits_posted = debits;
                    self.accounts[i].credits_posted = credits;
                }
            }
            Damage::Shift { n, m, amount } => {
                if let (Some(i), Some(j)) = (account(n), account(m)) {
                    let debits = &mut self.accounts[i].debits_posted;
                    *debits = debits.saturating_add(amount);
                    let credits = &mut self.accounts[j].credits_posted;
                    *credits = credits.saturating_add(amount);
                }
            }
            Damage::AccountId { n, id } => {
                if let Some(i) = account(n) {
                    self.accounts[i].id = AccountId(id);
                }
            }
            Damage::AccountPath { n, path } => {
                if let Some(i) = account(n) {
                    self.accounts[i].path = path;
                }
            }
            Damage::CopyPath { n, m } => {
                if let (Some(i), Some(j)) = (account(n), account(m)) {
                    self.accounts[j].path = self.accounts[i].path;
                }
            }
            Damage::SwapPaths(n, m) => {
                if let (Some(i), Some(j)) = (account(n), account(m)) {
                    let (a, b) = (self.accounts[i], self.accounts[j]);
                    self.accounts[i] = StoredAccount { id: a.id, ..b };
                    self.accounts[j] = StoredAccount { id: b.id, ..a };
                }
            }
            Damage::SwapAccounts(n, m) => {
                if let (Some(i), Some(j)) = (account(n), account(m)) {
                    self.accounts.swap(i, j);
                }
            }
            Damage::RepeatAccount(n) => {
                if let Some(i) = account(n) {
                    self.accounts.insert(i, self.accounts[i]);
                }
            }
            Damage::DropAccount(n) => {
                if let Some(i) = account(n) {
                    self.accounts.remove(i);
                }
            }
            _ => self.damage_pending(damage),
        }
    }

    fn damage_pending(&mut self, damage: &Damage) {
        let pending = self.pending.len();
        let transfer = |n| index(pending, n);
        match *damage {
            Damage::PendingAccount { n, debit, id } => {
                if let Some(i) = transfer(n) {
                    if debit {
                        self.pending[i].debit = AccountId(id);
                    } else {
                        self.pending[i].credit = AccountId(id);
                    }
                }
            }
            Damage::PendingSelf(n) => {
                if let Some(i) = transfer(n) {
                    self.pending[i].credit = self.pending[i].debit;
                }
            }
            Damage::PendingId { n, offset } => {
                if let Some(i) = transfer(n) {
                    let id = self.next_transfer.wrapping_add_signed(i64::from(offset));
                    self.pending[i].id = TransferId(id);
                }
            }
            Damage::PendingIdTo { n, id } => {
                if let Some(i) = transfer(n) {
                    self.pending[i].id = TransferId(id);
                }
            }
            Damage::PendingAmount { n, amount } => {
                if let Some(i) = transfer(n) {
                    self.pending[i].amount = amount;
                }
            }
            Damage::PendingReference { n, kind } => {
                if let Some(i) = transfer(n) {
                    self.pending[i].reference = any_reference(kind);
                }
            }
            Damage::SwapPending(n, m) => {
                if let (Some(i), Some(j)) = (transfer(n), transfer(m)) {
                    self.pending.swap(i, j);
                }
            }
            Damage::RepeatPending(n) => {
                if let Some(i) = transfer(n) {
                    self.pending.insert(i, self.pending[i]);
                }
            }
            Damage::DropPending(n) => {
                if let Some(i) = transfer(n) {
                    self.pending.remove(i);
                }
            }
            Damage::AddPending {
                debit,
                credit,
                amount,
                back,
            } => self.pending.push(Transfer {
                id: TransferId(self.next_transfer.saturating_sub(u64::from(back))),
                debit: AccountId(debit),
                credit: AccountId(credit),
                amount,
                reference: any_reference(6),
                created_at_ms: 0,
            }),
            Damage::NextTransfer(offset) => {
                self.next_transfer = self.next_transfer.wrapping_add_signed(i64::from(offset));
            }
            Damage::NextTransferTo(id) => self.next_transfer = id,
            _ => {}
        }
    }

    /// Restores from the rows: refused, or a ledger that is sound and
    /// holds every row.
    fn restore(&self) {
        let restored = Ledger::restore(
            CONFIG,
            self.accounts.iter().copied(),
            self.pending.iter().copied(),
            TransferId(self.next_transfer),
        );
        if let Ok(ledger) = restored {
            ledger.check_invariants();
            assert_eq!(ledger.accounts().count(), self.accounts.len());
            assert_eq!(ledger.pending_transfers().count(), self.pending.len());
            assert_eq!(ledger.next_transfer_id(), TransferId(self.next_transfer));
        }
    }
}

proptest! {
    #[test]
    fn names_never_panic(text in any::<String>()) {
        check_name(&text);
    }

    #[test]
    fn edited_names_never_panic(text in edited("[A-Za-z0-9_-]{0,70}")) {
        check_name(&text);
    }

    #[test]
    fn ledger_ids_never_panic(text in any::<String>()) {
        check_ledger_id(&text);
    }

    #[test]
    fn edited_ledger_ids_never_panic(
        text in edited(ledger_id().prop_map(|ledger| ledger.as_str().to_owned()))
    ) {
        check_ledger_id(&text);
    }

    #[test]
    fn account_paths_never_panic(text in any::<String>()) {
        check_path(&text);
    }

    #[test]
    fn account_paths_round_trip(path in account_path()) {
        assert_eq!(AccountPath::parse(&path.to_string()), Ok(path));
    }

    #[test]
    fn edited_account_paths_never_panic(
        text in prop_oneof![edited(account_path().prop_map(|path| path.to_string())), pieced_path()]
    ) {
        check_path(&text);
    }

    #[test]
    fn restore_never_panics_on_damaged_rows(
        ops in prop::collection::vec(op(), 0..60),
        settlements in prop::collection::vec(settlement(), 0..4),
        damage in prop::collection::vec(damage(), 1..6),
    ) {
        let mut rows = Rows::new(&run(&[ops, settlements].concat()));
        for damage in &damage {
            rows.damage(damage);
        }
        rows.restore();
    }
}
