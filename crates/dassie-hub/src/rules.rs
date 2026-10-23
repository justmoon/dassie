//! A hub's founding document (spec/designs/settlement-hubs.md#2-founding-a-hub):
//! the terms the sponsor fixes, and the acceptances of its validators. The
//! hub ID is the digest of the rules.
//!
//! Values in the terms are starting points for the simulation to set; what
//! the code fixes here is which values exist, how they bound each other
//! ([`Terms::check`]), and their encoding, which every hub ID depends on.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey, Signature};
use dassie_oer::{Alphabet, Encode, Reader, Size, Writer, size};

use crate::codec::{
    HubDecodeError, KEY_LEN, SIGNATURE_LEN, read_key, read_signature, write_key, write_signature,
};
use crate::digest::{Digest, Domain};
use crate::validators::{MAX_VALIDATORS, MIN_VALIDATORS, ValidatorSet};

/// An amount of the hub's asset, in its smallest unit (drops of XRP).
pub type Amount = u64;

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

/// The fees of a hub, all in drops (spec/designs/settlement-hubs.md#7-fees).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fees {
    /// What each operation pays.
    pub operation: Amount,
    /// The underlying ledger's fee for one transaction the escrow sends,
    /// multi-signed by a quorum: at least the ledger's base fee once per
    /// signer and once more. A withdrawal pays it on top of the operation
    /// fee, and so does each payment of a payout set.
    pub ledger: Amount,
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
    /// How far a block's timestamp may be from a validator's clock.
    pub precision: Millis,
    /// M: how much earlier than its Prepare a commitment expires.
    pub margin: Millis,
    /// The legs between members a peering over the hub allows for.
    pub allowance: Millis,
    /// How long a validator waits for a view's block before it skips it.
    pub view_timeout: Millis,
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
        // A payout to a new account on the XRP Ledger must create it
        // (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
        if fees.least_payout < LEDGER_BASE_RESERVE {
            return fail("least payout below the underlying ledger's base reserve");
        }
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
        if timers.hold_notice <= MAX_HOLD_MS || timers.margin == 0 || timers.precision == 0 {
            return fail("hold notice not longer than a packet may be held");
        }
        // A member that does not accept new rules can take its holds back
        // and leave before they apply (spec/designs/settlement-hubs.md#8-changing-the-validators).
        if timers.reconfigure_notice <= timers.hold_notice {
            return fail("reconfiguration notice not longer than a hold's");
        }
        if timers.stall <= timers.view_timeout || timers.absence <= timers.stall {
            return fail("stall or absence not longer than a view");
        }
        Ok(())
    }

    /// Whether these terms keep every table at least as large as
    /// `current`'s. New rules may not shrink a table: what it holds could
    /// no longer fit.
    #[must_use]
    pub const fn keeps_tables_of(&self, current: &Self) -> bool {
        let (new, old) = (&self.limits, &current.limits);
        new.accounts >= old.accounts
            && new.holds >= old.holds
            && new.holds_per_account >= old.holds_per_account
            && new.withdrawals >= old.withdrawals
            && new.pending_deposits >= old.pending_deposits
            && new.recent_deposits >= old.recent_deposits
            && new.claimed_per_hold >= old.claimed_per_hold
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

/// The longest a connector holds a packet (spec/decisions/connector-timing.md),
/// which a hold's notice must exceed.
pub const MAX_HOLD_MS: Millis = 30_000;

/// The XRP Ledger's base reserve, in drops: the least a payment that
/// creates an account delivers.
pub const LEDGER_BASE_RESERVE: Amount = 1_000_000;

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

    /// Checks the terms, the set and every acceptance.
    pub fn check(&self) -> Result<(), RulesError> {
        self.terms.check()?;
        if self.validator_set().is_none() {
            return Err(RulesError::Validators);
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
            if !acceptance.verifies(&terms, &self.terms.nonce) {
                return Err(RulesError::Acceptance(i));
            }
        }
        Ok(())
    }
}

const FEES_LEN: usize = 8 * 6 + 4;
const LIMITS_LEN: usize = 4 * 5 + 2 * 6;
const TIMERS_LEN: usize = 8 * 9;
const TERMS_LEN: usize = 32 + KEY_LEN + 4 + LEDGER_ACCOUNT_LEN + FEES_LEN + LIMITS_LEN + TIMERS_LEN;

impl Encode for Terms {
    fn encoded_len(&self) -> usize {
        TERMS_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
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
        timers.precision,
        timers.margin,
        timers.allowance,
        timers.view_timeout,
    ] {
        writer.write_u64(value);
    }
}

impl Terms {
    /// Decodes terms; [`Terms::check`] is the caller's.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let nonce = *reader.read_array::<32>()?;
        let sponsor = read_key(reader)?;
        let network = reader.read_u32()?;
        let escrow = *reader.read_array::<LEDGER_ACCOUNT_LEN>()?;
        let fees = Fees {
            operation: reader.read_u64()?,
            ledger: reader.read_u64()?,
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
        precision: reader.read_u64()?,
        margin: reader.read_u64()?,
        allowance: reader.read_u64()?,
        view_timeout: reader.read_u64()?,
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
        TERMS_LEN
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
            fees: Fees {
                operation: 10,
                ledger: 12,
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
                precision: 100,
                margin: 500,
                allowance: 300,
                view_timeout: 200,
            },
        }
    }

    /// The rules of a hub of `n` validators whose node keys are seeds
    /// 1..=n, for tests.
    pub(crate) fn rules(n: u8) -> (Rules, Vec<NodeKey>) {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let terms = terms(&sponsor);
        let consensus: Vec<NodeKey> = (1..=n)
            .map(|i| NodeKey::from_bytes(&[i + 50; 32]))
            .collect();
        let validators = (1..=n)
            .zip(&consensus)
            .map(|(i, key)| {
                Acceptance {
                    node: key.public_key(),
                    consensus: key.public_key(),
                    escrow: [i; LEDGER_KEY_LEN],
                    payout: [i.wrapping_add(100); LEDGER_KEY_LEN],
                    payout_chain: [i; 32],
                    endpoint: alloc::format!("v{i}.example:443"),
                    signature: Signature::from_bytes([0; 64]),
                }
                .sign(&NodeKey::from_bytes(&[i; 32]), &terms)
            })
            .collect();
        (Rules { terms, validators }, consensus)
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
    }
}
