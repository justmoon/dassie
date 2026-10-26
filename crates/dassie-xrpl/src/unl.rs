//! The UNL: the validators enough of the trusted publishers' lists name,
//! each with its current signing key, and the quorum of them that makes a
//! ledger validated, as rippled's defaults have them
//! (spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers),
//! sans-IO.
//!
//! - **A list counts** when its publisher's manifest verifies under the
//!   configured master key, its blob under the publisher's current signing
//!   key, its sequence is not below the highest seen of that publisher, and
//!   it has not expired. One that takes effect later waits as pending until
//!   then; a publisher keeps one current list and up to [`MAX_PENDING`]
//!   pending, each above the one before in sequence and taking effect
//!   later. As in rippled's `ValidatorList::updateTrusted`, the latest of
//!   them that took effect is the list in effect, and the ones it passed
//!   over go. A publisher that revoked its key is unavailable for good.
//! - **Manifests** of validators come from the lists and from the server's
//!   `manifest` method alike, and count only for a master a list in effect
//!   names and only when that master signed them, as rippled takes them: a
//!   list's own are taken as it takes effect, so a pending list's wait with
//!   it. The highest sequence wins, and a revocation leaves the master no
//!   key to sign with. One taken stays while any kept list names its
//!   master, so a list that takes effect later with an older one never
//!   hands its signing back. A signing key serves one master only, and
//!   never is a kept list's master's own key.
//! - **The UNL** is the validators that at least [`list_threshold`] of the
//!   lists in effect name, less any whose master revoked its key, as
//!   rippled's `ValidatorList::updateTrusted` has it: with one or two
//!   publishers, the union. There is none while [`unavailable_limit`]
//!   publishers or more have no list in effect, as rippled then has no
//!   quorum, nor while it names no validator, nor while it is UNL-blocked,
//!   as rippled's `NetworkOPs` has it: from when a list in effect expires
//!   or the lists name no validator, as before the first came, until every
//!   publisher has a list in effect; a publisher that revokes its key sets
//!   no block, though it keeps one already set. Quorum is [`quorum`], with the whole UNL effective:
//!   there is no negative UNL.
//! - **A ledger is validated** when a quorum of the UNL signed its hash with
//!   full, fully canonical validations, each signed by its master's current
//!   signing key at a time fresh by the node's clock. A master counts once,
//!   and not at all if it also signed another ledger of the same index. The
//!   cheapest checks come first, so a server's answer costs at most two
//!   signature checks per master: its validation of the ledger, and one of
//!   another.
//!
//! Time is an input, `now_ms`, in milliseconds since 1970; the ledger's
//! times are seconds since 2000-01-01. Validation signatures are checked
//! behind [`ValidationSignatures`], which a simulation substitutes.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::cmp::Ordering;

use serde_json::{Value, json};

use crate::header::LedgerHeader;
use crate::keys::{PublicKey, Signature};
use crate::manifest::Manifest;
use crate::validation::{FULL, FULLY_CANONICAL, Validation};
use crate::validator_list::{
    List, ListError, MAX_BLOBS, MAX_LIST_BYTES, MAX_LIST_VALIDATORS, MAX_PUBLISHERS, Message,
    SignedBlob, UnlConfig,
};
use crate::{from_hex, hex_upper};

/// The most lists of one publisher kept pending: as many as one answer of
/// its site carries.
pub const MAX_PENDING: usize = MAX_BLOBS;
/// The most validator manifests kept: one for each master any kept list
/// names, the current one and the pending ones of each publisher.
pub const MAX_MANIFESTS: usize = (1 + MAX_PENDING) * MAX_PUBLISHERS * MAX_LIST_VALIDATORS;
/// The most validations read for one ledger: mainnet's servers hear about
/// a hundred, of every validator they hear.
pub const MAX_VALIDATIONS: usize = 1_024;
/// How long after its signing a validation is fresh: as long as rippled
/// holds one current.
pub const FRESH_FOR_MS: u64 = 180_000;
/// How far past the node's clock a validation's signing time may be, as
/// rippled allows it.
pub const FRESH_AHEAD_MS: u64 = 300_000;
/// The ledger's epoch, 2000-01-01, in seconds since 1970.
pub const RIPPLE_EPOCH_UNIX_S: u64 = 946_684_800;

/// The ledger's time, in seconds since 2000-01-01, at `now_ms`.
#[must_use]
pub const fn ledger_seconds(now_ms: u64) -> u64 {
    (now_ms / 1_000).saturating_sub(RIPPLE_EPOCH_UNIX_S)
}

/// How many of a UNL of `validators`, `effective` of them not on a
/// negative UNL, make a quorum, as rippled's
/// `ValidatorList::calculateQuorum` has it: four fifths of the effective
/// ones or three fifths of all, whichever is more, each rounded up, and
/// each reckoned in `f32` as rippled does, which makes three fifths of
/// some sizes one more than the exact fraction. The node has no negative
/// UNL, so it passes the UNL's size as both, and then the four fifths are
/// always the more.
#[must_use]
pub fn quorum(validators: usize, effective: usize) -> usize {
    assert!(effective <= validators, "the effective UNL is part of it");
    let most = ceil_f32(effective, FOUR_FIFTHS_F32);
    let least = ceil_f32(validators, THREE_FIFTHS_F32);
    most.max(least)
}

/// `0.8f`, as an `f32` holds it, in 2^-24ths.
const FOUR_FIFTHS_F32: u64 = 13_421_773;
/// `0.6f`, so.
const THREE_FIFTHS_F32: u64 = 10_066_330;
/// The bits of an `f32`'s significand.
const F32_BITS: u32 = 24;

/// `n` times `fraction`, in 2^-24ths, rounded up to a whole number as
/// `f32` reckons it: the product rounded to the nearest `f32`, a tie to the
/// even one, then up. `n` itself is an `f32` exactly up to 2^24, far more
/// validators than a UNL holds.
fn ceil_f32(n: usize, fraction: u64) -> usize {
    let n = u64::try_from(n).unwrap_or(u64::MAX);
    assert!(n <= 1 << F32_BITS, "an f32 holds the UNL's size exactly");
    let product = n * fraction;
    let bits = u64::BITS - product.leading_zeros();
    let rounded = if bits <= F32_BITS {
        product
    } else {
        let shift = bits - F32_BITS;
        let kept = product >> shift;
        let rest = product & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        let up = rest > half || (rest == half && kept % 2 == 1);
        (kept + u64::from(up)) << shift
    };
    usize::try_from(rounded.div_ceil(1 << F32_BITS)).unwrap_or(usize::MAX)
}

/// How many publishers' lists must name a validator for it to be on the
/// UNL, of `publishers` configured: rippled's default
/// `validator_list_threshold`, one with fewer than three publishers, else
/// a majority.
#[must_use]
pub const fn list_threshold(publishers: usize) -> usize {
    assert!(publishers > 0, "a UNL has a publisher");
    if publishers < 3 {
        1
    } else {
        publishers / 2 + 1
    }
}

/// How many of `publishers` without a list in effect leave no quorum, as
/// rippled's `ValidatorList::calculateQuorum` has it: as many as could keep
/// a validator below [`list_threshold`] that would otherwise be on the
/// UNL, or as leave too few lists for any validator to reach it, whichever
/// is fewer.
#[must_use]
pub const fn unavailable_limit(publishers: usize) -> usize {
    let threshold = list_threshold(publishers);
    let none_reach = publishers - threshold + 1;
    if threshold < none_reach {
        threshold
    } else {
        none_reach
    }
}

/// Checks a validation's signature, as the node does with the real
/// signature and a simulation with its own (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
pub trait ValidationSignatures {
    /// Whether `validation` is signed by its signing key.
    fn signed(&self, validation: &Validation) -> bool;
}

/// The real check: ECDSA on secp256k1, fully canonical.
#[derive(Clone, Copy, Debug, Default)]
pub struct Secp256k1Signatures;

impl ValidationSignatures for Secp256k1Signatures {
    fn signed(&self, validation: &Validation) -> bool {
        (validation.signing_key).verify(&validation.signing_bytes(), &validation.signature)
    }
}

/// What became of a list taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// It is its publisher's current list.
    Current,
    /// It takes effect later, and waits until then.
    Pending,
    /// Its publisher's current or pending list already.
    Same,
    /// Its publisher revoked its key: the publisher is unavailable for
    /// good.
    Revoked,
}

/// Why a publisher has no list in effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// None was taken yet, or none that is in effect.
    NoList,
    /// Its list expired, and no newer one came.
    Expired,
    /// It revoked its key.
    Revoked,
}

/// Why there is no UNL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlError {
    /// [`unavailable_limit`] publishers or more have no list in effect:
    /// the first of them, by its place in the config.
    Unavailable {
        /// The publisher.
        publisher: usize,
        /// Why.
        why: Unavailable,
    },
    /// UNL-blocked, as rippled is: a list in effect expired, or the lists
    /// named no validator, and not every publisher has had a list in
    /// effect since.
    Blocked,
    /// Enough lists are in effect, but they name no validator often
    /// enough that has not revoked its key.
    Empty,
}

/// Why a validator's manifest is not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// It is not a manifest.
    Malformed,
    /// No list in effect names its master.
    NotListed,
    /// Its keys did not sign it.
    BadSignature,
    /// Its signing key is a kept list's master's, or another master's
    /// signing key.
    KeyInUse,
}

/// What a tally of one ledger's validations found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// The UNL's validators that signed the ledger.
    pub counted: usize,
    /// How many make a quorum.
    pub quorum: usize,
    /// How many validators the UNL has.
    pub validators: usize,
    /// The UNL's validators that did not count, in key order: for each,
    /// the account may ask the server's `manifest` method for a newer
    /// manifest, in case it signed with a key the node does not know yet.
    pub missing: Vec<PublicKey>,
    /// Validations of the index by signing keys no listed master's
    /// manifest names: the UNL's validators with keys not known yet, and
    /// every validator not on it.
    pub unknown: usize,
    /// The UNL's validators that did not count, whose validation of the
    /// ledger is signed, at a time not fresh: each once, however often it
    /// came, and only once its signature checks.
    pub stale: usize,
    /// The UNL's validators that also signed another ledger of the index,
    /// and so count for neither.
    pub conflicting: usize,
    /// Validations not read, not full, not fully canonical or not signed
    /// by their keys.
    pub refused: usize,
    /// Validations by validators not on the UNL.
    pub untrusted: usize,
    /// Validations of another ledger index.
    pub elsewhere: usize,
}

impl Tally {
    /// Whether a quorum signed.
    #[must_use]
    pub const fn reached(&self) -> bool {
        self.counted >= self.quorum && self.validators > 0
    }
}

/// Why a tally has no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TallyError {
    /// There is no UNL.
    Unl(UnlError),
    /// More than [`MAX_VALIDATIONS`] came.
    TooMany,
}

/// A validated ledger the account may act from: its index, close time
/// and hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// Its index.
    pub index: u32,
    /// When it closed, in seconds since 2000-01-01.
    pub close_time: u32,
    /// Its hash.
    pub hash: [u8; 32],
}

/// Where a ledger stands against the anchor kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// It is the kept anchor.
    Same,
    /// Later: a higher index that closed later.
    Newer,
    /// Earlier: a lower index that closed earlier. Never current.
    Older,
    /// Neither: another ledger of the index, or one whose index and close
    /// time disagree, which only another chain gives; whether it is a reset
    /// or a fork is the chain's to judge.
    Diverged,
}

impl Anchor {
    /// The ledger `header` is.
    #[must_use]
    pub fn of(header: &LedgerHeader) -> Self {
        Self {
            index: header.sequence,
            close_time: header.close_time,
            hash: header.hash(),
        }
    }

    /// Where it stands against `kept`. Along one chain both the index and
    /// the close time grow.
    #[must_use]
    pub fn relation(&self, kept: &Self) -> Relation {
        match (
            self.index.cmp(&kept.index),
            self.close_time.cmp(&kept.close_time),
        ) {
            _ if self.hash == kept.hash => Relation::Same,
            (Ordering::Greater, Ordering::Greater) => Relation::Newer,
            (Ordering::Less, Ordering::Less) => Relation::Older,
            _ => Relation::Diverged,
        }
    }
}

/// An anchor chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchored {
    /// The ledger.
    pub anchor: Anchor,
    /// Where it stands against the anchor kept, if one was.
    pub relation: Option<Relation>,
    /// Its validations.
    pub tally: Tally,
}

/// Why a ledger is no anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnchorError {
    /// There is no UNL.
    Unl(UnlError),
    /// More than [`MAX_VALIDATIONS`] came.
    TooMany,
    /// It is older than the anchor kept.
    Older,
    /// Too few of the UNL signed it, freshly.
    NoQuorum(Tally),
}

/// A list kept, as it came: its blob and signature, and the publisher's
/// manifest it verified under.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Kept {
    list: List,
    blob: Vec<u8>,
    signature: Signature,
    manifest: Manifest,
}

impl Kept {
    fn state(&self) -> Value {
        json!({
            "blob": hex_upper(&self.blob),
            "signature": hex_upper(self.signature.as_bytes()),
            "manifest": hex_upper(&self.manifest.to_bytes()),
        })
    }

    /// One the account wrote, unverified; `None` if it is not that.
    fn restore(value: &Value) -> Option<Option<Self>> {
        if value.is_null() {
            return Some(None);
        }
        let blob = from_hex(value.get("blob")?.as_str()?)?;
        if blob.len() > MAX_LIST_BYTES {
            return None;
        }
        let signature = Signature::from_slice(&from_hex(value.get("signature")?.as_str()?)?)?;
        let manifest = Manifest::from_bytes(&from_hex(value.get("manifest")?.as_str()?)?).ok()?;
        Some(Some(Self {
            list: List::parse(&blob).ok()?,
            blob,
            signature,
            manifest,
        }))
    }

    /// The pending lists the account wrote, unverified: at most
    /// [`MAX_PENDING`], or one or none as written before it kept more.
    /// `None` if it is not that.
    fn restore_pending(value: &Value) -> Option<Vec<Self>> {
        let Some(entries) = value.as_array() else {
            return Some(Self::restore(value)?.into_iter().collect());
        };
        if entries.len() > MAX_PENDING {
            return None;
        }
        let mut pending = Vec::with_capacity(entries.len());
        for entry in entries {
            pending.push(Self::restore(entry)??);
        }
        Some(pending)
    }

    fn expired(&self, now_s: u64) -> bool {
        u64::from(self.list.expiration) <= now_s
    }

    /// Whether it names a time it takes effect, and that time has come by
    /// `now_s`.
    fn effective_by(&self, now_s: u64) -> bool {
        (self.list.effective).is_some_and(|effective| u64::from(effective) <= now_s)
    }
}

/// What the account holds of one publisher.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PublisherState {
    /// Its newest manifest; a revocation ends it.
    manifest: Option<Manifest>,
    /// The highest list sequence seen of it.
    highest: Option<u32>,
    current: Option<Kept>,
    /// The lists that take effect later, at most [`MAX_PENDING`], each of a
    /// higher sequence than the current and the one before it, and taking
    /// effect later than the one before it: rippled's remaining lists.
    pending: Vec<Kept>,
}

impl PublisherState {
    fn revoked(&self) -> bool {
        self.manifest.as_ref().is_some_and(Manifest::is_revocation)
    }

    fn kept(&self) -> impl Iterator<Item = &Kept> {
        self.current.iter().chain(self.pending.iter())
    }

    /// How many of its pending lists have taken effect by `now_s`: the
    /// first ones, as each takes effect later than the one before.
    fn due(&self, now_s: u64) -> usize {
        (self.pending.iter())
            .take_while(|kept| kept.effective_by(now_s))
            .count()
    }

    /// The list that is in effect at `now_s`, or that becomes so once it
    /// moves on to `now_s`, expired or not: the latest pending list that
    /// took effect, as rippled's `updateTrusted` rotates it in, else the
    /// current one.
    fn latest(&self, now_s: u64) -> Option<&Kept> {
        let due = self.due(now_s);
        (due.checked_sub(1))
            .and_then(|last| self.pending.get(last))
            .or(self.current.as_ref())
    }

    /// Whether the list that is in effect, or that becomes so by `now_s`,
    /// has expired by then: as rippled's `updateTrusted` sees an available
    /// publisher's list expire, once it has moved a due pending list in.
    fn lapsed(&self, now_s: u64) -> bool {
        self.latest(now_s).is_some_and(|kept| kept.expired(now_s))
    }

    /// Its list in effect at `now_s`.
    fn in_effect(&self, now_s: u64) -> Result<&Kept, Unavailable> {
        if self.revoked() {
            return Err(Unavailable::Revoked);
        }
        match self.latest(now_s) {
            Some(kept) if !kept.expired(now_s) => Ok(kept),
            Some(_) => Err(Unavailable::Expired),
            // One it had expired, and went.
            None if self.pending.is_empty() && self.highest.is_some() => Err(Unavailable::Expired),
            None => Err(Unavailable::NoList),
        }
    }

    /// Moves on to `now_s`, as rippled's `updateTrusted` rotates a
    /// publisher's lists: the latest pending list that took effect becomes
    /// the current one, and those before it go. Whether one did.
    fn rotate(&mut self, now_s: u64) -> bool {
        let due = self.due(now_s);
        if due == 0 {
            return false;
        }
        self.current = self.pending.drain(..due).next_back();
        assert!(self.current.is_some(), "a due list became current");
        true
    }
}

/// Whether a publisher's `current` list and `pending` ones are as the
/// account keeps them: at most [`MAX_PENDING`] pending, each naming when it
/// takes effect, above the one before it in sequence, the current one
/// first if there is one, and taking effect later than the pending one
/// before it. Restore and the invariants check the same.
fn in_order(current: Option<&Kept>, pending: &[Kept]) -> bool {
    let sequences = (current.into_iter().chain(pending)).map(|kept| kept.list.sequence);
    let ascending =
        (sequences.clone().zip(sequences.skip(1))).all(|(before, after)| before < after);
    let effective = pending.iter().map(|kept| kept.list.effective);
    let later = (effective.clone().zip(effective.skip(1))).all(|(before, after)| before < after);
    let waits = pending.iter().all(|kept| kept.list.effective.is_some());
    pending.len() <= MAX_PENDING && ascending && later && waits
}

/// The UNL of one account, and what makes it: its publishers' lists and
/// its validators' manifests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unl {
    config: UnlConfig,
    /// One for each publisher of the config, in its order.
    publishers: Vec<PublisherState>,
    /// The newest manifest of each master a kept list names.
    manifests: BTreeMap<PublicKey, Manifest>,
    /// Each signing key of a manifest kept, and its master.
    signers: BTreeMap<PublicKey, PublicKey>,
    /// UNL-blocked, as rippled's `NetworkOPs` has it: set when a list in
    /// effect expires or the lists name no validator, and cleared only
    /// once every publisher has a list in effect. While set, there is no
    /// UNL.
    blocked: bool,
}

impl Unl {
    /// One that holds no list yet, and so is UNL-blocked, as rippled is
    /// once it finds no validator to trust. A config that does not pass
    /// [`UnlConfig::check`] is a bug.
    #[must_use]
    pub fn new(config: UnlConfig) -> Self {
        assert!(config.check(), "a config an account can follow");
        Self {
            publishers: alloc::vec![PublisherState::default(); config.publishers.len()],
            config,
            manifests: BTreeMap::new(),
            signers: BTreeMap::new(),
            blocked: true,
        }
    }

    /// Whether it is UNL-blocked at `now_ms`: blocked already, or a list in
    /// effect expired since.
    #[must_use]
    pub fn blocked(&self, now_ms: u64) -> bool {
        let now_s = ledger_seconds(now_ms);
        self.blocked || self.publishers.iter().any(|state| state.lapsed(now_s))
    }

    /// Clears the block once every publisher has a list in effect at
    /// `now_s`, then sets it if the lists name no validator, as rippled's
    /// `updateTrusted` does after it has moved the lists on.
    fn settle(&mut self, now_s: u64) {
        if (self.publishers.iter()).all(|state| state.in_effect(now_s).is_ok()) {
            self.blocked = false;
        }
        if self.trusted_set(now_s).is_empty() {
            self.blocked = true;
        }
    }

    /// Its config.
    #[must_use]
    pub const fn config(&self) -> &UnlConfig {
        &self.config
    }

    /// Publisher `publisher`'s current list, and its pending ones in the
    /// order they take effect.
    #[must_use]
    pub fn lists(&self, publisher: usize) -> (Option<&List>, Vec<&List>) {
        let Some(state) = self.publishers.get(publisher) else {
            return (None, Vec::new());
        };
        (
            state.current.as_ref().map(|kept| &kept.list),
            state.pending.iter().map(|kept| &kept.list).collect(),
        )
    }

    /// Takes what a publisher's site answered at `now_ms`.
    ///
    /// # Errors
    ///
    /// Why no list in it was taken. Of several blobs, the outcome is the
    /// best of theirs.
    pub fn take_list(&mut self, body: &[u8], now_ms: u64) -> Result<Taken, ListError> {
        let message = Message::parse(body)?;
        let publisher = self
            .position(&message.publisher)
            .ok_or(ListError::UnknownPublisher)?;
        let manifest =
            Manifest::from_bytes(&message.manifest).map_err(|_| ListError::BadManifest)?;
        if self.take_publisher_manifest(publisher, manifest)? {
            return Ok(Taken::Revoked);
        }
        // Lowest sequence first, so that a later list waits behind the one
        // in effect rather than making it stale.
        let mut blobs: Vec<_> = (message.blobs.into_iter())
            .map(|signed| (List::parse(&signed.blob), signed))
            .collect();
        blobs.sort_by_key(|(list, _)| list.as_ref().map_or(0, |list| list.sequence));
        let now_s = ledger_seconds(now_ms);
        let mut best = None;
        let mut accepted = false;
        for (list, signed) in blobs {
            let taken = self.take_signed(publisher, list, signed, now_s);
            best = Some(better(best, taken));
            accepted |= taken == Ok(Taken::Current);
            if taken == Ok(Taken::Revoked) {
                break;
            }
        }
        // Whatever was taken, before an error or not.
        self.prune();
        // As rippled's `applyLists` lifts the block on an accepted list
        // once every publisher has one.
        if accepted && (self.publishers.iter()).all(|state| state.in_effect(now_s).is_ok()) {
            self.blocked = false;
        }
        best.unwrap_or(Err(ListError::Malformed))
    }

    /// Takes one blob of `publisher`'s, read as `list`, under the manifest
    /// it carries, if it carries one, or the publisher's newest.
    fn take_signed(
        &mut self,
        publisher: usize,
        list: Result<List, ListError>,
        signed: SignedBlob,
        now_s: u64,
    ) -> Result<Taken, ListError> {
        if let Some(manifest) = &signed.manifest {
            let manifest = Manifest::from_bytes(manifest).map_err(|_| ListError::BadManifest)?;
            if self.take_publisher_manifest(publisher, manifest)? {
                return Ok(Taken::Revoked);
            }
        }
        let list = list?;
        let manifest = (self.publishers.get(publisher))
            .and_then(|state| state.manifest.clone())
            .ok_or(ListError::BadManifest)?;
        let blob = (signed.blob, signed.signature);
        self.take_blob(publisher, blob, manifest, list, now_s, true)
    }

    /// Takes a validator's manifest, as the server's `manifest` method
    /// gave it.
    ///
    /// # Errors
    ///
    /// Why it is not taken. One not newer than the master's kept is no
    /// error: `Ok(false)`.
    pub fn take_manifest(&mut self, bytes: &[u8]) -> Result<bool, ManifestError> {
        let manifest = Manifest::from_bytes(bytes).map_err(|_| ManifestError::Malformed)?;
        self.take_validator_manifest(manifest)
    }

    /// Moves on to `now_ms`, as rippled's `updateTrusted` does at a ledger:
    /// the latest pending list that took effect becomes its publisher's
    /// current one, and those it passed over go; an expired list goes and
    /// UNL-blocks it; then the block is settled.
    pub fn tick(&mut self, now_ms: u64) {
        let now_s = ledger_seconds(now_ms);
        let mut lapsed = false;
        let mut rotated = [false; MAX_PUBLISHERS];
        for (state, rotated) in self.publishers.iter_mut().zip(&mut rotated) {
            *rotated = state.rotate(now_s);
            if state
                .current
                .as_ref()
                .is_some_and(|kept| kept.expired(now_s))
            {
                lapsed = true;
                state.current = None;
            }
            // None that waits has expired, as each expires after it takes
            // effect; one would go all the same.
            state.pending.retain(|kept| !kept.expired(now_s));
        }
        self.prune();
        for (publisher, rotated) in rotated.into_iter().enumerate() {
            if rotated {
                self.take_list_manifests(publisher);
            }
        }
        if lapsed {
            self.blocked = true;
        }
        self.settle(now_s);
    }

    /// The UNL at `now_ms`: the masters at least [`list_threshold`] of the
    /// lists in effect name, less those whose newest manifest revokes them,
    /// in key order.
    ///
    /// # Errors
    ///
    /// Why there is none: [`unavailable_limit`] publishers or more without
    /// a list in effect, UNL-blocked, or no validator.
    pub fn unl(&self, now_ms: u64) -> Result<Vec<PublicKey>, UnlError> {
        let now_s = ledger_seconds(now_ms);
        let mut unavailable = None;
        let mut missing = 0;
        for (publisher, state) in self.publishers.iter().enumerate() {
            if let Err(why) = state.in_effect(now_s) {
                missing += 1;
                unavailable.get_or_insert(UnlError::Unavailable { publisher, why });
            }
        }
        let limit = unavailable_limit(self.publishers.len());
        if let Some(error) = unavailable.filter(|_| missing >= limit) {
            return Err(error);
        }
        let validators = self.trusted_set(now_s);
        if validators.is_empty() {
            return Err(UnlError::Empty);
        }
        if self.blocked(now_ms) {
            return Err(UnlError::Blocked);
        }
        Ok(validators)
    }

    /// The masters at least [`list_threshold`] of the lists in effect at
    /// `now_s` name, less those whose newest manifest revokes them, in key
    /// order: rippled's trusted set, whether or not there is a quorum.
    fn trusted_set(&self, now_s: u64) -> Vec<PublicKey> {
        let mut listings: BTreeMap<PublicKey, usize> = BTreeMap::new();
        for state in &self.publishers {
            if let Ok(kept) = state.in_effect(now_s) {
                for master in kept.list.validators.keys() {
                    *listings.entry(*master).or_default() += 1;
                }
            }
        }
        let threshold = list_threshold(self.publishers.len());
        let revoked =
            |master: &PublicKey| (self.manifests.get(master)).is_some_and(Manifest::is_revocation);
        (listings.into_iter())
            .filter(|(master, lists)| *lists >= threshold && !revoked(master))
            .map(|(master, _)| master)
            .collect()
    }

    /// The key a listed `master` signs with: its newest manifest's, none
    /// if that revoked it, and itself if it has none, as rippled has it.
    #[must_use]
    pub fn signing_key(&self, master: &PublicKey) -> Option<PublicKey> {
        match self.manifests.get(master) {
            Some(manifest) => manifest.signing.map(|(key, _)| key),
            None => self.named(master).then_some(*master),
        }
    }

    /// The listed master that signs with `signing_key`, if one does.
    #[must_use]
    pub fn master_of(&self, signing_key: &PublicKey) -> Option<PublicKey> {
        self.signers.get(signing_key).copied().or_else(|| {
            (!self.manifests.contains_key(signing_key) && self.named(signing_key))
                .then_some(*signing_key)
        })
    }

    /// The publisher whose master key is `key`.
    fn position(&self, key: &PublicKey) -> Option<usize> {
        (self.config.publishers.iter()).position(|publisher| publisher.master_key == *key)
    }

    /// Whether a kept list names `master`, pending or not.
    fn named(&self, master: &PublicKey) -> bool {
        (self.publishers.iter())
            .flat_map(PublisherState::kept)
            .any(|kept| kept.list.validators.contains_key(master))
    }

    /// Whether a list in effect names `master`: a current one, as a tick
    /// moves a pending list in once it takes effect, as rippled's
    /// `keyListings_` has it.
    fn listed(&self, master: &PublicKey) -> bool {
        (self.publishers.iter())
            .filter_map(|state| state.current.as_ref())
            .any(|kept| kept.list.validators.contains_key(master))
    }
}

/// The better of two outcomes of a list: one taken over one not, current
/// over pending over the same, and else the first.
fn better(
    best: Option<Result<Taken, ListError>>,
    taken: Result<Taken, ListError>,
) -> Result<Taken, ListError> {
    let Some(best) = best else {
        return taken;
    };
    let rank = |outcome: &Result<Taken, ListError>| match outcome {
        Ok(Taken::Revoked) => 4,
        Ok(Taken::Current) => 3,
        Ok(Taken::Pending) => 2,
        Ok(Taken::Same) => 1,
        Err(_) => 0,
    };
    if rank(&taken) > rank(&best) {
        taken
    } else {
        best
    }
}

/// Manifests, and lists once read.
impl Unl {
    /// Takes `manifest` of publisher `publisher`: whether it is revoked now.
    /// An older one changes nothing.
    fn take_publisher_manifest(
        &mut self,
        publisher: usize,
        manifest: Manifest,
    ) -> Result<bool, ListError> {
        let master_key = (self.config.publishers.get(publisher))
            .map(|publisher| publisher.master_key)
            .ok_or(ListError::UnknownPublisher)?;
        let state = (self.publishers.get_mut(publisher)).ok_or(ListError::UnknownPublisher)?;
        if state.manifest.as_ref() == Some(&manifest) {
            return Ok(manifest.is_revocation());
        }
        if state.revoked() {
            return Err(ListError::Revoked);
        }
        if (state.manifest.as_ref()).is_some_and(|kept| kept.sequence >= manifest.sequence) {
            return Ok(false);
        }
        if manifest.master_key != master_key || !manifest.verify() {
            return Err(ListError::BadManifest);
        }
        let revoked = manifest.is_revocation();
        state.manifest = Some(manifest);
        if revoked {
            state.current = None;
            state.pending.clear();
            self.prune();
        }
        Ok(revoked)
    }

    /// Takes `list` of `publisher`, signed in `blob` under `manifest`, at
    /// `now_s`: below the highest sequence seen only when `fresh` is not
    /// set, as at a restart. Its manifests are taken if it is in effect,
    /// but not at a restart, which takes those of a list it finds due once
    /// every publisher's lists are back, as a tick does.
    fn take_blob(
        &mut self,
        publisher: usize,
        (blob, signature): (Vec<u8>, Signature),
        manifest: Manifest,
        list: List,
        now_s: u64,
        fresh: bool,
    ) -> Result<Taken, ListError> {
        let Some((signing_key, _)) = manifest.signing else {
            return Err(ListError::Revoked);
        };
        let state = (self.publishers.get_mut(publisher)).ok_or(ListError::UnknownPublisher)?;
        if fresh && state.highest.is_some_and(|highest| list.sequence < highest) {
            return Err(ListError::Stale);
        }
        if state.kept().any(|kept| kept.list.sequence == list.sequence) {
            return Ok(Taken::Same);
        }
        if u64::from(list.expiration) <= now_s {
            return Err(ListError::Expired);
        }
        if !signing_key.verify(&blob, &signature) {
            return Err(ListError::BadSignature);
        }
        let sequence = list.sequence;
        // Stale below the highest sequence seen, the same at a kept one, as
        // a restart takes them in their order.
        assert!(
            state.kept().all(|kept| kept.list.sequence < sequence),
            "a list taken is above every list kept"
        );
        let later = (list.effective).filter(|effective| u64::from(*effective) > now_s);
        // The pending lists that take effect no earlier than this one never
        // would be the latest due: they go, as rippled's `applyLists` drops
        // them.
        let before = later.map(|later| {
            (state.pending.iter())
                .take_while(|kept| kept.list.effective.is_some_and(|at| at < later))
                .count()
        });
        if before.is_some_and(|before| before >= MAX_PENDING) {
            return Err(ListError::TooManyPending);
        }
        let kept = Kept {
            list,
            blob,
            signature,
            manifest,
        };
        let taken = if let Some(before) = before {
            state.pending.truncate(before);
            state.pending.push(kept);
            Taken::Pending
        } else {
            state.current = Some(kept);
            // Every pending list is below it.
            state.pending.clear();
            Taken::Current
        };
        state.highest = Some(
            state
                .highest
                .map_or(sequence, |highest| highest.max(sequence)),
        );
        if taken == Taken::Current && fresh {
            self.take_list_manifests(publisher);
        }
        Ok(taken)
    }

    /// Takes the validators' manifests publisher `publisher`'s current list
    /// carries, as it takes effect, as rippled's `updatePublisherList`
    /// does: a pending list's wait with it.
    fn take_list_manifests(&mut self, publisher: usize) {
        let manifests: Vec<Manifest> = (self.publishers.get(publisher))
            .and_then(|state| state.current.as_ref())
            .map(|kept| kept.list.validators.values().flatten().cloned().collect())
            .unwrap_or_default();
        for manifest in manifests {
            // One the list carries that does not verify is left out, as
            // rippled leaves it: the list counts all the same.
            let _ = self.take_validator_manifest(manifest);
        }
    }

    /// Takes a validator's `manifest`, of a master a list in effect names,
    /// as rippled takes one: whether it is newer than the one kept.
    fn take_validator_manifest(&mut self, manifest: Manifest) -> Result<bool, ManifestError> {
        if !self.listed(&manifest.master_key) {
            return Err(ManifestError::NotListed);
        }
        self.keep_manifest(manifest)
    }

    /// Keeps a validator's `manifest`, of a master a kept list names, the
    /// cheapest checks first: whether it is newer than the one kept. Kept
    /// while a pending list names the master too, as rippled's manifest
    /// cache keeps it, so a list that takes effect later with an older one
    /// never hands its signing back.
    fn keep_manifest(&mut self, manifest: Manifest) -> Result<bool, ManifestError> {
        let master = manifest.master_key;
        if !self.named(&master) {
            return Err(ManifestError::NotListed);
        }
        if (self.manifests.get(&master)).is_some_and(|kept| kept.sequence >= manifest.sequence) {
            return Ok(false);
        }
        if let Some((key, _)) = manifest.signing {
            let other = self.signers.get(&key).is_some_and(|owner| *owner != master);
            if other || self.named(&key) {
                return Err(ManifestError::KeyInUse);
            }
        }
        if !manifest.verify() {
            return Err(ManifestError::BadSignature);
        }
        let signing = manifest.signing;
        if let Some((old, _)) =
            (self.manifests.insert(master, manifest)).and_then(|old| old.signing)
        {
            self.signers.remove(&old);
        }
        if let Some((key, _)) = signing {
            self.signers.insert(key, master);
        }
        Ok(true)
    }

    /// Forgets the manifests of masters no kept list names, and those whose
    /// signing key a list now names as a master.
    fn prune(&mut self) {
        let listed: BTreeSet<PublicKey> = (self.publishers.iter())
            .flat_map(PublisherState::kept)
            .flat_map(|kept| kept.list.validators.keys().copied())
            .collect();
        self.manifests.retain(|master, manifest| {
            listed.contains(master)
                && (manifest.signing).is_none_or(|(key, _)| !listed.contains(&key))
        });
        self.signers = (self.manifests.iter())
            .filter_map(|(master, manifest)| manifest.signing.map(|(key, _)| (key, *master)))
            .collect();
    }
}

/// Per master, what a tally saw of it.
#[derive(Default)]
struct Seen {
    /// Its validation of the ledger was checked.
    checked: bool,
    /// And signed.
    counted: bool,
    /// One of another ledger of the index, not checked yet.
    other: Option<Validation>,
    /// More than one of another ledger of the index came, of one hash or
    /// several: the one checked may be a forgery sent ahead of another.
    others: bool,
    /// One of the ledger signed at a time not fresh, not checked yet.
    stale: Option<Validation>,
}

/// What a tally reads of one validation.
enum Read {
    /// Of the index, by a validator of the UNL, full, fully canonical and
    /// fresh.
    Fresh(PublicKey, Validation),
    /// So, but signed at a time not fresh.
    Stale(PublicKey, Validation),
    /// Counted in the tally for what it is.
    Counted,
}

/// Validations and quorum.
impl Unl {
    /// Counts the UNL's validators at `now_ms` that signed the ledger of
    /// `ledger_hash` and `ledger_index` in `validations`, each checked by
    /// `signatures` (spec/designs/xrpl-validation.md#8-the-verifier).
    ///
    /// # Errors
    ///
    /// Why it cannot count.
    pub fn tally<'a>(
        &self,
        (ledger_hash, ledger_index): ([u8; 32], u32),
        validations: impl IntoIterator<Item = &'a [u8]>,
        now_ms: u64,
        signatures: &dyn ValidationSignatures,
    ) -> Result<Tally, TallyError> {
        let unl: BTreeSet<PublicKey> = self
            .unl(now_ms)
            .map_err(TallyError::Unl)?
            .into_iter()
            .collect();
        let mut tally = Tally {
            quorum: quorum(unl.len(), unl.len()),
            validators: unl.len(),
            ..Tally::default()
        };
        let mut seen: BTreeMap<PublicKey, Seen> = BTreeMap::new();
        for (count, bytes) in validations.into_iter().enumerate() {
            if count >= MAX_VALIDATIONS {
                return Err(TallyError::TooMany);
            }
            let (master, validation) =
                match self.trusted(bytes, ledger_index, &unl, now_ms, &mut tally) {
                    Read::Fresh(master, validation) => (master, validation),
                    Read::Stale(master, validation) => {
                        if validation.ledger_hash == ledger_hash {
                            seen.entry(master)
                                .or_default()
                                .stale
                                .get_or_insert(validation);
                        }
                        continue;
                    }
                    Read::Counted => continue,
                };
            let master = seen.entry(master).or_default();
            if validation.ledger_hash != ledger_hash {
                master.others |= master.other.is_some();
                master.other.get_or_insert(validation);
            } else if !master.checked {
                // One check of a master's validation of the ledger: a
                // server that sends a forgery first only withholds it.
                master.checked = true;
                master.counted = signatures.signed(&validation);
                tally.refused += usize::from(!master.counted);
            }
        }
        for master in seen.values_mut() {
            // One validation of another ledger is checked, so a master
            // costs two checks at most. If it does not check and others
            // came, one of those may be signed: the master is withheld, as
            // if the server had left it out, whatever the order of the
            // answer, but not told as conflicting, which no check showed.
            if let Some(other) = master.other.take().filter(|_| master.counted) {
                if signatures.signed(&other) {
                    master.counted = false;
                    tally.conflicting += 1;
                } else if master.others {
                    master.counted = false;
                }
            }
            assert!(
                !master.counted || master.checked,
                "a master counts only for its checked validation"
            );
            // A stale validation is checked only for a master whose fresh
            // one was not, so a master still costs two checks at most; and
            // counts once, so a server cannot make a few stale validations,
            // or unsigned ones, look like a quorum that signed too late.
            let stale = master.stale.take().filter(|_| !master.checked);
            if stale.is_some_and(|stale| signatures.signed(&stale)) {
                tally.stale += 1;
            }
        }
        let counted = |master: &PublicKey| seen.get(master).is_some_and(|seen| seen.counted);
        tally.missing = unl
            .iter()
            .filter(|master| !counted(master))
            .copied()
            .collect();
        tally.counted = unl.len() - tally.missing.len();
        assert!(
            seen.keys().all(|master| unl.contains(master)),
            "only the UNL's validators count"
        );
        Ok(tally)
    }

    /// What the validation in `bytes` is: of the index and by a validator
    /// of `unl`, full and fully canonical, with its master, fresh or not;
    /// counted in `tally` otherwise. Its signature is not checked.
    fn trusted(
        &self,
        bytes: &[u8],
        ledger_index: u32,
        unl: &BTreeSet<PublicKey>,
        now_ms: u64,
        tally: &mut Tally,
    ) -> Read {
        let Ok(validation) = Validation::from_bytes(bytes) else {
            tally.refused += 1;
            return Read::Counted;
        };
        if validation.ledger_sequence != ledger_index {
            tally.elsewhere += 1;
            return Read::Counted;
        }
        let Some(master) = self.master_of(&validation.signing_key) else {
            tally.unknown += 1;
            return Read::Counted;
        };
        if !unl.contains(&master) {
            tally.untrusted += 1;
            return Read::Counted;
        }
        if validation.flags & (FULL | FULLY_CANONICAL) != FULL | FULLY_CANONICAL {
            tally.refused += 1;
            return Read::Counted;
        }
        if !fresh(validation.signing_time, now_ms) {
            return Read::Stale(master, validation);
        }
        Read::Fresh(master, validation)
    }

    /// The ledger of `header` as an anchor at `now_ms`, if a quorum of the
    /// UNL signed it freshly in `validations` and it is not older than
    /// `kept` (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
    /// Whether one that diverged from `kept` is a reset or a fork is the
    /// chain's to judge.
    ///
    /// # Errors
    ///
    /// Why it is none.
    pub fn anchor<'a>(
        &self,
        header: &LedgerHeader,
        validations: impl IntoIterator<Item = &'a [u8]>,
        now_ms: u64,
        kept: Option<&Anchor>,
        signatures: &dyn ValidationSignatures,
    ) -> Result<Anchored, AnchorError> {
        let anchor = Anchor::of(header);
        let relation = kept.map(|kept| anchor.relation(kept));
        // Before any signature is checked.
        if relation == Some(Relation::Older) {
            return Err(AnchorError::Older);
        }
        let tally = self
            .tally((anchor.hash, anchor.index), validations, now_ms, signatures)
            .map_err(|error| match error {
                TallyError::Unl(error) => AnchorError::Unl(error),
                TallyError::TooMany => AnchorError::TooMany,
            })?;
        if !tally.reached() {
            return Err(AnchorError::NoQuorum(tally));
        }
        Ok(Anchored {
            anchor,
            relation,
            tally,
        })
    }
}

/// Whether a validation signed at `signing_time`, in seconds since
/// 2000-01-01, is fresh at `now_ms`.
fn fresh(signing_time: u32, now_ms: u64) -> bool {
    let signed_ms = (u64::from(signing_time) + RIPPLE_EPOCH_UNIX_S).saturating_mul(1_000);
    signed_ms < now_ms.saturating_add(FRESH_AHEAD_MS)
        && now_ms < signed_ms.saturating_add(FRESH_FOR_MS)
}

/// What is kept with the account's state, and the invariants.
impl Unl {
    /// What it keeps: each publisher's newest manifest, highest sequence
    /// seen, and current and pending lists as they came; and each listed
    /// master's newest manifest, also one a list carries, since the lists
    /// that carry it may not come back at a restart while another that
    /// carries an older one does, which would hand the master's signing
    /// back to a key it rotated away from; and whether it is UNL-blocked,
    /// so that a restart does not lift the block, as it does rippled's.
    #[must_use]
    pub fn state(&self) -> Value {
        let publishers: Vec<Value> = (self.config.publishers.iter())
            .zip(&self.publishers)
            .map(|(publisher, state)| {
                json!({
                    "key": hex_upper(publisher.master_key.as_bytes()),
                    "manifest": state.manifest.as_ref().map(|manifest| hex_upper(&manifest.to_bytes())),
                    "highest": state.highest,
                    "current": state.current.as_ref().map(Kept::state),
                    "pending": state.pending.iter().map(Kept::state).collect::<Vec<_>>(),
                })
            })
            .collect();
        let manifests: Vec<Value> = (self.manifests.values())
            .map(|manifest| hex_upper(&manifest.to_bytes()).into())
            .collect();
        json!({ "publishers": publishers, "manifests": manifests, "blocked": self.blocked })
    }

    /// What [`Self::state`] wrote, under `config` at `now_ms`, every list
    /// and manifest verified again, and moved on to `now_ms` as a tick
    /// would. One that no longer verifies, because the publishers changed,
    /// or that expired, is dropped, to be fetched again; one that was in
    /// effect and expired blocks. The block holds under the same publishers
    /// only, and a state without it, as written before the node kept one,
    /// is not blocked but for what the lists show. A publisher's pending
    /// lists are read as written, and so is the one pending list or none
    /// written before the account kept more. `None` for a state the account
    /// never writes: a publisher twice, more manifests than
    /// [`MAX_MANIFESTS`] or two of one master, a list above its publisher's
    /// highest sequence, more than [`MAX_PENDING`] pending lists, a pending
    /// list not above the current one and the one before it, not taking
    /// effect later than the one before it, or naming no time it takes
    /// effect, or lists of a publisher that revoked its key.
    #[must_use]
    pub fn restore(config: UnlConfig, state: &Value, now_ms: u64) -> Option<Self> {
        let mut unl = Self::new(config);
        let publishers = state.get("publishers")?.as_array()?;
        let manifests = state.get("manifests")?.as_array()?;
        if publishers.len() > MAX_PUBLISHERS || manifests.len() > MAX_MANIFESTS {
            return None;
        }
        let blocked = match state.get("blocked") {
            None => false,
            Some(blocked) => blocked.as_bool()?,
        };
        let mut keys = BTreeSet::new();
        for entry in publishers {
            let key = from_hex(entry.get("key")?.as_str()?)?;
            let key = PublicKey::from_bytes(key.try_into().ok()?)?;
            if !keys.insert(key) {
                return None;
            }
        }
        // The block holds for the publishers it was set under; other
        // publishers start afresh, as rippled does with a new config.
        let configured: BTreeSet<PublicKey> = (unl.config.publishers.iter())
            .map(|publisher| publisher.master_key)
            .collect();
        unl.blocked = blocked && keys == configured;
        let mut rotated = [false; MAX_PUBLISHERS];
        for entry in publishers {
            let key = from_hex(entry.get("key")?.as_str()?)?;
            let key = PublicKey::from_bytes(key.try_into().ok()?)?;
            if let Some(publisher) = unl.position(&key) {
                let moved = unl.restore_publisher(publisher, entry, ledger_seconds(now_ms))?;
                *rotated.get_mut(publisher)? = moved;
            }
        }
        let mut masters = BTreeSet::new();
        for entry in manifests {
            let manifest = Manifest::from_bytes(&from_hex(entry.as_str()?)?).ok()?;
            if !masters.insert(manifest.master_key) {
                return None;
            }
            let _ = unl.keep_manifest(manifest);
        }
        unl.prune();
        // The manifests of a list that took effect since, once every
        // publisher's lists are back, as the tick would have taken them:
        // a key another kept list names as a master stays refused.
        for (publisher, rotated) in rotated.into_iter().enumerate() {
            if rotated {
                unl.take_list_manifests(publisher);
            }
        }
        unl.settle(ledger_seconds(now_ms));
        Some(unl)
    }

    /// Publisher `publisher`'s part of the state, in `entry`: whether a
    /// pending list took effect since, and so is moved in.
    fn restore_publisher(&mut self, publisher: usize, entry: &Value, now_s: u64) -> Option<bool> {
        let highest = match entry.get("highest")? {
            Value::Null => None,
            highest => Some(u32::try_from(highest.as_u64()?).ok()?),
        };
        let manifest = match entry.get("manifest")? {
            Value::Null => None,
            manifest => Some(Manifest::from_bytes(&from_hex(manifest.as_str()?)?).ok()?),
        };
        let mut current = Kept::restore(entry.get("current")?)?;
        let mut pending = Kept::restore_pending(entry.get("pending")?)?;
        let above = |kept: &Kept| highest.is_none_or(|highest| kept.list.sequence > highest);
        let revoked = manifest.as_ref().is_some_and(Manifest::is_revocation);
        let mut lists = current.iter().chain(pending.iter());
        if lists.clone().any(above)
            || !in_order(current.as_ref(), &pending)
            || (revoked && lists.next().is_some())
        {
            return None;
        }
        // Moved on to `now_s`, as the tick at `now_s` would have had the
        // node been running: the latest pending list that took effect is
        // the current one, and those before it go.
        let due = (pending.iter())
            .take_while(|kept| kept.effective_by(now_s))
            .count();
        let rotated = due > 0;
        if rotated {
            current = pending.drain(..due).next_back();
        }
        // A list in effect when it was kept, or since, that expired blocks.
        if current.as_ref().is_some_and(|kept| kept.expired(now_s)) {
            self.blocked = true;
            current = None;
        }
        let master_key = self.config.publishers.get(publisher)?.master_key;
        let state = self.publishers.get_mut(publisher)?;
        state.highest = highest;
        // Its master key still signed it, or it is dropped with its lists.
        state.manifest =
            manifest.filter(|manifest| manifest.master_key == master_key && manifest.verify());
        let Some(newest) = state.manifest.as_ref().map(|manifest| manifest.sequence) else {
            return Some(false);
        };
        for kept in current.into_iter().chain(pending) {
            let signed = kept.manifest.master_key == master_key
                && kept.manifest.sequence <= newest
                && kept.manifest.verify();
            if signed {
                let blob = (kept.blob, kept.signature);
                let _ = self.take_blob(publisher, blob, kept.manifest, kept.list, now_s, false);
            }
        }
        Some(rotated)
    }

    /// Asserts what holds after every change.
    pub fn check_invariants(&self) {
        assert!(self.config.check(), "a config an account can follow");
        assert_eq!(
            self.publishers.len(),
            self.config.publishers.len(),
            "one state for each publisher"
        );
        for (publisher, state) in self.config.publishers.iter().zip(&self.publishers) {
            let newest = state.manifest.as_ref();
            assert!(
                newest.is_none_or(|manifest| manifest.master_key == publisher.master_key),
                "a publisher's manifest is its master's"
            );
            for kept in state.kept() {
                assert!(
                    !state.revoked(),
                    "no list of a publisher that revoked its key"
                );
                assert!(
                    state.highest >= Some(kept.list.sequence),
                    "no list above the highest sequence seen"
                );
                assert!(
                    kept.manifest.master_key == publisher.master_key
                        && newest.is_some_and(|newest| kept.manifest.sequence <= newest.sequence),
                    "a list verified under its publisher's manifest, not past its newest"
                );
                assert!(kept.list.validators.len() <= MAX_LIST_VALIDATORS);
            }
            assert!(
                in_order(state.current.as_ref(), &state.pending),
                "at most MAX_PENDING pending lists, each naming when it takes effect, \
                 above the current one and the one before it, and taking effect later \
                 than the one before it"
            );
        }
        self.check_manifests();
    }

    fn check_manifests(&self) {
        assert!(self.manifests.len() <= MAX_MANIFESTS);
        for (master, manifest) in &self.manifests {
            assert_eq!(
                *master, manifest.master_key,
                "a manifest kept under its master"
            );
            assert!(
                self.named(master),
                "a manifest of a master a kept list names"
            );
            if let Some((key, _)) = manifest.signing {
                assert_eq!(
                    self.signers.get(&key),
                    Some(master),
                    "its signing key is its"
                );
                assert!(
                    !self.named(&key),
                    "no signing key is a kept list's master's"
                );
            }
        }
        let signing = self
            .manifests
            .values()
            .filter(|manifest| manifest.signing.is_some());
        assert_eq!(
            self.signers.len(),
            signing.count(),
            "each signing key once, of a manifest kept"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use alloc::vec;
    use core::cell::Cell;

    use super::*;
    use crate::keys::{Key, Secp256k1Key, Wallet};
    use crate::manifest::REVOKED;
    use crate::validator_list::{Publisher, SignedBlob};

    /// A time well within the lists below: in seconds since 2000-01-01.
    const NOW_S: u32 = 800_000_000;
    const NOW_MS: u64 = (NOW_S as u64 + RIPPLE_EPOCH_UNIX_S) * 1_000;
    const DAY_S: u32 = 86_400;

    /// A publisher of lists: its master key, and its current signing key
    /// with the manifest that hands signing to it.
    struct TestPublisher {
        master: Wallet,
        signing: Wallet,
        manifest: Manifest,
    }

    fn publisher(seed: u8) -> TestPublisher {
        let master = Wallet::from_seed([seed; 16]);
        let signing = Wallet::from_seed([seed.wrapping_add(100); 16]);
        let manifest = Manifest::sign(1, &master, Some(&signing as &dyn Key), None);
        TestPublisher {
            master,
            signing,
            manifest,
        }
    }

    impl TestPublisher {
        fn config(&self) -> Publisher {
            Publisher {
                master_key: self.master.public_key(),
                sites: vec!["https://example.com/vl".into()],
            }
        }

        /// What its site answers, with `lists` signed by its signing key.
        fn body(&self, lists: &[List]) -> Vec<u8> {
            Message {
                publisher: self.master.public_key(),
                manifest: self.manifest.to_bytes(),
                blobs: (lists.iter())
                    .map(|list| SignedBlob::sign(list, &self.signing))
                    .collect(),
            }
            .to_json()
        }
    }

    /// A validator: an Ed25519 master key and a secp256k1 signing key.
    struct TestValidator {
        master: Wallet,
        signing: Secp256k1Key,
        manifest: Manifest,
    }

    fn validator(seed: u8) -> TestValidator {
        let master = Wallet::from_seed([seed; 16]);
        let signing = Secp256k1Key::from_secret(&[seed; 32]).unwrap();
        let manifest = Manifest::sign(1, &master, Some(&signing as &dyn Key), None);
        TestValidator {
            master,
            signing,
            manifest,
        }
    }

    fn validators(seeds: core::ops::Range<u8>) -> Vec<TestValidator> {
        seeds.map(validator).collect()
    }

    impl TestValidator {
        fn key(&self) -> PublicKey {
            self.master.public_key()
        }

        /// Its validation of `hash` at `index`, signed at `signing_time`.
        fn validation(&self, hash: [u8; 32], index: u32, signing_time: u32) -> Vec<u8> {
            Validation {
                flags: FULL,
                ledger_hash: hash,
                ledger_sequence: index,
                close_time: None,
                load_fee: None,
                amendments: None,
                signing_time,
                signing_key: self.signing.public_key(),
                signature: Signature::from_slice(&[1]).unwrap(),
                consensus_hash: None,
                cookie: None,
                validated_hash: None,
                server_version: None,
                base_fee_drops: None,
                reserve_base_drops: None,
                reserve_increment_drops: None,
                gas_limit: None,
                bytecode_size_limit: None,
                gas_price: None,
                other_fields: Vec::new(),
            }
            .signed(&self.signing)
            .to_bytes()
        }
    }

    /// A list of `validators`, with the manifests they hand over.
    fn list(sequence: u32, validators: &[TestValidator]) -> List {
        List {
            sequence,
            expiration: NOW_S + DAY_S,
            effective: None,
            validators: (validators.iter())
                .map(|validator| (validator.key(), Some(validator.manifest.clone())))
                .collect(),
        }
    }

    fn unl(publishers: &[&TestPublisher]) -> Unl {
        Unl::new(UnlConfig {
            publishers: publishers
                .iter()
                .map(|publisher| publisher.config())
                .collect(),
        })
    }

    /// A check of signatures that counts them.
    #[derive(Default)]
    struct Counting(Cell<usize>);

    impl ValidationSignatures for Counting {
        fn signed(&self, validation: &Validation) -> bool {
            self.0.set(self.0.get() + 1);
            Secp256k1Signatures.signed(validation)
        }
    }

    fn tally(unl: &Unl, validations: &[Vec<u8>], now_ms: u64) -> Tally {
        let validations = validations.iter().map(Vec::as_slice);
        unl.tally(([7; 32], 100), validations, now_ms, &Secp256k1Signatures)
            .unwrap()
    }

    // rippled's `calculateQuorum`: four fifths of the effective UNL, never
    // below three fifths of the whole, each rounded up.
    #[test]
    fn quorum_is_rippled_s() {
        for (validators, effective, quorum) in [
            (0, 0, 0),
            (1, 1, 1),
            (4, 4, 4),
            (5, 5, 4),
            (6, 6, 5),
            (10, 10, 8),
            (35, 35, 28),
            (128, 128, 103),
            (512, 512, 410),
            // With validators on a negative UNL, three fifths of the whole
            // is the least.
            (35, 30, 24),
            (35, 26, 21),
            (35, 20, 21),
            (10, 5, 6),
            (10, 0, 6),
            // rippled's `f32` makes three fifths of 25 a hair over 15.
            (25, 15, 16),
            (25, 20, 16),
            (25, 21, 17),
        ] {
            assert_eq!(
                super::quorum(validators, effective),
                quorum,
                "{validators}, {effective}"
            );
        }
    }

    // rippled's default `validator_list_threshold`, and how many publishers
    // without a list leave no quorum.
    #[test]
    fn the_threshold_and_the_unavailable_limit_are_rippled_s() {
        for (publishers, threshold, limit) in [
            (1, 1, 1),
            (2, 1, 1),
            (3, 2, 2),
            (4, 3, 2),
            (5, 3, 3),
            (6, 4, 3),
        ] {
            assert_eq!(list_threshold(publishers), threshold, "{publishers}");
            assert_eq!(unavailable_limit(publishers), limit, "{publishers}");
        }
    }

    // spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: a list
    // counts under its publisher's signing key, at or above the highest
    // sequence seen, until it expires.
    #[test]
    fn a_list_counts_when_its_publisher_signed_it_and_it_has_not_expired() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        assert_eq!(
            unl.unl(NOW_MS),
            Err(UnlError::Unavailable {
                publisher: 0,
                why: Unavailable::NoList
            })
        );
        let body = ours.body(&[list(5, &validators)]);
        assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Current));
        unl.check_invariants();
        assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Same));
        let mut keys: Vec<PublicKey> = validators.iter().map(TestValidator::key).collect();
        keys.sort();
        assert_eq!(unl.unl(NOW_MS).unwrap(), keys);
        for validator in &validators {
            assert_eq!(
                unl.signing_key(&validator.key()),
                Some(validator.signing.public_key())
            );
            assert_eq!(
                unl.master_of(&validator.signing.public_key()),
                Some(validator.key())
            );
        }
        // Below the highest sequence: stale, also once it was replaced.
        let older = ours.body(&[list(4, &validators)]);
        assert_eq!(unl.take_list(&older, NOW_MS), Err(ListError::Stale));
        assert_eq!(
            unl.take_list(&ours.body(&[list(6, &validators)]), NOW_MS),
            Ok(Taken::Current)
        );
        assert_eq!(
            unl.take_list(&ours.body(&[list(5, &validators)]), NOW_MS),
            Err(ListError::Stale)
        );
        // Expired, and another key's signature, are not taken.
        let mut expired = list(7, &validators);
        expired.expiration = NOW_S;
        assert_eq!(
            unl.take_list(&ours.body(&[expired]), NOW_MS),
            Err(ListError::Expired)
        );
        let mut forged = Message::parse(&ours.body(&[list(8, &validators)])).unwrap();
        forged.blobs[0].signature = ours.master.sign(&forged.blobs[0].blob);
        assert_eq!(
            unl.take_list(&forged.to_json(), NOW_MS),
            Err(ListError::BadSignature)
        );
        let mut altered = Message::parse(&ours.body(&[list(8, &validators)])).unwrap();
        altered.blobs[0].blob = list(8, &validators[..4]).to_blob();
        assert_eq!(
            unl.take_list(&altered.to_json(), NOW_MS),
            Err(ListError::BadSignature)
        );
        assert_eq!(unl.lists(0).0.unwrap().sequence, 6);
        unl.check_invariants();
        // The list goes when it expires, and with it the UNL.
        let expiry_ms = NOW_MS + u64::from(DAY_S) * 1_000;
        assert_eq!(
            unl.unl(expiry_ms),
            Err(UnlError::Unavailable {
                publisher: 0,
                why: Unavailable::Expired
            })
        );
        unl.tick(expiry_ms);
        unl.check_invariants();
        assert_eq!(unl.lists(0), (None, vec![]));
        assert!(unl.manifests.is_empty());
    }

    #[test]
    fn only_the_configured_publishers_count() {
        let ours = publisher(1);
        let theirs = publisher(2);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        assert_eq!(
            unl.take_list(&theirs.body(&[list(1, &validators)]), NOW_MS),
            Err(ListError::UnknownPublisher)
        );
        // Our key named, their manifest.
        let mut message = Message::parse(&ours.body(&[list(1, &validators)])).unwrap();
        message.manifest = theirs.manifest.to_bytes();
        assert_eq!(
            unl.take_list(&message.to_json(), NOW_MS),
            Err(ListError::BadManifest)
        );
        // A manifest of our key with its master signature broken.
        let mut broken = ours.manifest.clone();
        broken.master_signature = theirs.master.sign(b"other");
        message.manifest = broken.to_bytes();
        assert_eq!(
            unl.take_list(&message.to_json(), NOW_MS),
            Err(ListError::BadManifest)
        );
        unl.check_invariants();
        assert_eq!(unl.lists(0), (None, vec![]));
    }

    // A publisher rotates its signing key with a newer manifest, which
    // carries over to lists after it; and revokes its key for good.
    #[test]
    fn publishers_rotate_their_keys_and_revoke_them() {
        let mut ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        assert_eq!(
            unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS),
            Ok(Taken::Current)
        );
        let first_manifest = ours.manifest.clone();
        ours.signing = Wallet::from_seed([50; 16]);
        ours.manifest = Manifest::sign(2, &ours.master, Some(&ours.signing as &dyn Key), None);
        assert_eq!(
            unl.take_list(&ours.body(&[list(2, &validators)]), NOW_MS),
            Ok(Taken::Current)
        );
        // A list signed by the old key, under the old manifest, is not.
        let old = TestPublisher {
            master: Wallet::from_seed([1; 16]),
            signing: Wallet::from_seed([101; 16]),
            manifest: first_manifest,
        };
        assert_eq!(
            unl.take_list(&old.body(&[list(3, &validators)]), NOW_MS),
            Err(ListError::BadSignature)
        );
        unl.check_invariants();
        ours.manifest = Manifest::sign(REVOKED, &ours.master, None, None);
        assert_eq!(
            unl.take_list(&ours.body(&[list(4, &validators)]), NOW_MS),
            Ok(Taken::Revoked)
        );
        unl.check_invariants();
        assert_eq!(unl.lists(0), (None, vec![]));
        assert_eq!(
            unl.unl(NOW_MS),
            Err(UnlError::Unavailable {
                publisher: 0,
                why: Unavailable::Revoked
            })
        );
        assert_eq!(
            unl.take_list(&old.body(&[list(5, &validators)]), NOW_MS),
            Err(ListError::Revoked)
        );
        assert!(unl.manifests.is_empty() && unl.signers.is_empty());
    }

    // A list that takes effect later waits as pending, in either order of a
    // version 2 answer, and replaces the current one once it is due.
    #[test]
    fn a_list_that_takes_effect_later_waits_until_then() {
        let ours = publisher(1);
        let validators = validators(10..16);
        let mut unl = unl(&[&ours]);
        let current = list(1, &validators[..5]);
        let mut later = list(2, &validators[1..]);
        later.effective = Some(NOW_S + 60);
        let body = ours.body(&[later.clone(), current.clone()]);
        assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Current));
        unl.check_invariants();
        assert_eq!(unl.lists(0), (Some(&current), vec![&later]));
        assert!(!unl.unl(NOW_MS).unwrap().contains(&validators[5].key()));
        let due = NOW_MS + 60_000;
        assert!(unl.unl(due).unwrap().contains(&validators[5].key()));
        unl.tick(due - 1_000);
        assert_eq!(unl.lists(0).1, vec![&later]);
        unl.tick(due);
        unl.check_invariants();
        assert_eq!(unl.lists(0), (Some(&later), vec![]));
        // The first validator is listed no more: its manifest goes.
        assert_eq!(unl.signing_key(&validators[0].key()), None);
        assert_eq!(unl.master_of(&validators[0].signing.public_key()), None);
        // A newer list in effect now replaces a pending one below it.
        let mut pending = list(3, &validators);
        pending.effective = Some(NOW_S + 600);
        assert_eq!(
            unl.take_list(&ours.body(&[pending]), due),
            Ok(Taken::Pending)
        );
        assert_eq!(
            unl.take_list(&ours.body(&[list(4, &validators)]), due),
            Ok(Taken::Current)
        );
        assert!(unl.lists(0).1.is_empty());
        unl.check_invariants();
        // A blob taken before another fails stays taken, and what it no
        // longer lists goes.
        let mut message =
            Message::parse(&ours.body(&[list(5, &validators[1..]), list(6, &validators)])).unwrap();
        message.blobs[1].manifest = Some(b"not one".to_vec());
        assert_eq!(unl.take_list(&message.to_json(), due), Ok(Taken::Current));
        unl.check_invariants();
        assert_eq!(unl.lists(0).0.unwrap().sequence, 5);
        assert_eq!(unl.signing_key(&validators[0].key()), None);
    }

    // Two lists that take effect later, in one answer: each is in effect
    // from its time, as rippled's `updateTrusted` rotates in the latest one
    // due, and passes over one that came due with a later one.
    #[test]
    fn the_next_future_list_is_the_one_in_effect() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        let mut next = list(2, &validators[..3]);
        next.effective = Some(NOW_S + 60);
        let mut later = list(3, &validators);
        later.effective = Some(NOW_S + 120);
        let body = ours.body(&[list(1, &validators), next.clone(), later.clone()]);
        assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Current));
        unl.check_invariants();
        assert_eq!(unl.lists(0).1, vec![&next, &later]);
        assert_eq!(unl.unl(NOW_MS).unwrap().len(), 5);
        let at_next = NOW_MS + 60_000;
        assert_eq!(
            unl.unl(at_next).unwrap().len(),
            3,
            "sequence 2 is in effect; sequence 3 is not yet"
        );
        let mut ticked = unl.clone();
        ticked.tick(at_next);
        ticked.check_invariants();
        assert_eq!(ticked.lists(0), (Some(&next), vec![&later]));
        assert_eq!(ticked.unl(at_next).unwrap().len(), 3);
        // The same answer again changes nothing.
        assert_eq!(ticked.take_list(&body, at_next), Ok(Taken::Same));
        assert_eq!(ticked.lists(0), (Some(&next), vec![&later]));
        let at_later = NOW_MS + 120_000;
        assert_eq!(ticked.unl(at_later).unwrap().len(), 5);
        ticked.tick(at_later);
        ticked.check_invariants();
        assert_eq!(ticked.lists(0), (Some(&later), vec![]));
        // Not ticked between, the later one is in effect, and the one it
        // passed over goes.
        assert_eq!(unl.unl(at_later).unwrap().len(), 5);
        unl.tick(at_later);
        unl.check_invariants();
        assert_eq!(unl, ticked);
    }

    // A list that takes effect no later than a pending one below it would
    // never be the latest due: the one below goes, as rippled's
    // `applyLists` drops it.
    #[test]
    fn a_later_list_that_takes_effect_sooner_replaces_those_below_it() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        let mut first = list(2, &validators[..3]);
        first.effective = Some(NOW_S + 60);
        let mut second = list(3, &validators[..4]);
        second.effective = Some(NOW_S + 600);
        let body = ours.body(&[list(1, &validators), first.clone(), second]);
        assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Current));
        let mut sooner = list(4, &validators[1..]);
        sooner.effective = Some(NOW_S + 300);
        assert_eq!(
            unl.take_list(&ours.body(&[sooner.clone()]), NOW_MS),
            Ok(Taken::Pending)
        );
        unl.check_invariants();
        assert_eq!(unl.lists(0).1, vec![&first, &sooner]);
        // At the same time, too.
        let mut same = list(5, &validators[2..]);
        same.effective = Some(NOW_S + 300);
        assert_eq!(
            unl.take_list(&ours.body(&[same.clone()]), NOW_MS),
            Ok(Taken::Pending)
        );
        unl.check_invariants();
        assert_eq!(unl.lists(0).1, vec![&first, &same]);
    }

    // A publisher keeps at most `MAX_PENDING` lists waiting: one more that
    // waits is not taken, nor counted as seen, until one of them is in
    // effect.
    #[test]
    fn the_pending_lists_are_bounded() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        let waiting = |sequence: u32| {
            let mut later = list(sequence, &validators);
            later.effective = Some(NOW_S + 60 * sequence);
            later
        };
        assert_eq!(
            unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS),
            Ok(Taken::Current)
        );
        let lists: Vec<List> = (2..).map(waiting).take(MAX_PENDING).collect();
        assert_eq!(
            unl.take_list(&ours.body(&lists), NOW_MS),
            Ok(Taken::Pending)
        );
        unl.check_invariants();
        assert_eq!(unl.lists(0).1.len(), MAX_PENDING);
        let sequence = u32::try_from(MAX_PENDING).unwrap() + 2;
        let one_more = ours.body(&[waiting(sequence)]);
        assert_eq!(
            unl.take_list(&one_more, NOW_MS),
            Err(ListError::TooManyPending)
        );
        unl.check_invariants();
        assert_eq!(unl.lists(0).1.len(), MAX_PENDING);
        assert_eq!(unl.publishers[0].highest, Some(sequence - 1));
        let first_due = NOW_MS + 120_000;
        unl.tick(first_due);
        assert_eq!(unl.take_list(&one_more, first_due), Ok(Taken::Pending));
        unl.check_invariants();
        assert_eq!(unl.lists(0).0.unwrap().sequence, 2);
        assert_eq!(unl.lists(0).1.len(), MAX_PENDING);
    }

    // A manifest carried by a list that is not yet in effect waits with it,
    // as rippled's `updatePublisherList` takes it only then.
    #[test]
    fn a_pending_manifest_waits_until_the_list_is_in_effect() {
        let ours = publisher(1);
        let mut validators = validators(10..12);
        let mut unl = unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let old = validators[0].signing.public_key();
        assert_eq!(unl.signing_key(&validators[0].key()), Some(old));
        let signing = Secp256k1Key::from_secret(&[9; 32]).unwrap();
        validators[0].manifest =
            Manifest::sign(2, &validators[0].master, Some(&signing as &dyn Key), None);
        validators[0].signing = signing;
        let mut pending = list(2, &validators);
        pending.effective = Some(NOW_S + 60);
        assert_eq!(
            unl.take_list(&ours.body(&[pending]), NOW_MS),
            Ok(Taken::Pending)
        );
        unl.check_invariants();
        assert_eq!(unl.signing_key(&validators[0].key()), Some(old));
        assert_eq!(unl.master_of(&old), Some(validators[0].key()));
        unl.tick(NOW_MS + 60_000);
        unl.check_invariants();
        assert_eq!(
            unl.signing_key(&validators[0].key()),
            Some(validators[0].signing.public_key())
        );
        assert_eq!(unl.master_of(&old), None);
    }

    // A validator only a pending list names has no manifest taken, from
    // the list or the server, until the list is in effect; the latest list
    // rotated in brings its own, and one a restart finds due does too.
    #[test]
    fn manifests_come_with_the_list_rotated_in() {
        let ours = publisher(1);
        let validators = validators(10..14);
        let mut unl = unl(&[&ours]);
        let newcomer = &validators[3];
        let mut next = list(2, &validators[..3]);
        next.effective = Some(NOW_S + 60);
        let mut later = list(3, &validators);
        later.effective = Some(NOW_S + 120);
        let body = ours.body(&[list(1, &validators[..3]), next, later]);
        unl.take_list(&body, NOW_MS).unwrap();
        unl.check_invariants();
        assert!(!unl.manifests.contains_key(&newcomer.key()));
        assert_eq!(
            unl.take_manifest(&newcomer.manifest.to_bytes()),
            Err(ManifestError::NotListed)
        );
        let state = unl.state();
        let at_ms = NOW_MS + 120_000;
        let mut ticked = unl.clone();
        ticked.tick(at_ms);
        ticked.check_invariants();
        assert_eq!(ticked.lists(0).0.unwrap().sequence, 3);
        assert_eq!(
            ticked.signing_key(&newcomer.key()),
            Some(newcomer.signing.public_key())
        );
        let restored = Unl::restore(unl.config.clone(), &state, at_ms).unwrap();
        restored.check_invariants();
        assert_eq!(restored, ticked);
        // Once it is in effect, the server's newer manifest counts too.
        let rotated = Secp256k1Key::from_secret(&[61; 32]).unwrap();
        let newer = Manifest::sign(2, &newcomer.master, Some(&rotated as &dyn Key), None);
        assert_eq!(ticked.take_manifest(&newer.to_bytes()), Ok(true));
        assert_eq!(
            ticked.signing_key(&newcomer.key()),
            Some(rotated.public_key())
        );
    }

    // A master's newer manifest stays while only a pending list names it,
    // so the list, once in effect, does not hand its signing back to the
    // older key it carries.
    #[test]
    fn a_pending_list_never_takes_a_master_back_to_an_older_key() {
        let ours = publisher(1);
        let validators = validators(10..14);
        let mut unl = unl(&[&ours]);
        let first = &validators[0];
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let rotated = Secp256k1Key::from_secret(&[60; 32]).unwrap();
        let newer = Manifest::sign(2, &first.master, Some(&rotated as &dyn Key), None);
        assert_eq!(unl.take_manifest(&newer.to_bytes()), Ok(true));
        // A list in effect at once without it, and one later with it again.
        let mut back = list(3, &validators);
        back.effective = Some(NOW_S + 60);
        let body = ours.body(&[list(2, &validators[1..]), back]);
        unl.take_list(&body, NOW_MS).unwrap();
        unl.check_invariants();
        let state = unl.state();
        let waiting = Unl::restore(unl.config.clone(), &state, NOW_MS).unwrap();
        waiting.check_invariants();
        assert_eq!(waiting, unl);
        let at_ms = NOW_MS + 60_000;
        unl.tick(at_ms);
        unl.check_invariants();
        assert_eq!(unl.signing_key(&first.key()), Some(rotated.public_key()));
        let restored = Unl::restore(unl.config.clone(), &state, at_ms).unwrap();
        assert_eq!(restored, unl);
    }

    // A list rotated in carries a manifest that hands a master's signing to
    // a key a list still waiting names as a master: refused, at a tick and
    // at a restart alike, and the master keeps its older manifest.
    #[test]
    fn a_restart_takes_a_rotated_list_s_manifests_as_a_tick_does() {
        let ours = publisher(1);
        let validators = validators(10..14);
        let (master, waiting) = (&validators[0], &validators[3]);
        let mut unl = unl(&[&ours]);
        let handed = Manifest::sign(2, &master.master, Some(&waiting.master as &dyn Key), None);
        let mut next = list(2, &validators[..3]);
        next.effective = Some(NOW_S + 60);
        next.validators.insert(master.key(), Some(handed));
        let mut later = list(3, &validators);
        later.effective = Some(NOW_S + 120);
        let body = ours.body(&[list(1, &validators[..3]), next, later]);
        unl.take_list(&body, NOW_MS).unwrap();
        let state = unl.state();
        let at_ms = NOW_MS + 60_000;
        unl.tick(at_ms);
        unl.check_invariants();
        assert_eq!(unl.lists(0).0.unwrap().sequence, 2);
        assert_eq!(
            unl.signing_key(&master.key()),
            Some(master.signing.public_key())
        );
        let restored = Unl::restore(unl.config.clone(), &state, at_ms).unwrap();
        restored.check_invariants();
        assert_eq!(restored, unl);
    }

    fn keys(validators: &[&TestValidator]) -> Vec<PublicKey> {
        let mut keys: Vec<PublicKey> = validators.iter().map(|validator| validator.key()).collect();
        keys.sort();
        keys
    }

    // spec/decisions/verified-ledgers.md: with two publishers, as mainnet
    // has, the UNL is the union of their lists, and either one without a
    // list in effect leaves no quorum.
    #[test]
    fn two_publishers_give_the_union() {
        let first = publisher(1);
        let second = publisher(2);
        let validators = validators(10..18);
        let mut unl = unl(&[&first, &second]);
        assert_eq!(
            unl.take_list(&first.body(&[list(1, &validators[..7])]), NOW_MS),
            Ok(Taken::Current)
        );
        assert_eq!(
            unl.unl(NOW_MS),
            Err(UnlError::Unavailable {
                publisher: 1,
                why: Unavailable::NoList
            })
        );
        let mut short = list(1, &validators[1..]);
        short.expiration = NOW_S + 600;
        assert_eq!(
            unl.take_list(&second.body(&[short]), NOW_MS),
            Ok(Taken::Current)
        );
        unl.check_invariants();
        let all: Vec<&TestValidator> = validators.iter().collect();
        assert_eq!(unl.unl(NOW_MS).unwrap(), keys(&all));
        // A validator only one publisher lists counts: the one only the
        // first lists, or the one only the second lists, makes the quorum
        // with the six both list.
        let validations = |signers: &[TestValidator]| -> Vec<Vec<u8>> {
            (signers.iter())
                .map(|validator| validator.validation([7; 32], 100, NOW_S))
                .collect()
        };
        let both_list = tally(&unl, &validations(&validators[1..7]), NOW_MS);
        assert_eq!(
            (both_list.counted, both_list.quorum, both_list.validators),
            (6, 7, 8)
        );
        assert!(!both_list.reached());
        let with_first_s = tally(&unl, &validations(&validators[..7]), NOW_MS);
        assert_eq!((with_first_s.counted, with_first_s.untrusted), (7, 0));
        assert!(with_first_s.reached());
        let with_second_s = tally(&unl, &validations(&validators[1..]), NOW_MS);
        assert_eq!((with_second_s.counted, with_second_s.untrusted), (7, 0));
        assert!(with_second_s.reached());
        // The second's list expires: no quorum, though the first's still
        // names validators.
        assert_eq!(
            unl.unl(NOW_MS + 600_000),
            Err(UnlError::Unavailable {
                publisher: 1,
                why: Unavailable::Expired
            })
        );
        // Lists that name no validator give no UNL.
        assert_eq!(
            unl.take_list(&first.body(&[list(2, &[])]), NOW_MS),
            Ok(Taken::Current)
        );
        assert_eq!(
            unl.take_list(&second.body(&[list(2, &[])]), NOW_MS),
            Ok(Taken::Current)
        );
        assert_eq!(unl.unl(NOW_MS), Err(UnlError::Empty));
    }

    // With three publishers or more, a validator needs a majority of the
    // lists, and two publishers without a list leave no quorum.
    #[test]
    fn more_publishers_take_a_majority() {
        let publishers = [publisher(1), publisher(2), publisher(3), publisher(4)];
        let v = validators(10..16);
        let bodies = [
            publishers[0].body(&[list(1, &v[..5])]),
            publishers[1].body(&[list(1, &v[1..])]),
            // The same validators as v[0], v[2] and v[3].
            publishers[2].body(&[list(1, &[validator(10), validator(12), validator(13)])]),
            // As v[2] and v[3].
            publishers[3].body(&[list(1, &[validator(12), validator(13)])]),
        ];
        // Three publishers: a validator needs two lists, what one lists
        // alone is not on it.
        let mut three = unl(&[&publishers[0], &publishers[1], &publishers[2]]);
        three.take_list(&bodies[0], NOW_MS).unwrap();
        assert_eq!(
            three.unl(NOW_MS),
            Err(UnlError::Unavailable {
                publisher: 1,
                why: Unavailable::NoList
            })
        );
        three.take_list(&bodies[1], NOW_MS).unwrap();
        three.take_list(&bodies[2], NOW_MS).unwrap();
        three.check_invariants();
        assert_eq!(
            three.unl(NOW_MS).unwrap(),
            keys(&[&v[0], &v[1], &v[2], &v[3], &v[4]])
        );
        // Four publishers: a validator needs three lists.
        let mut four = unl(&publishers.each_ref());
        four.take_list(&bodies[0], NOW_MS).unwrap();
        four.take_list(&bodies[1], NOW_MS).unwrap();
        assert_eq!(
            four.unl(NOW_MS),
            Err(UnlError::Unavailable {
                publisher: 2,
                why: Unavailable::NoList
            })
        );
        four.take_list(&bodies[2], NOW_MS).unwrap();
        four.take_list(&bodies[3], NOW_MS).unwrap();
        assert_eq!(four.unl(NOW_MS).unwrap(), keys(&[&v[2], &v[3]]));
    }

    /// Three publishers whose lists are all in effect: A lists v[..5], B
    /// v[1..], and C all six, until `NOW_S + 600`.
    fn three_publishers() -> (Unl, [TestPublisher; 3], Vec<TestValidator>) {
        let publishers = [publisher(1), publisher(2), publisher(3)];
        let v = validators(10..16);
        let mut unl = unl(&publishers.each_ref());
        let mut short = list(1, &v);
        short.expiration = NOW_S + 600;
        for body in [
            publishers[0].body(&[list(1, &v[..5])]),
            publishers[1].body(&[list(1, &v[1..])]),
            publishers[2].body(&[short]),
        ] {
            assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Current));
        }
        let all: Vec<&TestValidator> = v.iter().collect();
        assert_eq!(unl.unl(NOW_MS).unwrap(), keys(&all));
        (unl, publishers, v)
    }

    // rippled's UNL-blocked state: once a list in effect expires, there is
    // no UNL until every publisher has a list in effect again, however few
    // the threshold needs. So with C's list expired, A alone cannot shrink
    // the UNL to the one validator it shares with B.
    #[test]
    fn an_expired_list_blocks_until_every_publisher_has_one() {
        let (mut unl, [a, _, c], v) = three_publishers();
        let lapsed = NOW_MS + 600_000;
        assert_eq!(unl.unl(lapsed), Err(UnlError::Blocked));
        unl.tick(lapsed);
        unl.check_invariants();
        assert_eq!(unl.unl(lapsed), Err(UnlError::Blocked));
        assert_eq!(
            unl.take_list(&a.body(&[list(2, &v[1..2])]), lapsed),
            Ok(Taken::Current)
        );
        assert_eq!(unl.unl(lapsed), Err(UnlError::Blocked));
        // A restart keeps the block.
        let restored = Unl::restore(unl.config.clone(), &unl.state(), lapsed).unwrap();
        assert_eq!(restored, unl);
        assert_eq!(restored.unl(lapsed), Err(UnlError::Blocked));
        // C's next list lifts it: two of three lists name each validator.
        assert_eq!(
            unl.take_list(&c.body(&[list(2, &v)]), lapsed),
            Ok(Taken::Current)
        );
        let left: Vec<&TestValidator> = v[1..].iter().collect();
        assert_eq!(unl.unl(lapsed).unwrap(), keys(&left));
        // Expired and then revoked, C never has a list again: blocked for
        // good, under these publishers.
        let (mut unl, [_, _, mut c], v) = three_publishers();
        unl.tick(lapsed);
        c.manifest = Manifest::sign(REVOKED, &c.master, None, None);
        assert_eq!(
            unl.take_list(&c.body(&[list(3, &v)]), lapsed),
            Ok(Taken::Revoked)
        );
        unl.tick(lapsed);
        assert_eq!(unl.unl(lapsed), Err(UnlError::Blocked));
    }

    // A publisher never fetched keeps the block that the empty UNL of a
    // fresh start sets, as in rippled, until its list comes.
    #[test]
    fn a_publisher_never_fetched_keeps_the_start_blocked() {
        let publishers = [publisher(1), publisher(2), publisher(3)];
        let v = validators(10..16);
        let mut unl = unl(&publishers.each_ref());
        assert!(unl.blocked(NOW_MS));
        unl.take_list(&publishers[0].body(&[list(1, &v[..5])]), NOW_MS)
            .unwrap();
        unl.take_list(&publishers[1].body(&[list(1, &v[1..])]), NOW_MS)
            .unwrap();
        unl.tick(NOW_MS);
        assert_eq!(unl.unl(NOW_MS), Err(UnlError::Blocked));
        let restored = Unl::restore(unl.config.clone(), &unl.state(), NOW_MS).unwrap();
        assert_eq!(restored.unl(NOW_MS), Err(UnlError::Blocked));
        unl.take_list(&publishers[2].body(&[list(1, &v)]), NOW_MS)
            .unwrap();
        let all: Vec<&TestValidator> = v.iter().collect();
        assert_eq!(unl.unl(NOW_MS).unwrap(), keys(&all));
        // A state kept before the node kept the block, with C's list never
        // fetched, starts unblocked, as rippled restarts with the lists it
        // cached.
        let mut kept = restored.state();
        kept.as_object_mut().unwrap().remove("blocked");
        let restarted = Unl::restore(unl.config.clone(), &kept, NOW_MS).unwrap();
        assert_eq!(
            restarted.unl(NOW_MS).unwrap(),
            keys(&[&v[1], &v[2], &v[3], &v[4]])
        );
    }

    // A publisher that revokes its key leaves no list, but sets no block,
    // as in rippled: the threshold then needs both other lists, and either
    // of those publishers alone can shrink the UNL to what they share.
    // This is rippled's exposure too, and only configs of three or four
    // publishers, which only an owner names, meet it.
    #[test]
    fn a_revoked_publisher_sets_no_block() {
        let (mut unl, [a, _, mut c], v) = three_publishers();
        c.manifest = Manifest::sign(REVOKED, &c.master, None, None);
        assert_eq!(
            unl.take_list(&c.body(&[list(2, &v)]), NOW_MS),
            Ok(Taken::Revoked)
        );
        unl.tick(NOW_MS);
        unl.check_invariants();
        assert_eq!(
            unl.unl(NOW_MS).unwrap(),
            keys(&[&v[1], &v[2], &v[3], &v[4]])
        );
        assert_eq!(
            unl.take_list(&a.body(&[list(2, &v[1..2])]), NOW_MS),
            Ok(Taken::Current)
        );
        assert_eq!(unl.unl(NOW_MS).unwrap(), keys(&[&v[1]]));
    }

    // Validators rotate their keys with manifests the lists lag behind:
    // the node takes newer ones from the server, when the listed master
    // signed them.
    #[test]
    fn newer_manifests_of_listed_masters_count() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        assert_eq!(
            unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS),
            Ok(Taken::Current)
        );
        let rotated = Secp256k1Key::from_secret(&[60; 32]).unwrap();
        let first = &validators[0];
        let newer = Manifest::sign(2, &first.master, Some(&rotated as &dyn Key), None);
        assert_eq!(unl.take_manifest(&newer.to_bytes()), Ok(true));
        assert_eq!(unl.take_manifest(&newer.to_bytes()), Ok(false));
        assert_eq!(unl.take_manifest(&first.manifest.to_bytes()), Ok(false));
        unl.check_invariants();
        assert_eq!(unl.signing_key(&first.key()), Some(rotated.public_key()));
        assert_eq!(unl.master_of(&first.signing.public_key()), None);
        assert_eq!(unl.master_of(&rotated.public_key()), Some(first.key()));
        // A list that carries the older one leaves the newer.
        assert_eq!(
            unl.take_list(&ours.body(&[list(2, &validators)]), NOW_MS),
            Ok(Taken::Current)
        );
        assert_eq!(unl.signing_key(&first.key()), Some(rotated.public_key()));
        // Of a master not listed, signed by another, or with a key in use.
        let stranger = validator(30);
        assert_eq!(
            unl.take_manifest(&stranger.manifest.to_bytes()),
            Err(ManifestError::NotListed)
        );
        let mut forged = Manifest::sign(3, &first.master, Some(&rotated as &dyn Key), None);
        forged.master_signature = stranger.master.sign(&forged.signing_bytes());
        assert_eq!(
            unl.take_manifest(&forged.to_bytes()),
            Err(ManifestError::BadSignature)
        );
        let second = &validators[1];
        let in_use = Manifest::sign(2, &second.master, Some(&rotated as &dyn Key), None);
        assert_eq!(
            unl.take_manifest(&in_use.to_bytes()),
            Err(ManifestError::KeyInUse)
        );
        let a_master = &validators[2].master as &dyn Key;
        let master_key = Manifest::sign(2, &second.master, Some(a_master), None);
        assert_eq!(
            unl.take_manifest(&master_key.to_bytes()),
            Err(ManifestError::KeyInUse)
        );
        assert_eq!(unl.take_manifest(b"not one"), Err(ManifestError::Malformed));
        // A revocation leaves the master no key, for good.
        let revocation = Manifest::sign(REVOKED, &first.master, None, None);
        assert_eq!(unl.take_manifest(&revocation.to_bytes()), Ok(true));
        assert_eq!(unl.signing_key(&first.key()), None);
        assert_eq!(unl.master_of(&rotated.public_key()), None);
        assert_eq!(unl.take_manifest(&newer.to_bytes()), Ok(false));
        unl.check_invariants();
        // And leaves the UNL, as rippled trusts no revoked master: the
        // quorum is of the four left.
        let left: Vec<&TestValidator> = validators[1..].iter().collect();
        assert_eq!(unl.unl(NOW_MS).unwrap(), keys(&left));
        let validations: Vec<Vec<u8>> = (validators[1..].iter())
            .map(|validator| validator.validation([7; 32], 100, NOW_S))
            .collect();
        let tally = tally(&unl, &validations, NOW_MS);
        assert_eq!((tally.counted, tally.quorum, tally.validators), (4, 4, 4));
    }

    // spec/designs/xrpl-validation.md#8-the-verifier: a validation counts
    // only by a listed master's current key, full, fully canonical, fresh
    // and signed; a master counts once, and not at all if it signed two
    // ledgers of the index.
    #[test]
    fn a_quorum_of_fresh_signatures_validates() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let sign = |validator: &TestValidator| validator.validation([7; 32], 100, NOW_S);
        let mut validations: Vec<Vec<u8>> = validators[..4].iter().map(sign).collect();
        let reached = tally(&unl, &validations, NOW_MS);
        assert!(reached.reached());
        assert_eq!(
            (reached.counted, reached.quorum, reached.validators),
            (4, 4, 5)
        );
        assert_eq!(reached.missing, vec![validators[4].key()]);
        // Twice the same counts once.
        validations[3] = validations[0].clone();
        let short = tally(&unl, &validations, NOW_MS);
        assert_eq!(short.counted, 3);
        assert!(!short.reached());
        // Partial, of another index, by a stranger, unsigned, unread.
        let mut partial = Validation::from_bytes(&validations[1]).unwrap();
        partial.flags = 0;
        let partial = partial.signed(&validators[1].signing).to_bytes();
        let elsewhere = validators[2].validation([7; 32], 101, NOW_S);
        let stranger = validator(40).validation([7; 32], 100, NOW_S);
        let mut unsigned = Validation::from_bytes(&sign(&validators[3])).unwrap();
        unsigned.signature = validators[4].signing.sign(b"other");
        let odd = [
            partial,
            elsewhere,
            stranger,
            unsigned.to_bytes(),
            b"none".to_vec(),
        ];
        let tallied = tally(&unl, &odd, NOW_MS);
        assert_eq!(tallied.counted, 0);
        assert_eq!(
            (tallied.refused, tallied.elsewhere, tallied.unknown),
            (3, 1, 1)
        );
        // Signed two ledgers of the index: counts for neither.
        let mut equivocal: Vec<Vec<u8>> = validators.iter().map(sign).collect();
        equivocal.push(validators[0].validation([8; 32], 100, NOW_S));
        let tallied = tally(&unl, &equivocal, NOW_MS);
        assert_eq!((tallied.counted, tallied.conflicting), (4, 1));
        assert_eq!(tallied.missing, vec![validators[0].key()]);
    }

    // A master that signed two ledgers of the index counts for neither,
    // whatever the order of the answer: a forgery of another ledger sent
    // first does not hide the real one sent after it, of another hash or
    // the same, and a master still costs two signature checks at most.
    #[test]
    fn a_second_ledger_counts_against_a_master_in_any_order() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let forged = |hash: [u8; 32]| {
            let mut forged =
                Validation::from_bytes(&validators[0].validation(hash, 100, NOW_S)).unwrap();
            forged.signature = validators[1].signing.sign(b"forged");
            forged.to_bytes()
        };
        let honest: Vec<Vec<u8>> = (validators[..4].iter())
            .map(|validator| validator.validation([7; 32], 100, NOW_S))
            .collect();
        let second = validators[0].validation([9; 32], 100, NOW_S);
        // The real second ledger first, the one the tally checks.
        let shown: Vec<Vec<u8>> = [second.clone()].into_iter().chain(honest.clone()).collect();
        let tallied = tally(&unl, &shown, NOW_MS);
        assert_eq!((tallied.counted, tallied.conflicting), (3, 1));
        assert!(!tallied.reached());
        // A forgery first, of another hash or the second's own.
        for hash in [[8; 32], [9; 32]] {
            let hidden: Vec<Vec<u8>> = [forged(hash)]
                .into_iter()
                .chain(honest.clone())
                .chain([second.clone()])
                .collect();
            let checks = Counting::default();
            let tallied = unl
                .tally(
                    ([7; 32], 100),
                    hidden.iter().map(Vec::as_slice),
                    NOW_MS,
                    &checks,
                )
                .unwrap();
            assert_eq!(tallied.counted, 3, "the equivocator counts for neither");
            assert!(!tallied.reached());
            assert!(tallied.missing.contains(&validators[0].key()));
            assert_eq!(checks.0.get(), 5);
        }
        // A forgery alone takes nothing from the master that signed.
        let lone: Vec<Vec<u8>> = [forged([8; 32])].into_iter().chain(honest).collect();
        let tallied = tally(&unl, &lone, NOW_MS);
        assert_eq!((tallied.counted, tallied.conflicting), (4, 0));
    }

    #[test]
    fn signatures_are_fresh_within_rippled_s_window() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let validations: Vec<Vec<u8>> = (validators.iter())
            .map(|validator| validator.validation([7; 32], 100, NOW_S))
            .collect();
        for (now_ms, counted) in [
            (NOW_MS, 5),
            (NOW_MS + FRESH_FOR_MS - 1, 5),
            (NOW_MS + FRESH_FOR_MS, 0),
            (NOW_MS - FRESH_AHEAD_MS + 1, 5),
            (NOW_MS - FRESH_AHEAD_MS, 0),
        ] {
            let tallied = tally(&unl, &validations, now_ms);
            assert_eq!(tallied.counted, counted, "{now_ms}");
            assert_eq!(tallied.stale, 5 - counted);
        }
    }

    // Stale validations count distinct masters whose signatures check: one
    // validator's, repeated, or unsigned ones, never look like a quorum
    // that signed too late.
    #[test]
    fn stale_validations_count_once_per_master_and_only_signed() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let late = NOW_MS + FRESH_FOR_MS;
        let one = validators[0].validation([7; 32], 100, NOW_S);
        let repeated = vec![one; 10];
        assert_eq!(tally(&unl, &repeated, late).stale, 1);
        let mut unsigned =
            Validation::from_bytes(&validators[1].validation([7; 32], 100, NOW_S)).unwrap();
        unsigned.signature = validators[1].signing.sign(b"other");
        let unsigned = vec![unsigned.to_bytes(); 10];
        assert_eq!(tally(&unl, &unsigned, late).stale, 0);
        // Of another ledger of the index: not this one's, stale or not.
        let other = vec![validators[2].validation([8; 32], 100, NOW_S)];
        assert_eq!(tally(&unl, &other, late).stale, 0);
        // A master that counted is not stale too.
        let both = vec![
            validators[3].validation([7; 32], 100, NOW_S),
            validators[3].validation([7; 32], 100, NOW_S + 60),
        ];
        let tallied = tally(&unl, &both, late);
        assert_eq!((tallied.counted, tallied.stale), (1, 0));
    }

    // A flood costs at most two signature checks per master: its
    // validation of the ledger, and one of another.
    #[test]
    fn a_flood_costs_two_signature_checks_per_master() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let mut flood = Vec::new();
        for (round, validator) in (0..500).zip(validators.iter().cycle()) {
            let other = u8::try_from(round % 3).unwrap();
            flood.push(validator.validation([7 + other; 32], 100, NOW_S));
        }
        let checks = Counting::default();
        let validations = flood.iter().map(Vec::as_slice);
        let tallied = unl
            .tally(([7; 32], 100), validations, NOW_MS, &checks)
            .unwrap();
        assert_eq!(tallied.counted, 0);
        assert_eq!(tallied.conflicting, 5);
        assert_eq!(checks.0.get(), 10);
        let too_many = vec![b"x".to_vec(); MAX_VALIDATIONS + 1];
        let validations = too_many.iter().map(Vec::as_slice);
        assert_eq!(
            unl.tally(([7; 32], 100), validations, NOW_MS, &checks),
            Err(TallyError::TooMany)
        );
    }

    fn header(index: u32, close_time: u32) -> LedgerHeader {
        LedgerHeader {
            sequence: index,
            total_drops: 99_000_000_000_000_000,
            parent_hash: [1; 32],
            transaction_hash: [2; 32],
            account_hash: [3; 32],
            parent_close_time: close_time - 4,
            close_time,
            close_time_resolution: 10,
            close_flags: 0,
        }
    }

    // spec/designs/xrpl-validation.md#6-the-account-on-proofs: an older
    // anchor is never current, and is refused before any signature is
    // checked.
    #[test]
    fn an_anchor_is_a_validated_ledger_not_older_than_the_kept_one() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        let kept_header = header(100, NOW_S - 10);
        let kept = Anchor::of(&kept_header);
        let anchor = |header: &LedgerHeader, count: usize, checks: &Counting| {
            let hash = header.hash();
            let validations: Vec<Vec<u8>> = (validators.iter().take(count))
                .map(|validator| validator.validation(hash, header.sequence, NOW_S))
                .collect();
            unl.anchor(
                header,
                validations.iter().map(Vec::as_slice),
                NOW_MS,
                Some(&kept),
                checks,
            )
        };
        let checks = Counting::default();
        let newer = anchor(&header(105, NOW_S - 2), 4, &checks).unwrap();
        assert_eq!(newer.relation, Some(Relation::Newer));
        assert_eq!(newer.anchor.index, 105);
        assert_eq!(newer.tally.counted, 4);
        assert_eq!(
            anchor(&kept_header, 5, &checks).unwrap().relation,
            Some(Relation::Same)
        );
        let checked = checks.0.get();
        assert_eq!(
            anchor(&header(99, NOW_S - 14), 5, &checks),
            Err(AnchorError::Older)
        );
        assert_eq!(checks.0.get(), checked);
        for diverged in [
            header(100, NOW_S - 9),
            header(98, NOW_S - 2),
            header(101, NOW_S - 20),
        ] {
            assert_eq!(
                anchor(&diverged, 5, &checks).unwrap().relation,
                Some(Relation::Diverged)
            );
        }
        let Err(AnchorError::NoQuorum(tally)) = anchor(&header(106, NOW_S - 1), 3, &checks) else {
            panic!("no quorum");
        };
        assert_eq!((tally.counted, tally.quorum), (3, 4));
        let none = Unl::new(unl.config.clone());
        assert!(matches!(
            none.anchor(&kept_header, [], NOW_MS, None, &checks),
            Err(AnchorError::Unl(UnlError::Unavailable { .. }))
        ));
    }

    /// A UNL of two publishers with a current and a pending list, and a
    /// manifest newer than the lists'.
    fn kept_state() -> (Unl, [TestPublisher; 2], Vec<TestValidator>) {
        let publishers = [publisher(1), publisher(2)];
        let validators = validators(10..16);
        let mut unl = unl(&[&publishers[0], &publishers[1]]);
        let mut later = list(9, &validators[1..]);
        later.effective = Some(NOW_S + 600);
        let first = publishers[0].body(&[list(7, &validators[..5]), later]);
        unl.take_list(&first, NOW_MS).unwrap();
        unl.take_list(&publishers[1].body(&[list(3, &validators)]), NOW_MS)
            .unwrap();
        let rotated = Secp256k1Key::from_secret(&[60; 32]).unwrap();
        let newer = Manifest::sign(2, &validators[2].master, Some(&rotated as &dyn Key), None);
        unl.take_manifest(&newer.to_bytes()).unwrap();
        unl.check_invariants();
        (unl, publishers, validators)
    }

    // spec/designs/xrpl-validation.md#10-configuration-and-state: what is
    // kept round-trips, verified again; what the account never writes is
    // refused.
    #[test]
    fn the_kept_state_round_trips() {
        let (unl, _, validators) = kept_state();
        let state = unl.state();
        // Every listed master's newest manifest.
        assert_eq!(
            state["manifests"].as_array().unwrap().len(),
            validators.len()
        );
        let restored = Unl::restore(unl.config.clone(), &state, NOW_MS).unwrap();
        restored.check_invariants();
        assert_eq!(restored, unl);
        assert_eq!(restored.state(), state);
        // Once the pending list is due, it comes back current.
        let due = NOW_MS + 600_000;
        let later = Unl::restore(unl.config.clone(), &state, due).unwrap();
        later.check_invariants();
        assert_eq!(later.lists(0).0.unwrap().sequence, 9);
        assert_eq!(later.publishers[0].highest, Some(9));
        // Expired, the lists are dropped, and fetched again; the highest
        // sequences stay.
        let expired = Unl::restore(unl.config.clone(), &state, NOW_MS + 2 * 86_400_000).unwrap();
        expired.check_invariants();
        assert_eq!(
            (expired.lists(0), expired.lists(1)),
            ((None, vec![]), (None, vec![]))
        );
        assert_eq!(expired.publishers[1].highest, Some(3));
        assert!(expired.manifests.is_empty());
        // Under other publishers, theirs are dropped.
        let mut config = unl.config.clone();
        config.publishers.remove(0);
        let other = Unl::restore(config, &state, NOW_MS).unwrap();
        other.check_invariants();
        assert_eq!(other.lists(0).0.unwrap().sequence, 3);
        assert_eq!(other.unl(NOW_MS).unwrap().len(), validators.len());
    }

    // Each pending list round-trips, and moves on at a restart as a tick
    // would have: the latest due is the current one.
    #[test]
    fn the_pending_lists_round_trip() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        let mut next = list(2, &validators[..3]);
        next.effective = Some(NOW_S + 60);
        let mut later = list(3, &validators[..4]);
        later.effective = Some(NOW_S + 120);
        let body = ours.body(&[list(1, &validators), next.clone(), later.clone()]);
        unl.take_list(&body, NOW_MS).unwrap();
        let state = unl.state();
        assert_eq!(
            state["publishers"][0]["pending"].as_array().unwrap().len(),
            2
        );
        let restored = Unl::restore(unl.config.clone(), &state, NOW_MS).unwrap();
        restored.check_invariants();
        assert_eq!(restored, unl);
        assert_eq!(restored.state(), state);
        for (at_ms, current, pending) in [
            (NOW_MS + 60_000, &next, vec![&later]),
            (NOW_MS + 120_000, &later, vec![]),
        ] {
            let restored = Unl::restore(unl.config.clone(), &state, at_ms).unwrap();
            restored.check_invariants();
            assert_eq!(restored.lists(0), (Some(current), pending));
            let mut ticked = unl.clone();
            ticked.tick(at_ms);
            assert_eq!(restored, ticked);
        }
        // A due list that expired since blocks, and takes the current one
        // with it, as a tick would.
        let mut short = list(4, &validators[..2]);
        short.effective = Some(NOW_S + 60);
        short.expiration = NOW_S + 90;
        let mut unl = self::unl(&[&ours]);
        unl.take_list(&ours.body(&[list(1, &validators), short]), NOW_MS)
            .unwrap();
        let state = unl.state();
        let at_ms = NOW_MS + 100_000;
        let restored = Unl::restore(unl.config.clone(), &state, at_ms).unwrap();
        restored.check_invariants();
        unl.tick(at_ms);
        assert_eq!(restored, unl);
        assert_eq!(restored.lists(0), (None, vec![]));
        assert!(restored.blocked(at_ms));
    }

    // A state written when a publisher kept one pending list, or none,
    // still loads.
    #[test]
    fn a_state_with_one_pending_list_loads() {
        let (unl, _, _) = kept_state();
        let mut state = unl.state();
        let pending = state["publishers"][0]["pending"][0].clone();
        assert!(pending.is_object());
        state["publishers"][0]["pending"] = pending;
        state["publishers"][1]["pending"] = Value::Null;
        let restored = Unl::restore(unl.config.clone(), &state, NOW_MS).unwrap();
        restored.check_invariants();
        assert_eq!(restored, unl);
        assert_eq!(restored.lists(0).1.len(), 1);
    }

    // Lists that wait with no current list: an account's first answer
    // dated ahead, or a current list that expired while one waits. They
    // hold the invariants and round-trip, also in the form written when a
    // publisher kept one pending list.
    #[test]
    fn pending_lists_wait_without_a_current_one() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let waiting = |sequence: u32, after_s: u32| {
            let mut later = list(sequence, &validators);
            later.effective = Some(NOW_S + after_s);
            later
        };
        let round_trip = |unl: &Unl, now_ms: u64| {
            unl.check_invariants();
            let state = unl.state();
            let restored = Unl::restore(unl.config.clone(), &state, now_ms).unwrap();
            restored.check_invariants();
            assert_eq!(&restored, unl);
            state
        };
        // A fresh account's first answer, all of it dated ahead.
        let mut fresh = unl(&[&ours]);
        let body = ours.body(&[waiting(1, 60), waiting(2, 120)]);
        assert_eq!(fresh.take_list(&body, NOW_MS), Ok(Taken::Pending));
        assert_eq!(fresh.lists(0).0, None);
        assert_eq!(fresh.lists(0).1.len(), 2);
        round_trip(&fresh, NOW_MS);
        // The current list expires while one waits.
        let mut unl = unl(&[&ours]);
        let mut short = list(1, &validators);
        short.expiration = NOW_S + 60;
        let body = ours.body(&[short, waiting(2, 120)]);
        assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Current));
        let expired_ms = NOW_MS + 60_000;
        unl.tick(expired_ms);
        assert_eq!(unl.lists(0).0, None);
        assert_eq!(unl.lists(0).1.len(), 1);
        let mut state = round_trip(&unl, expired_ms);
        // As written when a publisher kept one pending list.
        let pending = state["publishers"][0]["pending"][0].clone();
        state["publishers"][0]["pending"] = pending;
        assert_eq!(state["publishers"][0]["current"], Value::Null);
        let restored = Unl::restore(unl.config.clone(), &state, expired_ms).unwrap();
        restored.check_invariants();
        assert_eq!(restored, unl);
        // Then the one that waits takes effect.
        let due_ms = NOW_MS + 120_000;
        unl.tick(due_ms);
        assert_eq!(unl.lists(0).0.unwrap().sequence, 2);
        round_trip(&unl, due_ms);
    }

    // A master's newer manifest one list carries outlives that list at a
    // restart, while another list still names the master with an older
    // one: its signing never goes back to the key it rotated away from.
    #[test]
    fn a_restart_never_takes_a_master_back_to_an_older_key() {
        let publishers = [publisher(1), publisher(2)];
        let validators = validators(10..15);
        let mut unl = unl(&[&publishers[0], &publishers[1]]);
        let rotated = Secp256k1Key::from_secret(&[60; 32]).unwrap();
        let first = &validators[0];
        let newer = Manifest::sign(2, &first.master, Some(&rotated as &dyn Key), None);
        let mut carrying = list(1, &validators);
        carrying.expiration = NOW_S + 60;
        carrying.validators.insert(first.key(), Some(newer));
        unl.take_list(&publishers[0].body(&[carrying]), NOW_MS)
            .unwrap();
        unl.take_list(&publishers[1].body(&[list(1, &validators)]), NOW_MS)
            .unwrap();
        assert_eq!(unl.signing_key(&first.key()), Some(rotated.public_key()));
        let state = unl.state();
        // The list that carried it has expired by the restart.
        let later = NOW_MS + 120_000;
        unl.tick(later);
        unl.check_invariants();
        assert_eq!(unl.lists(0), (None, vec![]));
        let restored = Unl::restore(unl.config.clone(), &state, later).unwrap();
        restored.check_invariants();
        assert_eq!(restored, unl);
        assert_eq!(
            restored.signing_key(&first.key()),
            Some(rotated.public_key())
        );
        assert_eq!(restored.master_of(&first.signing.public_key()), None);
    }

    #[test]
    fn an_impossible_state_is_refused() {
        let (unl, publishers, _) = kept_state();
        let state = unl.state();
        let config = || unl.config.clone();
        let refused = |change: &dyn Fn(&mut Value)| {
            let mut changed = state.clone();
            change(&mut changed);
            Unl::restore(config(), &changed, NOW_MS)
        };
        // A publisher twice: two current lists for one.
        assert_eq!(
            refused(&|state| {
                let first = state["publishers"][0].clone();
                state["publishers"].as_array_mut().unwrap().push(first);
            }),
            None
        );
        // Two manifests for one master.
        assert_eq!(
            refused(&|state| {
                let first = state["manifests"][0].clone();
                state["manifests"].as_array_mut().unwrap().push(first);
            }),
            None
        );
        assert_eq!(
            refused(&|state| state["manifests"] = vec![Value::Null; MAX_MANIFESTS + 1].into()),
            None
        );
        // A list above the highest sequence.
        assert_eq!(
            refused(&|state| state["publishers"][1]["highest"] = 2.into()),
            None
        );
        assert_eq!(
            refused(&|state| state["publishers"][1]["highest"] = Value::Null),
            None
        );
        // A pending list not above the current one, or in effect already.
        assert_eq!(
            refused(&|state| {
                let current = state["publishers"][1]["current"].clone();
                state["publishers"][1]["pending"] = current;
            }),
            None
        );
        // Lists of a publisher that revoked its key.
        let revocation = Manifest::sign(REVOKED, &publishers[0].master, None, None);
        assert_eq!(
            refused(&|state| {
                state["publishers"][0]["manifest"] = hex_upper(&revocation.to_bytes()).into();
            }),
            None
        );
        // Bytes that do not read.
        assert_eq!(
            refused(&|state| state["publishers"][0]["current"]["blob"] = "00".into()),
            None
        );
        assert_eq!(Unl::restore(config(), &json!({}), NOW_MS), None);
        // A list whose signature no longer verifies is dropped, not refused.
        let dropped = refused(&|state| {
            let other = state["publishers"][0]["current"]["signature"].clone();
            state["publishers"][1]["current"]["signature"] = other;
        })
        .unwrap();
        dropped.check_invariants();
        assert_eq!(dropped.lists(1), (None, vec![]));
        // So is a publisher's manifest its master did not sign, with the
        // lists verified under it.
        let unsigned = refused(&|state| {
            let other = state["publishers"][0]["manifest"].clone();
            state["publishers"][1]["manifest"] = other;
        })
        .unwrap();
        unsigned.check_invariants();
        assert_eq!(unsigned.lists(1), (None, vec![]));
        assert_eq!(unsigned.publishers[1].manifest, None);
    }

    // Pending lists the account never writes: out of order, twice, more
    // than are kept, not taking effect later than the one before, or one
    // that does not read.
    #[test]
    fn impossible_pending_lists_are_refused() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours]);
        let waiting = |sequence: u32, after_s: u32| {
            let mut later = list(sequence, &validators);
            later.effective = Some(NOW_S + after_s);
            later
        };
        let body = ours.body(&[list(1, &validators), waiting(2, 60), waiting(3, 120)]);
        unl.take_list(&body, NOW_MS).unwrap();
        let state = unl.state();
        let refused = |change: &dyn Fn(&mut Vec<Value>)| {
            let mut changed = state.clone();
            change(changed["publishers"][0]["pending"].as_array_mut().unwrap());
            Unl::restore(unl.config.clone(), &changed, NOW_MS)
        };
        assert!(refused(&|_| {}).is_some());
        assert_eq!(refused(&|pending| pending.swap(0, 1)), None);
        assert_eq!(refused(&|pending| pending[1] = pending[0].clone()), None);
        assert_eq!(
            refused(&|pending| pending.resize(MAX_PENDING + 1, pending[1].clone())),
            None
        );
        let sooner = |pending: &mut Vec<Value>| {
            let blob = hex_upper(&waiting(3, 60).to_blob());
            pending[1]["blob"] = blob.into();
        };
        assert_eq!(refused(&sooner), None);
        assert_eq!(refused(&|pending| pending[1] = Value::Null), None);
    }
}
