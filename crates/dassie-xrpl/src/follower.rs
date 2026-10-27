//! What every scheme on an XRPL account does alike, whatever holding it
//! books (spec/areas/xrpl.md#following-the-account): the XRP scheme
//! ([`crate::XrpScheme`]) and the token scheme ([`crate::TokenScheme`])
//! each embed one [`Follower`], and add only what their holding needs: the
//! XRP and its reserve, or the line, its issuer and the peg guard.
//!
//! A follower keeps what the books hold of the holding, its cursor into the
//! account's transactions, the count of write-offs it last saw and the
//! owner's payment, and it writes and reads all of them as the scheme's
//! state. Each part of a scheme's state is defined here once, so the two
//! schemes cannot drift apart. Every XRPL scheme is an owner book: it peers
//! with no one, and settles with no one (spec/areas/xrpl.md#owner-books).

use alloc::vec::Vec;

use dassie_ledger::{MAX_TOTAL, Name};
use dassie_settlement::{
    Effect, OwnerPayment, OwnerPaymentState, PaymentFailure, ReconcileCause, SchemeError,
};

use crate::account::{News, XrplAccount};
use crate::api::TxRecord;
use crate::codec::Memo;
use crate::from_hex;
use crate::kept::{In, Out, PEERING_SCHEME_VERSION, SCHEME_VERSION};
use crate::keys::AccountId;
use crate::scheme::owner_memos;

/// Most peers' addresses a state written while the schemes peered holds.
const PEERING_MAX_PEERS: usize = 1536;
/// Most settlements looked up after a restart such a state holds.
const PEERING_MAX_RECOVERING: usize = 64;
/// Most entry payments such a state holds.
const PEERING_MAX_ENTRIES: usize = 256;

/// What a scheme on an XRPL account keeps of the account, alike for every
/// holding.
pub(crate) struct Follower {
    /// The account whose holding it books.
    pub(crate) account: AccountId,
    /// What the books hold of the holding, in internal units.
    pub(crate) booked: u64,
    /// The most an owner's payment pays, in the holding's whole units
    /// (drops, or millionths of a token).
    max_owner_units: u64,
    // Kept with the books.
    /// Where it read the account's transactions to, as (ledger, index):
    /// set by its first sync, dropped only with the account.
    pub(crate) cursor: Option<(u32, u32)>,
    /// How many times the account was written off, as the scheme last saw
    /// it ([`XrplAccount::write_offs`]).
    pub(crate) write_offs: u64,
    /// The owner's payment on its way: its hash, and the whole units it
    /// pays.
    pub(crate) owner_payment: Option<([u8; 32], u64)>,
    /// The owner's last payment that is no longer on its way, and what
    /// became of it, kept with the books so a restart still answers for it.
    pub(crate) owner_outcome: Option<([u8; 32], OwnerPaymentState)>,
    /// What a state written while the schemes peered held of entry
    /// payments, in internal units: in the account, and never on the books.
    /// It is the owner's deposit, as a hold that ran out was, booked the
    /// next time the account tells the scheme what it learnt
    /// ([`Self::book_entries`]); never written again.
    pub(crate) entries: u64,
    // Learnt from the account, as it told the scheme.
    /// The account's newest validated ledger.
    pub(crate) validated: u32,
}

impl Follower {
    /// A follower of `account`, whose books last checkpointed
    /// `checkpoint`, and whose owner's payments pay at most
    /// `max_owner_units`.
    pub(crate) fn new(
        account: &XrplAccount,
        checkpoint: Option<u64>,
        max_owner_units: u64,
    ) -> Self {
        Self {
            account: account.account(),
            booked: checkpoint.unwrap_or(0),
            max_owner_units,
            cursor: None,
            write_offs: 0,
            owner_payment: None,
            owner_outcome: None,
            entries: 0,
            validated: 0,
        }
    }

    /// Whether the books hold anything of the account.
    pub(crate) const fn holds_anything(&self) -> bool {
        self.cursor.is_some() || self.booked > 0
    }

    /// Before the news of an input is told: the account's newest ledger and
    /// its count of write-offs taken. Returns whether the scheme missed a
    /// write-off while it was not on the account (stopped, or removed): the
    /// account counts another, and is not telling one now, which the
    /// scheme writes off in order. What it held went with the account, and
    /// its cursor is on a chain gone, which would hold the account back
    /// (spec/areas/xrpl.md#following-the-account).
    pub(crate) fn catch_up(&mut self, account: &XrplAccount, news: &[News]) -> bool {
        self.validated = account.validated();
        let telling = news
            .iter()
            .any(|news| matches!(news, News::WrittenOff { .. }));
        let missed = self.write_offs != account.write_offs() && !telling;
        self.write_offs = account.write_offs();
        missed
    }

    /// Books what a state written while the schemes peered held of entry
    /// payments as the owner's deposit, once, when the account tells the
    /// scheme `news`: the account holds them, and the books hold them from
    /// now on, as they would have once their hold ran out. Returns whether
    /// it booked any. The scheme calls it before it takes the news, after a
    /// write-off it missed, which took them with the account, so the books
    /// match the account as it is.
    pub(crate) fn book_entries(&mut self, news: &[News], effects: &mut Vec<Effect>) -> bool {
        if news.is_empty() {
            return false;
        }
        let amount = core::mem::take(&mut self.entries);
        if amount == 0 || self.cursor.is_none() {
            return false;
        }
        effects.push(Effect::Deposit { amount });
        self.booked = self.booked.saturating_add(amount).min(MAX_TOTAL);
        true
    }

    /// After the news of an input was told: an owner's payment the account
    /// gave up while the scheme was away can never apply, and is cancelled;
    /// one it keeps no more, and told nothing of, was lost with its state,
    /// and what it did is in the holding; one it keeps for the scheme,
    /// validated while the scheme was away, it tells again. Then the
    /// account is told where the scheme is.
    pub(crate) fn caught_up(
        &mut self,
        account: &mut XrplAccount,
        scheme: Name,
        effects: &mut Vec<Effect>,
    ) {
        if let Some((hash, _)) = self.owner_payment {
            if account.take_never_sent(scheme, hash) {
                self.owner_done(OwnerPaymentState::Cancelled, effects);
            } else if !account.keeps_transaction(&hash) {
                self.owner_done(OwnerPaymentState::Unknown, effects);
            }
        }
        account.follow(scheme, self.cursor, self.holds_anything());
    }

    /// The account is gone: what the books held is a loss through suspense,
    /// and the cursor, on a chain gone, is dropped. Entry payments a state
    /// written while the schemes peered held went with the account.
    pub(crate) fn written_off(&mut self, effects: &mut Vec<Effect>) {
        if self.booked > 0 {
            effects.push(Effect::Reconcile {
                amount: self.booked,
                gain: false,
                cause: ReconcileCause::Unexplained,
            });
            self.booked = 0;
        }
        self.entries = 0;
        self.cursor = None;
    }

    /// Makes the books hold `target`, through suspense, for `cause`.
    pub(crate) fn reconcile_to(
        &mut self,
        target: u64,
        cause: ReconcileCause,
        effects: &mut Vec<Effect>,
    ) {
        if target != self.booked {
            effects.push(Effect::Reconcile {
                amount: target.abs_diff(self.booked),
                gain: target > self.booked,
                cause,
            });
            self.booked = target;
        }
    }

    /// Makes the books hold `booked`, at most a total. Below zero, they
    /// hold nothing, and what they are short comes from suspense, so that a
    /// reconcile after starts from what the books hold.
    pub(crate) fn hold(&mut self, booked: i128, effects: &mut Vec<Effect>) {
        self.booked = narrow(booked.clamp(0, i128::from(MAX_TOTAL)));
        if booked < 0 {
            effects.push(Effect::Reconcile {
                amount: narrow(-booked),
                gain: true,
                cause: ReconcileCause::Unexplained,
            });
        }
    }
}

/// Settlements the node still holds pending after a restart, signed before
/// the XRPL schemes stopped peering: none is booked as a settlement any
/// more, so each is cancelled. What the ledger did with one is booked as
/// any transaction of the account's is, by what it moved: the owner's
/// withdrawal, if it applied.
pub(crate) fn cancel_pending(pending: &[(Name, u64)], effects: &mut Vec<Effect>) {
    for (settlement, _) in pending {
        effects.push(Effect::Cancel {
            settlement: *settlement,
        });
    }
}

/// The owner's payments.
impl Follower {
    /// Whether `hash` is the owner's payment on its way.
    pub(crate) fn is_owner_payment(&self, hash: &[u8; 32]) -> bool {
        self.owner_payment.is_some_and(|(owner, _)| owner == *hash)
    }

    /// Whether `hash` is the owner's last payment no longer on its way.
    pub(crate) fn was_owner_payment(&self, hash: &[u8; 32]) -> bool {
        self.owner_outcome.is_some_and(|(owner, _)| owner == *hash)
    }

    /// The owner's payment on its way is no longer: what became of it,
    /// which posts the node's hold on the owner's funds if it applied, and
    /// voids it otherwise. One whose outcome the scheme never read is in
    /// the holding it synced to.
    pub(crate) fn owner_done(&mut self, outcome: OwnerPaymentState, effects: &mut Vec<Effect>) {
        if let Some((hash, _)) = self.owner_payment.take() {
            self.owner_outcome = Some((hash, outcome));
            effects.push(Effect::OwnerPaid {
                id: hash_name(&hash),
                applied: outcome == OwnerPaymentState::Applied,
            });
        }
    }

    /// What became of the owner's payment `id`.
    pub(crate) fn owner_state(&self, id: Name) -> OwnerPaymentState {
        let Some(hash) = name_hash(id) else {
            return OwnerPaymentState::Unknown;
        };
        if self.is_owner_payment(&hash) {
            return OwnerPaymentState::InFlight;
        }
        match self.owner_outcome {
            Some((done, outcome)) if done == hash => outcome,
            _ => OwnerPaymentState::Unknown,
        }
    }

    /// Where `payment` goes and the memo it carries, if the scheme may sign
    /// one now: one at a time, never to the account itself, with a memo of
    /// the type its reader expects, never a settlement's ([`owner_memos`]).
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] while one is on its way;
    /// [`SchemeError::InvalidData`] for one it never signs.
    pub(crate) fn owner_request(
        &self,
        payment: &OwnerPayment,
    ) -> Result<(AccountId, Vec<Memo>), SchemeError> {
        if self.owner_payment.is_some() {
            return Err(SchemeError::Unavailable);
        }
        let destination = AccountId::parse(&payment.destination).ok_or(SchemeError::InvalidData)?;
        let memos = owner_memos(payment)?;
        if destination == self.account {
            return Err(SchemeError::InvalidData);
        }
        Ok((destination, memos))
    }

    /// The owner's payment `hash`, of `units` of the holding, that
    /// `scheme` just signed on `account`: executed at once, its submission
    /// going out after the batch that keeps it with the account's and the
    /// scheme's state (spec/areas/xrpl.md#deposits-and-withdrawals).
    pub(crate) fn owner_signed(
        &mut self,
        account: &mut XrplAccount,
        scheme: Name,
        hash: [u8; 32],
        units: u64,
    ) -> Name {
        assert!(
            (1..=self.max_owner_units).contains(&units),
            "an owner's payment pays what the books can hold"
        );
        let executed = account.execute(scheme, hash);
        assert!(executed, "a payment just signed is in flight");
        self.owner_payment = Some((hash, units));
        hash_name(&hash)
    }
}

/// What is kept with the books.
impl Follower {
    /// A number that changes whenever what [`Self::write`] keeps does,
    /// cheaply, mixed with `extra`, a number of what the scheme keeps
    /// beside it: the node writes the state only then.
    pub(crate) fn fingerprint(&self, extra: u64) -> u64 {
        let mut mix: u64 = 0xcbf2_9ce4_8422_2325;
        let mut add = |value: u64| {
            mix ^= value;
            mix = mix.wrapping_mul(0x0100_0000_01b3);
        };
        let (ledger, index) = self.cursor.unwrap_or((u32::MAX, u32::MAX));
        add(u64::from(ledger) << 32 | u64::from(index));
        add(self.write_offs);
        add(extra);
        if let Some((hash, units)) = self.owner_payment {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]));
            add(units);
        }
        if let Some((hash, outcome)) = self.owner_outcome {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]));
            add(u64::from(outcome_code(outcome)));
        }
        add(self.entries);
        mix
    }

    /// Writes what it keeps, in this order: the cursor, the count of
    /// write-offs, the owner's payment on its way and what became of the
    /// last (spec/areas/xrpl.md#state-and-restarts). The scheme writes the
    /// version before it, [`SCHEME_VERSION`], and what it keeps of its own
    /// after it.
    pub(crate) fn write(&self, out: &mut Out) {
        out.option(self.cursor, |out, (ledger, index)| {
            out.u32(ledger);
            out.u32(index);
        });
        out.u64(self.write_offs);
        out.option(self.owner_payment, |out, (hash, units)| {
            out.bytes(&hash);
            out.u64(units);
        });
        out.option(self.owner_outcome, |out, (hash, outcome)| {
            out.bytes(&hash);
            out.u8(outcome_code(outcome));
        });
    }

    /// Reads back what [`Self::write`] wrote, in a state of `version`:
    /// [`SCHEME_VERSION`], or [`PEERING_SCHEME_VERSION`], written while the
    /// schemes peered, which held more after the count of write-offs
    /// ([`read_peering`]): the entry payments in it are booked as the
    /// owner's deposit ([`Self::book_entries`]), and the rest is dropped.
    /// `None` for a state no follower writes: more than it keeps, entry
    /// payments before the first sync, an owner's payment of nothing or of
    /// more than the books hold, or an outcome it never writes.
    pub(crate) fn read(&mut self, input: &mut In<'_>, version: u8) -> Option<()> {
        let peering = match version {
            SCHEME_VERSION => false,
            PEERING_SCHEME_VERSION => true,
            _ => return None,
        };
        self.cursor = input.option(|input| Some((input.u32()?, input.u32()?)))?;
        self.write_offs = input.u64()?;
        if peering {
            self.entries = read_peering(input)?;
            if self.cursor.is_none() && self.entries > 0 {
                return None;
            }
        }
        self.owner_payment = input.option(|input| {
            let hash = input.array::<32>()?;
            let units = input.u64()?;
            (1..=self.max_owner_units)
                .contains(&units)
                .then_some((hash, units))
        })?;
        self.owner_outcome =
            input.option(|input| Some((input.array::<32>()?, outcome_of(input.u8()?)?)))?;
        Some(())
    }

    /// Asserts what holds after every change.
    pub(crate) fn check_invariants(&self) {
        assert!(
            self.owner_payment
                .is_none_or(|(_, units)| (1..=self.max_owner_units).contains(&units)),
            "the owner's payment pays whole units the books can hold"
        );
        assert!(
            self.owner_outcome
                .is_none_or(|(_, outcome)| outcome != OwnerPaymentState::InFlight),
            "the last owner's payment done is no longer on its way"
        );
        assert!(self.entries <= MAX_TOTAL, "the books could take them");
        assert!(
            self.cursor.is_some() || self.entries == 0,
            "entry payments are read past the first sync"
        );
        assert!(self.booked <= MAX_TOTAL, "the books hold at most a total");
    }
}

/// Reads what a state written while the schemes peered held between the
/// count of write-offs and the owner's payment: the peers' addresses, each
/// a name and an account; the settlements looked up after a restart, each
/// a hash and whether it was unread; and the entry payments held, each its
/// node, the account it came from, its amount, its hash and the ledger its
/// hold ran to. Returns what the entry payments add up to, in internal
/// units; `None` for more than such a state held, an entry payment of
/// nothing or twice, or more than the books could take.
fn read_peering(input: &mut In<'_>) -> Option<u64> {
    // A name is at least two bytes, and an account twenty.
    let peers = input.count(22, PEERING_MAX_PEERS)?;
    for _ in 0..peers {
        input.name()?;
        input.account()?;
    }
    let recovering = input.count(33, PEERING_MAX_RECOVERING)?;
    for _ in 0..recovering {
        input.array::<32>()?;
        input.bool()?;
    }
    // A name is at least two bytes, the rest 64.
    let held = input.count(66, PEERING_MAX_ENTRIES)?;
    let mut hashes = Vec::with_capacity(held);
    let mut total: u64 = 0;
    for _ in 0..held {
        input.name()?;
        input.account()?;
        let amount = input.u64().filter(|amount| *amount > 0)?;
        let hash = input.array::<32>()?;
        input.u32()?;
        if hashes.contains(&hash) {
            return None;
        }
        hashes.push(hash);
        total = total.checked_add(amount).filter(|total| *total <= MAX_TOTAL)?;
    }
    Some(total)
}

/// The XRP Ledger's result of a payment to an account that does not exist,
/// of less than the reserve that would create it.
const TOO_LITTLE_RESULT: &str = "tecNO_DST_INSUF_XRP";

/// Why the owner's payment `record`, which failed, failed.
pub(crate) fn failure_of(record: &TxRecord) -> PaymentFailure {
    if record.result == TOO_LITTLE_RESULT {
        PaymentFailure::TooLittle
    } else {
        PaymentFailure::Other
    }
}

/// How a state writes what became of an owner's payment.
const fn outcome_code(outcome: OwnerPaymentState) -> u8 {
    match outcome {
        OwnerPaymentState::Unknown | OwnerPaymentState::InFlight => 0,
        OwnerPaymentState::Applied => 1,
        OwnerPaymentState::Failed(PaymentFailure::Other) => 2,
        OwnerPaymentState::Failed(PaymentFailure::TooLittle) => 3,
        OwnerPaymentState::Cancelled => 4,
    }
}

/// What became of an owner's payment, as a state writes it.
const fn outcome_of(code: u8) -> Option<OwnerPaymentState> {
    Some(match code {
        0 => OwnerPaymentState::Unknown,
        1 => OwnerPaymentState::Applied,
        2 => OwnerPaymentState::Failed(PaymentFailure::Other),
        3 => OwnerPaymentState::Failed(PaymentFailure::TooLittle),
        4 => OwnerPaymentState::Cancelled,
        _ => return None,
    })
}

/// What the books hold, or a difference from it. Every amount booked is
/// within [`MAX_TOTAL`], and so is what the books held before it.
pub(crate) fn narrow(booked: i128) -> u64 {
    let narrow = u64::try_from(booked);
    assert!(narrow.is_ok(), "booked past what a u64 holds");
    narrow.unwrap_or_default()
}

/// A transaction hash as a settlement ID: uppercase hex, 64 characters.
pub(crate) fn hash_name(hash: &[u8; 32]) -> Name {
    Name::hex(hash, true)
}

/// A settlement ID as the transaction hash it is, if it is one.
pub(crate) fn name_hash(settlement: Name) -> Option<[u8; 32]> {
    from_hex(settlement.as_str())?.try_into().ok()
}

