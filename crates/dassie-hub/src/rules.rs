//! A hub's founding document (spec/designs/settlement-hubs.md#2-founding-a-hub):
//! the terms the sponsor fixes, and the acceptances of its validators. The
//! hub ID is the digest of the rules.
//!
//! Values in the terms are starting points for the simulation to set; what
//! the code fixes here is which values exist, how they bound each other
//! ([`Terms::check`]), and their encoding, which every hub ID depends on.
//!
//! A hub holds one asset on the XRP Ledger: XRP, or a token of an issuer,
//! such as DSUSD or RLUSD ([`Terms::asset`]). The encoding is versioned so
//! that the terms of an XRP hub encode as they always have, and every hub
//! founded before tokens keeps its ID: a token hub's terms start with a
//! mark no XRP hub's nonce may start with ([`Terms::check`]), then the
//! version and the asset, then what an XRP hub's terms hold.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey, Signature};
use dassie_oer::{Alphabet, Encode, Reader, Size, Writer, size};

use crate::codec::{
    HubDecodeError, KEY_LEN, SIGNATURE_LEN, read_key, read_signature, write_key, write_signature,
};
use crate::consensus::{MAX_STALL_MS, MIN_STALL_MS};
use crate::digest::{Digest, Domain};
use crate::validators::{MAX_VALIDATORS, MIN_VALIDATORS, ValidatorSet};

pub use dassie_xrpl::amount::Asset;
use dassie_xrpl::amount::{Currency, MAX_MANTISSA, TokenAmount, TokenValue};
use dassie_xrpl::keys::AccountId;

/// An amount of the hub's asset, in its smallest unit: drops of XRP, or
/// millionths of a token ([`TOKEN_DECIMALS`]).
pub type Amount = u64;

/// The decimals of a token hub's unit: a millionth of the token, the unit
/// the node's books count a dollar token in
/// ([`UNITS_PER_MICRO`](dassie_xrpl::token::UNITS_PER_MICRO)), as a drop
/// is an XRP hub's.
pub const TOKEN_DECIMALS: u8 = 6;

/// The least XRP, in drops, a token hub's escrow holds before a member
/// asks for a withdrawal: the gas its transactions burn. The escrow's own
/// XRP pays every fee of a token hub, never the token, and anyone may send
/// it XRP, which is never a deposit; a member that finds the escrow's XRP
/// balance below this is to top it up first (not built yet: this is what
/// it will check against). The XRP Ledger lets an account
/// burn its reserve for fees, so the whole balance counts, never what is
/// above the reserve, and no arithmetic of reserves is needed. A token
/// hub's gas ([`Fees::gas`]) is at most this, so the escrow can always pay
/// the next transaction once it holds it.
pub const LEAST_ESCROW_GAS_DROPS: u64 = 1_000_000;

/// The XRP Ledger's base fee in drops: what a transaction pays at least,
/// and a multi-signed one once more for each signature.
pub const LEDGER_BASE_FEE: Amount = 10;

/// The least gas a token hub names: the base fee once for each signature
/// of the largest set of validators there may be, and once more. New rules
/// may add validators but never change the gas ([`Terms::keeps_tables_of`]),
/// so it is sized for any set the hub may come to have; a gas sized for
/// fewer would name a fee the ledger refuses once the set grows, and stop
/// the escrow's sequence for good.
pub const LEAST_TOKEN_GAS_DROPS: Amount = LEDGER_BASE_FEE * (MAX_VALIDATORS as u64 + 1);

/// A time in milliseconds since the Unix epoch, or a duration in them.
pub type Millis = u64;

/// A hub, named by the digest of its rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HubId(pub Digest);

/// Size of an XRPL public key as the escrow's signer list names it: a
/// prefix byte (`0xED` for Ed25519) and 32 bytes, or a compressed
/// secp256k1 point.
pub const LEDGER_KEY_LEN: usize = 33;

/// Size of an XRPL account ID.
pub const LEDGER_ACCOUNT_LEN: usize = 20;

/// The most bytes of a validator's endpoint, a host and port.
pub const MAX_ENDPOINT_LEN: usize = 255;

/// What an endpoint may contain: a DNS name or an address, and a port.
const ENDPOINT_SIZE: Size = Size::range(1, MAX_ENDPOINT_LEN);
const ENDPOINT_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'A', b'Z')
    .with_range(b'0', b'9')
    .with_chars(b".-:[]");

/// The fees of a hub, in the hub's asset but for the gas, which is XRP
/// (spec/designs/settlement-hubs.md#7-fees).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fees {
    /// What each operation pays.
    pub operation: Amount,
    /// In an XRP hub, the underlying ledger's fee for one transaction the
    /// escrow sends, multi-signed by a quorum: at least the ledger's base
    /// fee once per signer and once more. A withdrawal pays it on top of
    /// the operation fee, and so does each payment of a payout set. Zero in
    /// a token hub, whose escrow pays its fees in XRP ([`Fees::gas`]).
    pub ledger: Amount,
    /// In a token hub, the fee in drops of XRP that each transaction the
    /// escrow sends names, multi-signed by a quorum, as `ledger` is in an
    /// XRP hub: paid from the escrow's own XRP, never charged to an
    /// account or burnt from the token the hub holds. It never changes, so
    /// it is at least the fee of the largest set of validators the hub may
    /// come to have ([`LEAST_TOKEN_GAS_DROPS`]). Zero in an XRP hub.
    pub gas: Amount,
    /// Parts per million of each fee that go to the reserve account.
    pub reserve_share_ppm: u32,
    /// What each account locks while it exists.
    pub account_reserve: Amount,
    /// What each hold locks from its payer while it exists.
    pub hold_reserve: Amount,
    /// What a new account starts with beyond its reserve, for the fees of
    /// its first operations.
    pub starting_balance: Amount,
    /// The least a withdrawal or a payout pays on the underlying ledger.
    pub least_payout: Amount,
}

/// The bounds of a hub's tables (spec/designs/settlement-hubs.md#3-the-ledger).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Accounts, members and validators together.
    pub accounts: u32,
    /// Holds.
    pub holds: u32,
    /// Holds one account pays into or takes from.
    pub holds_per_account: u16,
    /// Withdrawals not yet settled on the underlying ledger.
    pub withdrawals: u32,
    /// Deposits fewer than a quorum have attested.
    pub pending_deposits: u32,
    /// Deposits credited that the hub still remembers, against crediting
    /// one twice.
    pub recent_deposits: u32,
    /// Operations in a block.
    pub block_operations: u16,
    /// Of those, the places kept for `redeem` and `claim`.
    pub deadline_operations: u16,
    /// Sequences of one hold claimed with fulfillments and not yet passed
    /// by a redeem.
    pub claimed_per_hold: u16,
    /// Commitments in one `claim`.
    pub commitments_per_claim: u16,
    /// K: the blocks back for which a block records votes.
    pub vote_window: u16,
}

/// The durations of a hub (spec/designs/settlement-hubs.md#3-the-ledger).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timers {
    /// How long a reduction of a hold waits before it takes effect.
    pub hold_notice: Millis,
    /// How long payees may still redeem and claim after `shutdown`.
    pub shutdown_notice: Millis,
    /// How long `reconfigure` waits before it takes effect.
    pub reconfigure_notice: Millis,
    /// How long a validator's votes may go unrecorded before the hub shuts
    /// down.
    pub absence: Millis,
    /// How long a validator sees no block become final before it declares
    /// the hub stalled.
    pub stall: Millis,
    /// M: how much earlier than its Prepare a commitment expires. At least
    /// what finality takes with f validators faulty, as consensus's timings
    /// give it ([`least_margin_ms`]); the precision and the views' timeouts
    /// are consensus's, derived from the region, never the rules'
    /// (`consensus/config.rs`).
    pub margin: Millis,
    /// The legs between members a peering over the hub allows for.
    pub allowance: Millis,
}

/// Everything in the rules but the validators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terms {
    /// A random nonce the sponsor picked, under which validators derive
    /// their keys for this hub.
    pub nonce: [u8; 32],
    /// The sponsor's node key.
    pub sponsor: PublicKey,
    /// The underlying ledger's network ID (1 is the XRPL testnet).
    pub network: u32,
    /// The escrow's account on the underlying ledger, known in advance:
    /// its master key is derived from the sponsor's seed and the nonce.
    pub escrow: [u8; LEDGER_ACCOUNT_LEN],
    /// What the hub holds: XRP, or a token of an issuer, on the escrow's
    /// trust line to it.
    pub asset: Asset,
    /// The hub's fees.
    pub fees: Fees,
    /// The bounds of its tables.
    pub limits: Limits,
    /// Its durations.
    pub timers: Timers,
}

/// A validator's acceptance of the terms, signed with its node key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Acceptance {
    /// The validator's node key, which signs the acceptance.
    pub node: PublicKey,
    /// The key it signs votes, certificates and receipts with.
    pub consensus: PublicKey,
    /// Its key on the escrow's signer list.
    pub escrow: [u8; LEDGER_KEY_LEN],
    /// The secp256k1 public key from which its payout keys derive.
    pub payout: [u8; LEDGER_KEY_LEN],
    /// The BIP 32 chain code of its payout key.
    pub payout_chain: [u8; 32],
    /// Where members and validators reach it.
    pub endpoint: String,
    /// Its node key's signature over [`Acceptance::digest`].
    pub signature: Signature,
}

/// A hub's rules: the terms and one acceptance per validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// The terms.
    pub terms: Terms,
    /// The validators, in the order of the set.
    pub validators: Vec<Acceptance>,
}

/// Why rules or terms are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulesError {
    /// A value or a relation between values is out of bounds.
    Terms(&'static str),
    /// The validators are too few, too many or not distinct.
    Validators,
    /// An acceptance's signature does not verify.
    Acceptance(usize),
}

impl Terms {
    /// The digest an acceptance names.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::of(Domain::Terms, &[&self.to_vec()])
    }

    /// Checks the relations the hub's safety rests on: values that would
    /// let the ledger reach a state it cannot leave are refused here, never
    /// found in a block.
    pub fn check(&self) -> Result<(), RulesError> {
        self.check_fees()?;
        self.check_limits()?;
        self.check_timers()
    }

    fn check_fees(&self) -> Result<(), RulesError> {
        let fees = &self.fees;
        let fail = |why| Err(RulesError::Terms(why));
        if fees.reserve_share_ppm > 1_000_000 {
            return fail("reserve share above one");
        }
        // Free operations, or holds that lock nothing, would cost nothing
        // to fill the tables with.
        if fees.operation == 0 || fees.hold_reserve == 0 {
            return fail("an operation or a hold costs nothing");
        }
        self.check_asset()?;
        // Every account the hub keeps can be paid out at shutdown
        // (spec/designs/settlement-hubs.md#7-fees).
        if fees.account_reserve < fees.least_payout.saturating_add(fees.ledger) {
            return fail("account reserve below least payout and its fee");
        }
        if fees.starting_balance < fees.operation.saturating_mul(4) {
            return fail("starting balance too small for a new account's first operations");
        }
        Ok(())
    }

    /// The asset, and the fees that depend on it.
    fn check_asset(&self) -> Result<(), RulesError> {
        let fees = &self.fees;
        let fail = |why| Err(RulesError::Terms(why));
        match self.asset {
            Asset::Xrp => {
                // Its terms encode as they did before tokens, and that
                // encoding must not read as a later version's.
                if self.nonce.starts_with(&VERSION_MARK) {
                    return fail("nonce starts as a later version's terms");
                }
                if fees.gas != 0 {
                    return fail("gas in an XRP hub, whose ledger fee pays its transactions");
                }
                // A payout to a new account on the XRP Ledger must create
                // it (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
                if fees.least_payout < LEDGER_BASE_RESERVE {
                    return fail("least payout below the underlying ledger's base reserve");
                }
            }
            Asset::Token { currency, issuer } => {
                if !token_valid(currency, issuer) || issuer.0 == self.escrow {
                    return fail("a token hub's asset is not a token another account issues");
                }
                // The escrow's XRP pays every fee of a token hub: the hub
                // charges none in the token, and never burns it.
                if fees.ledger != 0 {
                    return fail("a ledger fee in a token hub, whose escrow pays fees in XRP");
                }
                if fees.gas < LEAST_TOKEN_GAS_DROPS || fees.gas > LEAST_ESCROW_GAS_DROPS {
                    return fail(
                        "gas below the largest set's fee, or above the least XRP the escrow keeps",
                    );
                }
                // A Payment of nothing is malformed: it would never use its
                // sequence, and the escrow's would stop.
                if fees.least_payout == 0 {
                    return fail("least payout zero");
                }
            }
        }
        Ok(())
    }

    fn check_limits(&self) -> Result<(), RulesError> {
        let limits = &self.limits;
        let fail = |why| Err(RulesError::Terms(why));
        // Room for deadlines, and for everything else.
        if limits.deadline_operations == 0 || limits.deadline_operations >= limits.block_operations
        {
            return fail("no room for deadlines, or none for other operations");
        }
        // A payer may leave this many claims missing before its payee stops
        // taking its Prepares (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
        if limits.claimed_per_hold < 1024 || limits.commitments_per_claim == 0 {
            return fail("claimed table smaller than the missing claims allowed");
        }
        if usize::from(limits.vote_window) < MAX_VALIDATORS {
            return fail("vote window shorter than any validator's turn");
        }
        // What a follower decodes before it applies the rules: a block a
        // leader may build under them must decode.
        let decodable = usize::from(limits.block_operations)
            <= crate::service::payload::MAX_OPERATIONS
            && usize::from(limits.vote_window) <= crate::service::payload::MAX_VOTE_HEIGHTS
            && usize::from(limits.commitments_per_claim) <= crate::operation::MAX_COMMITMENTS;
        if !decodable {
            return fail("a block or claim the rules allow could not be decoded");
        }
        let empty = limits.accounts == 0
            || limits.holds == 0
            || limits.holds_per_account == 0
            || limits.withdrawals == 0
            || limits.recent_deposits == 0
            || usize::try_from(limits.pending_deposits).unwrap_or(usize::MAX) < MAX_VALIDATORS;
        if empty {
            return fail("a table without room, or pending deposits without one per validator");
        }
        Ok(())
    }

    fn check_timers(&self) -> Result<(), RulesError> {
        let timers = &self.timers;
        let fail = |why| Err(RulesError::Terms(why));
        // A hold cannot shrink before any packet on it expires.
        if timers.hold_notice <= MAX_HOLD_MS || timers.margin == 0 {
            return fail("hold notice not longer than a packet may be held");
        }
        // A member that does not accept new rules can take its holds back
        // and leave before they apply (spec/designs/settlement-hubs.md#8-changing-the-validators).
        if timers.reconfigure_notice <= timers.hold_notice {
            return fail("reconfiguration notice not longer than a hold's");
        }
        if timers.absence <= timers.stall {
            return fail("absence not longer than a stall");
        }
        // A hub recovering from the longest outage must not stall first,
        // and an outage as long as the stall must fit in the views an
        // engine holds (consensus/config.rs).
        if !(MIN_STALL_MS..=MAX_STALL_MS).contains(&timers.stall) {
            return fail("stall not between a recovery and the views an engine holds");
        }
        Ok(())
    }

    /// Whether these terms keep every table at least as large as
    /// `current`'s, and keep its asset and gas. New rules may not shrink a
    /// table: what it holds could no longer fit. Nor may they change the
    /// asset, which every amount in the tables counts, or the gas, which a
    /// withdrawal already given a sequence names: validators that signed it
    /// before the change and after would sign two transactions, and its
    /// sequence could wait for ever.
    #[must_use]
    pub fn keeps_tables_of(&self, current: &Self) -> bool {
        let (new, old) = (&self.limits, &current.limits);
        self.asset == current.asset
            && self.fees.gas == current.fees.gas
            && new.accounts >= old.accounts
            && new.holds >= old.holds
            && new.holds_per_account >= old.holds_per_account
            && new.withdrawals >= old.withdrawals
            && new.pending_deposits >= old.pending_deposits
            && new.recent_deposits >= old.recent_deposits
            && new.claimed_per_hold >= old.claimed_per_hold
    }

    /// The fee in drops each transaction the escrow sends names: the
    /// ledger fee in an XRP hub, the gas in a token hub.
    #[must_use]
    pub const fn transaction_fee(&self) -> Amount {
        match self.asset {
            Asset::Xrp => self.fees.ledger,
            Asset::Token { .. } => self.fees.gas,
        }
    }

    /// The node's internal units in one of the hub's: a drop, or a
    /// millionth of a token.
    #[must_use]
    pub const fn units_per_amount(&self) -> u64 {
        match self.asset {
            Asset::Xrp => dassie_xrpl::UNITS_PER_DROP,
            Asset::Token { .. } => dassie_xrpl::token::UNITS_PER_MICRO,
        }
    }

    /// `amount` of the hub's asset as a Payment on the XRP Ledger pays it:
    /// drops, or the token. An amount of a token beyond the ledger's
    /// sixteen digits, ten billion tokens and more, is rounded down to
    /// them: the escrow keeps what is left, and still holds at least what
    /// the hub owes ([`token_value`]).
    #[must_use]
    pub fn ledger_amount(&self, amount: Amount) -> dassie_xrpl::codec::Amount {
        match self.asset {
            Asset::Xrp => dassie_xrpl::codec::Amount::Xrp(amount),
            Asset::Token { currency, issuer } => dassie_xrpl::codec::Amount::Token(TokenAmount {
                value: token_value(amount),
                currency,
                issuer,
            }),
        }
    }

    /// What `delivered` adds to the hub, in its unit, if it is of the
    /// hub's asset: drops, or the whole millionths of the token, rounded
    /// down. What is below a millionth stays in the escrow, beyond what
    /// the hub owes. `None` for another asset, or more than an amount
    /// holds.
    #[must_use]
    pub fn hub_amount(&self, delivered: &dassie_xrpl::codec::Amount) -> Option<Amount> {
        match (self.asset, delivered) {
            (Asset::Xrp, dassie_xrpl::codec::Amount::Xrp(drops)) => Some(*drops),
            (Asset::Token { currency, issuer }, dassie_xrpl::codec::Amount::Token(token))
                if token.currency == currency && token.issuer == issuer =>
            {
                let units = token.value.floor_units(TOKEN_DECIMALS)?;
                Amount::try_from(units).ok()
            }
            _ => None,
        }
    }

    /// What a new account is paid at least: its reserve and its starting
    /// balance (spec/designs/settlement-hubs.md#5-joining).
    #[must_use]
    pub const fn new_account_minimum(&self) -> Amount {
        self.fees
            .account_reserve
            .saturating_add(self.fees.starting_balance)
    }
}

/// The least margin M of a hub with `validators` validators: the precision,
/// and what it takes to finalize a block when the next f leaders are faulty
/// (`Config::margin_ms`), at consensus's timings, which every validator
/// runs.
#[must_use]
pub fn least_margin_ms(validators: usize) -> Millis {
    let faults = u64::try_from(validators.saturating_sub(1) / 3).unwrap_or(u64::MAX);
    crate::consensus::Config::DEFAULT.margin_ms(faults)
}

/// The longest a connector holds a packet (spec/decisions/connector-timing.md),
/// which a hold's notice must exceed.
pub const MAX_HOLD_MS: Millis = 30_000;

/// The XRP Ledger's base reserve, in drops: the least a payment that
/// creates an account delivers.
pub const LEDGER_BASE_RESERVE: Amount = 1_000_000;

/// Whether a token can be of `currency` and `issuer`, as the XRP Ledger
/// takes one.
fn token_valid(currency: Currency, issuer: AccountId) -> bool {
    TokenAmount {
        value: TokenValue::ZERO,
        currency,
        issuer,
    }
    .is_valid()
}

/// `amount` millionths of a token as the XRP Ledger's value: exactly, up
/// to sixteen digits, the most a value holds; beyond them, ten billion
/// tokens and more, rounded down to sixteen digits.
#[must_use]
pub fn token_value(amount: Amount) -> TokenValue {
    let (mut mantissa, mut exponent) = (amount, -i32::from(TOKEN_DECIMALS));
    // An amount has at most twenty digits: four steps at most.
    for _ in 0..4 {
        if mantissa <= MAX_MANTISSA {
            break;
        }
        mantissa /= 10;
        exponent = exponent.saturating_add(1);
    }
    let value = TokenValue::new(false, mantissa, exponent);
    assert!(value.is_some(), "sixteen digits of millionths are a value");
    value.unwrap_or(TokenValue::ZERO)
}

impl Acceptance {
    /// The digest the validator's node key signs: the nonce, the hash of
    /// the terms, its keys and its endpoint.
    #[must_use]
    pub fn digest(&self, terms: &Digest, nonce: &[u8; 32]) -> Digest {
        Digest::of(
            Domain::Acceptance,
            &[
                nonce,
                terms.as_bytes(),
                self.node.as_bytes(),
                self.consensus.as_bytes(),
                &self.escrow,
                &self.payout,
                &self.payout_chain,
                self.endpoint.as_bytes(),
            ],
        )
    }

    /// Signs an acceptance with `node`, the validator's node key.
    #[must_use]
    pub fn sign(mut self, node: &NodeKey, terms: &Terms) -> Self {
        self.node = node.public_key();
        let digest = self.digest(&terms.digest(), &terms.nonce);
        self.signature = node.sign(digest.as_bytes());
        self
    }

    /// Whether the signature verifies.
    #[must_use]
    pub fn verifies(&self, terms: &Digest, nonce: &[u8; 32]) -> bool {
        let digest = self.digest(terms, nonce);
        self.node.verify(digest.as_bytes(), &self.signature).is_ok()
    }
}

impl Rules {
    /// The hub ID.
    #[must_use]
    pub fn id(&self) -> HubId {
        HubId(Digest::of(Domain::Rules, &[&self.to_vec()]))
    }

    /// The validators' consensus keys as a set.
    #[must_use]
    pub fn validator_set(&self) -> Option<ValidatorSet> {
        ValidatorSet::new(self.validators.iter().map(|a| a.consensus).collect())
    }

    /// Checks the terms, the set and every acceptance, and that the margin
    /// covers what finality takes with as many validators faulty as the set
    /// tolerates: with less, a block that settles a packet need not be
    /// final before the packet expires, and the payer's guarantee is void
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). Members
    /// check it too: a hub whose rules do not check is joined by no one.
    pub fn check(&self) -> Result<(), RulesError> {
        self.terms.check()?;
        if self.validator_set().is_none() {
            return Err(RulesError::Validators);
        }
        if self.terms.timers.margin < least_margin_ms(self.validators.len()) {
            return Err(RulesError::Terms(
                "margin below what finality takes with f validators faulty",
            ));
        }
        let nodes_distinct = self.validators.iter().enumerate().all(|(i, a)| {
            self.validators
                .iter()
                .skip(i.saturating_add(1))
                .all(|b| b.node != a.node && b.escrow != a.escrow && b.payout != a.payout)
        });
        if !nodes_distinct {
            return Err(RulesError::Validators);
        }
        let terms = self.terms.digest();
        for (i, acceptance) in self.validators.iter().enumerate() {
            let usable = acceptance.keys_usable(&self.terms.escrow);
            if !usable || !acceptance.verifies(&terms, &self.terms.nonce) {
                return Err(RulesError::Acceptance(i));
            }
        }
        Ok(())
    }
}

impl Acceptance {
    /// Whether its keys can do their jobs: an escrow key that is a key of
    /// the XRP Ledger and not the escrow's own (a signer list cannot name
    /// its account), and a payout key that is a secp256k1 point. A
    /// validator whose keys could not would leave a change of signers that
    /// never applies, or a payout set that cannot be built, and freeze the
    /// escrow (spec/designs/settlement-hubs.md#9-shutdown).
    #[must_use]
    pub fn keys_usable(&self, escrow: &[u8; LEDGER_ACCOUNT_LEN]) -> bool {
        let escrow_key = dassie_xrpl::keys::PublicKey::from_bytes(self.escrow).filter(|key| {
            match key.key_type() {
                dassie_xrpl::keys::KeyType::Ed25519 => {
                    let mut point = [0; 32];
                    point.copy_from_slice(self.escrow.get(1..).unwrap_or(&[0; 32]));
                    PublicKey::from_bytes(&point).is_ok_and(|key| !key.is_weak())
                }
                dassie_xrpl::keys::KeyType::Secp256k1 => {
                    k256::PublicKey::from_sec1_bytes(&self.escrow).is_ok()
                }
            }
        });
        let payout = matches!(self.payout.first(), Some(0x02 | 0x03))
            && k256::PublicKey::from_sec1_bytes(&self.payout).is_ok();
        escrow_key.is_some_and(|key| key.account().0 != *escrow) && payout
    }
}

const FEES_LEN: usize = 8 * 6 + 4;
const LIMITS_LEN: usize = 4 * 5 + 2 * 6;
const TIMERS_LEN: usize = 8 * 7;
/// The terms of an XRP hub, as they were before tokens.
const TERMS_LEN: usize = 32 + KEY_LEN + 4 + LEDGER_ACCOUNT_LEN + FEES_LEN + LIMITS_LEN + TIMERS_LEN;

/// What starts the terms of every version after the first. The first
/// version, an XRP hub's, starts with its nonce, which [`Terms::check`]
/// refuses to start so.
const VERSION_MARK: [u8; 7] = [0xff, b'D', b'a', b's', b's', b'i', b'e'];
/// The version of a token hub's terms: after the mark, the version, the
/// token's currency and issuer, and the gas; then an XRP hub's terms.
const TOKEN_VERSION: u8 = 1;
/// What a token hub's terms hold before an XRP hub's.
const TOKEN_HEADER_LEN: usize = VERSION_MARK.len() + 1 + 20 + LEDGER_ACCOUNT_LEN + 8;

impl Encode for Terms {
    fn encoded_len(&self) -> usize {
        match self.asset {
            Asset::Xrp => TERMS_LEN,
            Asset::Token { .. } => TOKEN_HEADER_LEN + TERMS_LEN,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        if let Asset::Token { currency, issuer } = self.asset {
            writer.write_bytes(&VERSION_MARK);
            writer.write_u8(TOKEN_VERSION);
            writer.write_bytes(&currency.0);
            writer.write_bytes(&issuer.0);
            writer.write_u64(self.fees.gas);
        }
        writer.write_bytes(&self.nonce);
        write_key(writer, &self.sponsor);
        writer.write_u32(self.network);
        writer.write_bytes(&self.escrow);
        let fees = &self.fees;
        for value in [fees.operation, fees.ledger] {
            writer.write_u64(value);
        }
        writer.write_u32(fees.reserve_share_ppm);
        for value in [
            fees.account_reserve,
            fees.hold_reserve,
            fees.starting_balance,
            fees.least_payout,
        ] {
            writer.write_u64(value);
        }
        encode_limits(writer, &self.limits);
        encode_timers(writer, &self.timers);
    }
}

fn encode_limits(writer: &mut Writer<'_>, limits: &Limits) {
    writer.write_u32(limits.accounts);
    writer.write_u32(limits.holds);
    writer.write_u16(limits.holds_per_account);
    writer.write_u32(limits.withdrawals);
    writer.write_u32(limits.pending_deposits);
    writer.write_u32(limits.recent_deposits);
    for value in [
        limits.block_operations,
        limits.deadline_operations,
        limits.claimed_per_hold,
        limits.commitments_per_claim,
        limits.vote_window,
    ] {
        writer.write_u16(value);
    }
}

fn encode_timers(writer: &mut Writer<'_>, timers: &Timers) {
    for value in [
        timers.hold_notice,
        timers.shutdown_notice,
        timers.reconfigure_notice,
        timers.absence,
        timers.stall,
        timers.margin,
        timers.allowance,
    ] {
        writer.write_u64(value);
    }
}

impl Terms {
    /// Decodes terms of either version; [`Terms::check`] is the caller's.
    /// A token hub's terms name a token, and only a token: an XRP hub's
    /// have one encoding.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let versioned = reader
            .clone()
            .read_array::<7>()
            .is_ok_and(|mark| *mark == VERSION_MARK);
        let (asset, gas) = if versioned {
            reader.read_array::<7>()?;
            let version = reader.read_u8()?;
            if version != TOKEN_VERSION {
                return Err(HubDecodeError::UnknownChoice(u64::from(version)));
            }
            let currency = Currency(*reader.read_array::<20>()?);
            let issuer = AccountId(*reader.read_array::<LEDGER_ACCOUNT_LEN>()?);
            if !token_valid(currency, issuer) {
                return Err(HubDecodeError::Invalid("a token hub's asset is no token"));
            }
            (Asset::Token { currency, issuer }, reader.read_u64()?)
        } else {
            (Asset::Xrp, 0)
        };
        let nonce = *reader.read_array::<32>()?;
        let sponsor = read_key(reader)?;
        let network = reader.read_u32()?;
        let escrow = *reader.read_array::<LEDGER_ACCOUNT_LEN>()?;
        let fees = Fees {
            operation: reader.read_u64()?,
            ledger: reader.read_u64()?,
            gas,
            reserve_share_ppm: reader.read_u32()?,
            account_reserve: reader.read_u64()?,
            hold_reserve: reader.read_u64()?,
            starting_balance: reader.read_u64()?,
            least_payout: reader.read_u64()?,
        };
        let limits = Limits {
            accounts: reader.read_u32()?,
            holds: reader.read_u32()?,
            holds_per_account: reader.read_u16()?,
            withdrawals: reader.read_u32()?,
            pending_deposits: reader.read_u32()?,
            recent_deposits: reader.read_u32()?,
            block_operations: reader.read_u16()?,
            deadline_operations: reader.read_u16()?,
            claimed_per_hold: reader.read_u16()?,
            commitments_per_claim: reader.read_u16()?,
            vote_window: reader.read_u16()?,
        };
        let timers = decode_timers(reader)?;
        Ok(Self {
            nonce,
            sponsor,
            network,
            escrow,
            asset,
            fees,
            limits,
            timers,
        })
    }
}

fn decode_timers(reader: &mut Reader<'_>) -> Result<Timers, HubDecodeError> {
    Ok(Timers {
        hold_notice: reader.read_u64()?,
        shutdown_notice: reader.read_u64()?,
        reconfigure_notice: reader.read_u64()?,
        absence: reader.read_u64()?,
        stall: reader.read_u64()?,
        margin: reader.read_u64()?,
        allowance: reader.read_u64()?,
    })
}

impl Encode for Acceptance {
    fn encoded_len(&self) -> usize {
        KEY_LEN * 2 + LEDGER_KEY_LEN * 2 + 32 + size::prefixed(self.endpoint.len()) + SIGNATURE_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        write_key(writer, &self.node);
        write_key(writer, &self.consensus);
        writer.write_bytes(&self.escrow);
        writer.write_bytes(&self.payout);
        writer.write_bytes(&self.payout_chain);
        writer.write_ascii(&self.endpoint, ENDPOINT_SIZE, ENDPOINT_ALPHABET);
        write_signature(writer, &self.signature);
    }
}

impl Acceptance {
    /// Decodes an acceptance; whether it verifies is the caller's to check.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(Self {
            node: read_key(reader)?,
            consensus: read_key(reader)?,
            escrow: *reader.read_array::<LEDGER_KEY_LEN>()?,
            payout: *reader.read_array::<LEDGER_KEY_LEN>()?,
            payout_chain: *reader.read_array::<32>()?,
            endpoint: reader.read_ascii(ENDPOINT_SIZE, ENDPOINT_ALPHABET)?.into(),
            signature: read_signature(reader)?,
        })
    }

    /// Whether `endpoint` is one an acceptance can carry.
    #[must_use]
    pub fn valid_endpoint(endpoint: &str) -> bool {
        ENDPOINT_SIZE.contains(endpoint.len())
            && ENDPOINT_ALPHABET.contains_all(endpoint.as_bytes())
    }
}

impl Encode for Rules {
    fn encoded_len(&self) -> usize {
        self.terms.encoded_len()
            + size::quantity(self.validators.len())
            + self
                .validators
                .iter()
                .map(Encode::encoded_len)
                .sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.terms.encode_to(writer);
        writer.write_quantity(self.validators.len());
        for acceptance in &self.validators {
            acceptance.encode_to(writer);
        }
    }
}

impl Rules {
    /// Decodes rules; [`Rules::check`] is the caller's.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let terms = Terms::decode(reader)?;
        let count = reader.read_quantity(MAX_VALIDATORS)?;
        if count < MIN_VALIDATORS {
            return Err(HubDecodeError::Invalid("too few validators"));
        }
        let mut validators = Vec::with_capacity(count);
        for _ in 0..count {
            validators.push(Acceptance::decode(reader)?);
        }
        Ok(Self { terms, validators })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Terms that pass [`Terms::check`], for tests.
    pub(crate) fn terms(sponsor: &NodeKey) -> Terms {
        Terms {
            nonce: [7; 32],
            sponsor: sponsor.public_key(),
            network: 1,
            escrow: [9; LEDGER_ACCOUNT_LEN],
            asset: Asset::Xrp,
            fees: Fees {
                operation: 10,
                ledger: 12,
                gas: 0,
                reserve_share_ppm: 100_000,
                account_reserve: 2_000_000,
                hold_reserve: 200_000,
                starting_balance: 1_000,
                least_payout: 1_000_000,
            },
            limits: Limits {
                accounts: 1_000,
                holds: 4_000,
                holds_per_account: 64,
                withdrawals: 256,
                pending_deposits: 256,
                recent_deposits: 4_096,
                block_operations: 512,
                deadline_operations: 128,
                claimed_per_hold: 1_100,
                commitments_per_claim: 64,
                vote_window: 32,
            },
            timers: Timers {
                hold_notice: 60_000,
                shutdown_notice: 3_600_000,
                reconfigure_notice: 86_400_000,
                absence: 3 * 86_400_000,
                stall: 3_600_000,
                margin: 600,
                allowance: 300,
            },
        }
    }

    /// A usable escrow key: Ed25519, as the ledger writes it.
    pub(crate) fn escrow_key(seed: u8) -> [u8; LEDGER_KEY_LEN] {
        let mut key = [0xed; LEDGER_KEY_LEN];
        let point = NodeKey::from_bytes(&[seed.wrapping_add(10); 32]).public_key();
        key[1..].copy_from_slice(point.as_bytes());
        key
    }

    /// A usable payout key.
    pub(crate) fn payout_key(seed: u8) -> crate::escrow::bip32::ExtendedPublic {
        crate::escrow::bip32::ExtendedPrivate::master(&[seed; 32])
            .and_then(|key| key.public())
            .unwrap()
    }

    /// The rules of a hub of `n` validators whose node keys are seeds
    /// 1..=n, for tests.
    pub(crate) fn rules(n: u8) -> (Rules, Vec<NodeKey>) {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let mut terms = terms(&sponsor);
        terms.timers.margin = least_margin_ms(usize::from(n));
        let consensus: Vec<NodeKey> = (1..=n)
            .map(|i| NodeKey::from_bytes(&[i + 50; 32]))
            .collect();
        let validators = (1..=n)
            .zip(&consensus)
            .map(|(i, key)| {
                Acceptance {
                    node: key.public_key(),
                    consensus: key.public_key(),
                    escrow: escrow_key(i),
                    payout: payout_key(i).key,
                    payout_chain: payout_key(i).chain,
                    endpoint: alloc::format!("v{i}.example:443"),
                    signature: Signature::from_bytes([0; 64]),
                }
                .sign(&NodeKey::from_bytes(&[i; 32]), &terms)
            })
            .collect();
        (Rules { terms, validators }, consensus)
    }

    // spec/areas/hubs.md#consensus: the margin covers what finality takes
    // with f validators faulty, the precision and an honest leader's block
    // after f skipped views, or the rules do not check, for validators and
    // members alike.
    #[test]
    fn the_margin_covers_finality_with_f_faulty() {
        assert_eq!(least_margin_ms(4), 600);
        assert_eq!(least_margin_ms(7), 850);
        let (mut rules, _) = rules(4);
        rules.check().unwrap();
        rules.terms.timers.margin = 599;
        assert_eq!(
            rules.check(),
            Err(RulesError::Terms(
                "margin below what finality takes with f validators faulty"
            ))
        );
    }

    #[test]
    fn rules_check_and_round_trip() {
        let (rules, _) = rules(4);
        rules.check().unwrap();
        let bytes = rules.to_vec();
        let mut reader = Reader::new(&bytes);
        let decoded = Rules::decode(&mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(decoded, rules);
        assert_eq!(decoded.id(), rules.id());
    }

    #[test]
    fn unusable_keys_are_refused() {
        let escrow = rules(4).0.terms.escrow;
        let mut master = [0; LEDGER_KEY_LEN];
        // A key whose account is the escrow itself cannot be on its list.
        let (mut own, _) = rules(4);
        own.validators[1].escrow = escrow_key(1);
        let own_account = dassie_xrpl::keys::PublicKey::from_bytes(own.validators[1].escrow)
            .unwrap()
            .account()
            .0;
        own.terms.escrow = own_account;
        assert!(!own.validators[1].keys_usable(&own.terms.escrow));
        master[0] = 0x02;
        let (mut bad, _) = rules(4);
        bad.validators[2].payout = master;
        assert!(!bad.validators[2].keys_usable(&escrow));
        let (mut bad, _) = rules(4);
        bad.validators[3].escrow = [7; LEDGER_KEY_LEN];
        assert!(!bad.validators[3].keys_usable(&escrow));
        let (good, _) = rules(4);
        assert!(good.validators.iter().all(|a| a.keys_usable(&escrow)));
    }

    #[test]
    fn a_changed_acceptance_fails() {
        let (mut rules, _) = rules(4);
        rules.validators[2].endpoint = "evil.example:443".into();
        assert_eq!(rules.check(), Err(RulesError::Acceptance(2)));
    }

    #[test]
    fn terms_relations() {
        let sponsor = NodeKey::from_bytes(&[1; 32]);
        let mut bad = terms(&sponsor);
        bad.timers.hold_notice = MAX_HOLD_MS;
        assert!(bad.check().is_err());
        let mut bad = terms(&sponsor);
        bad.fees.account_reserve = bad.fees.least_payout;
        assert!(bad.check().is_err());
        let mut bad = terms(&sponsor);
        bad.limits.claimed_per_hold = 1_000;
        assert!(bad.check().is_err());
        // A stall shorter than recovery from the longest outage, or longer
        // than an engine holds the views of.
        for stall in [MIN_STALL_MS - 1, MAX_STALL_MS + 1] {
            let mut bad = terms(&sponsor);
            bad.timers.stall = stall;
            assert!(bad.check().is_err(), "{stall}");
        }
        for stall in [MIN_STALL_MS, MAX_STALL_MS] {
            let mut good = terms(&sponsor);
            good.timers.stall = stall;
            assert_eq!(good.check(), Ok(()), "{stall}");
        }
    }

    /// `terms` as a DSUSD hub's: the token, no ledger fee but the least gas
    /// in its place, and a least payout of a millionth.
    pub(crate) fn token_terms(mut terms: Terms) -> Terms {
        terms.asset = Asset::Token {
            currency: dassie_xrpl::dsusd::CURRENCY,
            issuer: dassie_xrpl::dsusd::ISSUER,
        };
        terms.fees.gas = LEAST_TOKEN_GAS_DROPS;
        terms.fees.ledger = 0;
        terms.fees.least_payout = 1;
        terms
    }

    // A token hub's terms carry the token and the gas, round-trip, and name
    // a hub other than the same terms in XRP would.
    #[test]
    fn token_terms_round_trip_and_name_another_hub() {
        let sponsor = NodeKey::from_bytes(&[3; 32]);
        let xrp = terms(&sponsor);
        let token = token_terms(xrp.clone());
        token.check().unwrap();
        let bytes = token.to_vec();
        assert_eq!(bytes.len(), token.encoded_len());
        assert_eq!(bytes.len(), xrp.encoded_len() + TOKEN_HEADER_LEN);
        assert!(bytes.starts_with(&VERSION_MARK));
        let mut reader = Reader::new(&bytes);
        assert_eq!(Terms::decode(&mut reader).unwrap(), token);
        reader.finish().unwrap();
        assert_ne!(token.digest(), xrp.digest());
        assert_eq!(token.transaction_fee(), LEAST_TOKEN_GAS_DROPS);
        assert_eq!(
            token.units_per_amount(),
            dassie_xrpl::token::UNITS_PER_MICRO
        );
        // An unknown version, or an XRP "token", does not decode.
        let mut later = bytes.clone();
        later[VERSION_MARK.len()] = 2;
        assert!(Terms::decode(&mut Reader::new(&later)).is_err());
        let mut xrp_token = bytes;
        xrp_token[VERSION_MARK.len() + 1..VERSION_MARK.len() + 21].fill(0);
        assert!(Terms::decode(&mut Reader::new(&xrp_token)).is_err());
    }

    #[test]
    fn fee_checks_follow_the_asset() {
        let sponsor = NodeKey::from_bytes(&[3; 32]);
        let fails = |change: &dyn Fn(&mut Terms), token: bool| {
            let mut terms = terms(&sponsor);
            if token {
                terms = token_terms(terms);
            }
            change(&mut terms);
            terms.check().is_err()
        };
        // An XRP hub pays out at least the ledger's base reserve, has no
        // gas, and no nonce that reads as a later version.
        assert!(fails(
            &|t| t.fees.least_payout = LEDGER_BASE_RESERVE - 1,
            false
        ));
        assert!(fails(&|t| t.fees.gas = 1, false));
        assert!(fails(
            &|t| t.nonce[..7].copy_from_slice(&VERSION_MARK),
            false
        ));
        // A token hub's least payout is any amount but none; its fees are
        // gas, enough for the largest set of validators and at most what
        // the escrow keeps; and its token is a token of
        // an issuer other than the escrow.
        assert!(!fails(&|t| t.fees.least_payout = 1, true));
        assert!(fails(&|t| t.fees.least_payout = 0, true));
        assert!(fails(&|t| t.fees.ledger = 1, true));
        assert!(fails(&|t| t.fees.gas = 0, true));
        assert!(fails(&|t| t.fees.gas = LEAST_TOKEN_GAS_DROPS - 1, true));
        assert_eq!(LEAST_TOKEN_GAS_DROPS, 170, "sixteen signatures and one");
        assert!(!fails(&|t| t.fees.gas = LEAST_ESCROW_GAS_DROPS, true));
        assert!(fails(&|t| t.fees.gas = LEAST_ESCROW_GAS_DROPS + 1, true));
        assert!(fails(
            &|t| {
                t.asset = Asset::Token {
                    currency: dassie_xrpl::dsusd::CURRENCY,
                    issuer: AccountId(t.escrow),
                };
            },
            true
        ));
        // New rules keep the asset and the gas.
        let xrp = terms(&sponsor);
        let token = token_terms(xrp.clone());
        assert!(token.keeps_tables_of(&token));
        assert!(!token.keeps_tables_of(&xrp) && !xrp.keeps_tables_of(&token));
        let mut more_gas = token.clone();
        more_gas.fees.gas += 1;
        assert!(!more_gas.keeps_tables_of(&token));
    }

    // Millionths of a token are its value exactly up to sixteen digits, and
    // rounded down beyond; what the escrow is paid counts in whole
    // millionths, rounded down, and only in the hub's token.
    #[test]
    fn token_amounts_and_the_hubs_unit() {
        let xrp = terms(&NodeKey::from_bytes(&[3; 32]));
        let token = token_terms(xrp.clone());
        let value = |text: &str| TokenValue::parse(text).unwrap();
        assert_eq!(token_value(1_500_000), value("1.5"));
        assert_eq!(token_value(1), value("0.000001"));
        assert_eq!(token_value(0), TokenValue::ZERO);
        assert_eq!(
            token_value(12_345_678_901_234_567),
            value("12345678901.23456")
        );
        assert_eq!(token_value(u64::MAX), value("18446744073709.55"));
        let dsusd = |text: &str| {
            dassie_xrpl::codec::Amount::Token(dassie_xrpl::dsusd::TOKEN.amount(value(text)))
        };
        assert_eq!(token.ledger_amount(2_000_000), dsusd("2"));
        assert_eq!(token.hub_amount(&dsusd("2.0000019")), Some(2_000_001));
        assert_eq!(token.hub_amount(&dassie_xrpl::codec::Amount::Xrp(5)), None);
        let mut other = dassie_xrpl::dsusd::TOKEN.amount(value("2"));
        other.issuer = AccountId([1; 20]);
        assert_eq!(
            token.hub_amount(&dassie_xrpl::codec::Amount::Token(other)),
            None
        );
        assert_eq!(xrp.hub_amount(&dassie_xrpl::codec::Amount::Xrp(5)), Some(5));
        assert_eq!(xrp.hub_amount(&dsusd("2")), None);
        assert_eq!(xrp.ledger_amount(7), dassie_xrpl::codec::Amount::Xrp(7));
    }
}
