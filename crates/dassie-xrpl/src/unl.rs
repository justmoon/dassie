//! The UNL: the validators every trusted publisher's list names, each with
//! its current signing key, and the quorum of them that makes a ledger
//! validated (spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers),
//! sans-IO.
//!
//! - **A list counts** when its publisher's manifest verifies under the
//!   configured master key, its blob under the publisher's current signing
//!   key, its sequence is not below the highest seen of that publisher, and
//!   it has not expired. One that takes effect later waits as pending until
//!   then; a publisher keeps one current list and one pending. A publisher
//!   that revoked its key is unavailable for good.
//! - **Manifests** of validators come from the lists and from the server's
//!   `manifest` method alike, and count only for a master a list names and
//!   only when that master signed them: the highest sequence wins, and a
//!   revocation leaves the master no key to sign with. A signing key serves
//!   one master only, and never is a listed master's own key.
//! - **The UNL** is the validators every publisher's list in effect names,
//!   and none while any publisher has no list in effect, or while fewer
//!   remain than the floor. Quorum is four fifths of it, rounded up: there
//!   is no negative UNL.
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
    List, ListError, MAX_LIST_BYTES, MAX_LIST_VALIDATORS, MAX_PUBLISHERS, Message, SignedBlob,
    UnlConfig,
};
use crate::{from_hex, hex_upper};

/// The most validator manifests kept: one for each master any kept list
/// names, a current and a pending one of each publisher.
pub const MAX_MANIFESTS: usize = 2 * MAX_PUBLISHERS * MAX_LIST_VALIDATORS;
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

/// How many of `validators` make a quorum: four fifths, rounded up.
#[must_use]
pub const fn quorum(validators: usize) -> usize {
    (4 * validators).div_ceil(5)
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
    /// Its list expired.
    Expired,
    /// It revoked its key.
    Revoked,
}

/// Why there is no UNL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlError {
    /// A publisher, by its place in the config, has no list in effect.
    Unavailable {
        /// The publisher.
        publisher: usize,
        /// Why.
        why: Unavailable,
    },
    /// The lists in effect leave fewer validators than the floor.
    BelowFloor {
        /// How many they leave.
        validators: usize,
        /// The floor.
        floor: usize,
    },
}

/// Why a validator's manifest is not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// It is not a manifest.
    Malformed,
    /// No kept list names its master.
    NotListed,
    /// Its keys did not sign it.
    BadSignature,
    /// Its signing key is a listed master's, or another master's signing
    /// key.
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
    /// Validations of the UNL's validators signed at a time not fresh.
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

    fn expired(&self, now_s: u64) -> bool {
        u64::from(self.list.expiration) <= now_s
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
    /// A list that takes effect later, of a higher sequence than the
    /// current.
    pending: Option<Kept>,
}

impl PublisherState {
    fn revoked(&self) -> bool {
        self.manifest.as_ref().is_some_and(Manifest::is_revocation)
    }

    fn kept(&self) -> impl Iterator<Item = &Kept> {
        self.current.iter().chain(self.pending.iter())
    }

    /// Its list in effect at `now_s`.
    fn in_effect(&self, now_s: u64) -> Result<&Kept, Unavailable> {
        if self.revoked() {
            return Err(Unavailable::Revoked);
        }
        let due = |kept: &&Kept| {
            (kept.list.effective).is_some_and(|effective| u64::from(effective) <= now_s)
                && !kept.expired(now_s)
        };
        if let Some(pending) = self.pending.as_ref().filter(due) {
            return Ok(pending);
        }
        match &self.current {
            Some(current) if !current.expired(now_s) => Ok(current),
            Some(_) => Err(Unavailable::Expired),
            None => Err(Unavailable::NoList),
        }
    }
}

/// The UNL of one account, and what makes it: its publishers' lists and
/// its validators' manifests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unl {
    config: UnlConfig,
    /// One for each publisher of the config, in its order.
    publishers: Vec<PublisherState>,
    /// Each listed master's newest manifest.
    manifests: BTreeMap<PublicKey, Manifest>,
    /// Each signing key of a manifest kept, and its master.
    signers: BTreeMap<PublicKey, PublicKey>,
}

impl Unl {
    /// One that holds no list yet. A config that does not pass
    /// [`UnlConfig::check`] is a bug.
    #[must_use]
    pub fn new(config: UnlConfig) -> Self {
        assert!(config.check(), "a config an account can follow");
        Self {
            publishers: alloc::vec![PublisherState::default(); config.publishers.len()],
            config,
            manifests: BTreeMap::new(),
            signers: BTreeMap::new(),
        }
    }

    /// Its config.
    #[must_use]
    pub const fn config(&self) -> &UnlConfig {
        &self.config
    }

    /// Publisher `publisher`'s current list and pending one.
    #[must_use]
    pub fn lists(&self, publisher: usize) -> (Option<&List>, Option<&List>) {
        let Some(state) = self.publishers.get(publisher) else {
            return (None, None);
        };
        (
            state.current.as_ref().map(|kept| &kept.list),
            state.pending.as_ref().map(|kept| &kept.list),
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
        for (list, signed) in blobs {
            let taken = self.take_signed(publisher, list, signed, now_s);
            best = Some(better(best, taken));
            if taken == Ok(Taken::Revoked) {
                break;
            }
        }
        // Whatever was taken, before an error or not.
        self.prune();
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

    /// Moves on to `now_ms`: a pending list that took effect becomes its
    /// publisher's current one, and an expired list goes.
    pub fn tick(&mut self, now_ms: u64) {
        let now_s = ledger_seconds(now_ms);
        for state in &mut self.publishers {
            let due = (state.pending.as_ref()).is_some_and(|pending| {
                (pending.list.effective).is_some_and(|effective| u64::from(effective) <= now_s)
            });
            if due {
                state.current = state.pending.take();
            }
            for slot in [&mut state.current, &mut state.pending] {
                if slot.as_ref().is_some_and(|kept| kept.expired(now_s)) {
                    *slot = None;
                }
            }
        }
        self.prune();
    }

    /// The UNL at `now_ms`: the masters every publisher's list in effect
    /// names, in key order.
    ///
    /// # Errors
    ///
    /// Why there is none.
    pub fn unl(&self, now_ms: u64) -> Result<Vec<PublicKey>, UnlError> {
        let now_s = ledger_seconds(now_ms);
        let mut lists = Vec::with_capacity(self.publishers.len());
        for (publisher, state) in self.publishers.iter().enumerate() {
            let kept =
                (state.in_effect(now_s)).map_err(|why| UnlError::Unavailable { publisher, why })?;
            lists.push(&kept.list);
        }
        let Some((first, rest)) = lists.split_first() else {
            return Err(UnlError::Unavailable {
                publisher: 0,
                why: Unavailable::NoList,
            });
        };
        let validators: Vec<PublicKey> = (first.validators.keys())
            .filter(|master| rest.iter().all(|list| list.validators.contains_key(master)))
            .copied()
            .collect();
        if validators.len() < self.config.floor {
            return Err(UnlError::BelowFloor {
                validators: validators.len(),
                floor: self.config.floor,
            });
        }
        Ok(validators)
    }

    /// The key a listed `master` signs with: its newest manifest's, none
    /// if that revoked it, and itself if it has none, as rippled has it.
    #[must_use]
    pub fn signing_key(&self, master: &PublicKey) -> Option<PublicKey> {
        match self.manifests.get(master) {
            Some(manifest) => manifest.signing.map(|(key, _)| key),
            None => self.listed(master).then_some(*master),
        }
    }

    /// The listed master that signs with `signing_key`, if one does.
    #[must_use]
    pub fn master_of(&self, signing_key: &PublicKey) -> Option<PublicKey> {
        self.signers.get(signing_key).copied().or_else(|| {
            (!self.manifests.contains_key(signing_key) && self.listed(signing_key))
                .then_some(*signing_key)
        })
    }

    /// The publisher whose master key is `key`.
    fn position(&self, key: &PublicKey) -> Option<usize> {
        (self.config.publishers.iter()).position(|publisher| publisher.master_key == *key)
    }

    /// Whether a kept list names `master`.
    fn listed(&self, master: &PublicKey) -> bool {
        (self.publishers.iter())
            .flat_map(PublisherState::kept)
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
            state.pending = None;
            self.prune();
        }
        Ok(revoked)
    }

    /// Takes `list` of `publisher`, signed in `blob` under `manifest`, at
    /// `now_s`: below the highest sequence seen only when `fresh` is not
    /// set, as at a restart.
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
        let later = list
            .effective
            .is_some_and(|effective| u64::from(effective) > now_s);
        let manifests: Vec<Manifest> = list.validators.values().flatten().cloned().collect();
        let kept = Some(Kept {
            list,
            blob,
            signature,
            manifest,
        });
        let taken = if later {
            state.pending = kept;
            Taken::Pending
        } else {
            state.current = kept;
            if (state.pending.as_ref()).is_some_and(|pending| pending.list.sequence <= sequence) {
                state.pending = None;
            }
            Taken::Current
        };
        state.highest = Some(
            state
                .highest
                .map_or(sequence, |highest| highest.max(sequence)),
        );
        for manifest in manifests {
            // One the list carries that does not verify is left out, as
            // rippled leaves it: the list counts all the same.
            let _ = self.take_validator_manifest(manifest);
        }
        Ok(taken)
    }

    /// Takes a validator's `manifest`, the cheapest checks first: whether it
    /// is newer than the one kept.
    fn take_validator_manifest(&mut self, manifest: Manifest) -> Result<bool, ManifestError> {
        let master = manifest.master_key;
        if !self.listed(&master) {
            return Err(ManifestError::NotListed);
        }
        if (self.manifests.get(&master)).is_some_and(|kept| kept.sequence >= manifest.sequence) {
            return Ok(false);
        }
        if let Some((key, _)) = manifest.signing {
            let other = self.signers.get(&key).is_some_and(|owner| *owner != master);
            if other || self.listed(&key) {
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
            quorum: quorum(unl.len()),
            validators: unl.len(),
            ..Tally::default()
        };
        let mut seen: BTreeMap<PublicKey, Seen> = BTreeMap::new();
        for (count, bytes) in validations.into_iter().enumerate() {
            if count >= MAX_VALIDATIONS {
                return Err(TallyError::TooMany);
            }
            let Some((master, validation)) =
                self.trusted(bytes, ledger_index, &unl, now_ms, &mut tally)
            else {
                continue;
            };
            let master = seen.entry(master).or_default();
            if validation.ledger_hash != ledger_hash {
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
            let other = master.other.take().filter(|_| master.counted);
            if other.is_some_and(|other| signatures.signed(&other)) {
                master.counted = false;
                tally.conflicting += 1;
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

    /// The validation in `bytes` and its master, if it is of the index and
    /// by a validator of `unl`, full, fully canonical and fresh; counted in
    /// `tally` otherwise. Its signature is not checked.
    fn trusted(
        &self,
        bytes: &[u8],
        ledger_index: u32,
        unl: &BTreeSet<PublicKey>,
        now_ms: u64,
        tally: &mut Tally,
    ) -> Option<(PublicKey, Validation)> {
        let Ok(validation) = Validation::from_bytes(bytes) else {
            tally.refused += 1;
            return None;
        };
        if validation.ledger_sequence != ledger_index {
            tally.elsewhere += 1;
            return None;
        }
        let Some(master) = self.master_of(&validation.signing_key) else {
            tally.unknown += 1;
            return None;
        };
        if !unl.contains(&master) {
            tally.untrusted += 1;
            return None;
        }
        if validation.flags & (FULL | FULLY_CANONICAL) != FULL | FULLY_CANONICAL {
            tally.refused += 1;
            return None;
        }
        if !fresh(validation.signing_time, now_ms) {
            tally.stale += 1;
            return None;
        }
        Some((master, validation))
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
    /// back to a key it rotated away from.
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
                    "pending": state.pending.as_ref().map(Kept::state),
                })
            })
            .collect();
        let manifests: Vec<Value> = (self.manifests.values())
            .map(|manifest| hex_upper(&manifest.to_bytes()).into())
            .collect();
        json!({ "publishers": publishers, "manifests": manifests })
    }

    /// What [`Self::state`] wrote, under `config` at `now_ms`, every list
    /// and manifest verified again. One that no longer verifies, because
    /// the publishers changed, or that expired, is dropped, to be fetched
    /// again. `None` for a state the account never writes: a publisher
    /// twice, more manifests than [`MAX_MANIFESTS`] or two of one master,
    /// a list above its publisher's highest sequence, a pending list not
    /// above the current one or not taking effect later, or lists of a
    /// publisher that revoked its key.
    #[must_use]
    pub fn restore(config: UnlConfig, state: &Value, now_ms: u64) -> Option<Self> {
        let mut unl = Self::new(config);
        let publishers = state.get("publishers")?.as_array()?;
        let manifests = state.get("manifests")?.as_array()?;
        if publishers.len() > MAX_PUBLISHERS || manifests.len() > MAX_MANIFESTS {
            return None;
        }
        let mut keys = BTreeSet::new();
        for entry in publishers {
            let key = from_hex(entry.get("key")?.as_str()?)?;
            let key = PublicKey::from_bytes(key.try_into().ok()?)?;
            if !keys.insert(key) {
                return None;
            }
            if let Some(publisher) = unl.position(&key) {
                unl.restore_publisher(publisher, entry, ledger_seconds(now_ms))?;
            }
        }
        let mut masters = BTreeSet::new();
        for entry in manifests {
            let manifest = Manifest::from_bytes(&from_hex(entry.as_str()?)?).ok()?;
            if !masters.insert(manifest.master_key) {
                return None;
            }
            let _ = unl.take_validator_manifest(manifest);
        }
        unl.prune();
        Some(unl)
    }

    /// Publisher `publisher`'s part of the state, in `entry`.
    fn restore_publisher(&mut self, publisher: usize, entry: &Value, now_s: u64) -> Option<()> {
        let highest = match entry.get("highest")? {
            Value::Null => None,
            highest => Some(u32::try_from(highest.as_u64()?).ok()?),
        };
        let manifest = match entry.get("manifest")? {
            Value::Null => None,
            manifest => Some(Manifest::from_bytes(&from_hex(manifest.as_str()?)?).ok()?),
        };
        let current = Kept::restore(entry.get("current")?)?;
        let pending = Kept::restore(entry.get("pending")?)?;
        let above = |kept: &Kept| highest.is_none_or(|highest| kept.list.sequence > highest);
        let disordered = match (&current, &pending) {
            (Some(current), Some(pending)) => pending.list.sequence <= current.list.sequence,
            _ => false,
        };
        let revoked = manifest.as_ref().is_some_and(Manifest::is_revocation);
        let lists = current.iter().chain(pending.iter());
        if lists.clone().any(above)
            || disordered
            || pending
                .as_ref()
                .is_some_and(|pending| pending.list.effective.is_none())
            || (revoked && lists.count() > 0)
        {
            return None;
        }
        let master_key = self.config.publishers.get(publisher)?.master_key;
        let state = self.publishers.get_mut(publisher)?;
        state.highest = highest;
        // Its master key still signed it, or it is dropped with its lists.
        state.manifest =
            manifest.filter(|manifest| manifest.master_key == master_key && manifest.verify());
        let Some(newest) = state.manifest.as_ref().map(|manifest| manifest.sequence) else {
            return Some(());
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
        Some(())
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
            if let Some(pending) = &state.pending {
                assert!(
                    pending.list.effective.is_some(),
                    "a pending list takes effect later"
                );
                assert!(
                    (state.current.as_ref())
                        .is_none_or(|current| current.list.sequence < pending.list.sequence),
                    "a pending list is above the current one"
                );
            }
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
            assert!(self.listed(master), "a manifest of a listed master");
            if let Some((key, _)) = manifest.signing {
                assert_eq!(
                    self.signers.get(&key),
                    Some(master),
                    "its signing key is its"
                );
                assert!(!self.listed(&key), "no signing key is a listed master's");
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

    fn unl(publishers: &[&TestPublisher], floor: usize) -> Unl {
        Unl::new(UnlConfig {
            publishers: publishers
                .iter()
                .map(|publisher| publisher.config())
                .collect(),
            floor,
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

    #[test]
    fn quorum_is_four_fifths_rounded_up() {
        for (validators, quorum) in [
            (1, 1),
            (4, 4),
            (5, 4),
            (6, 5),
            (10, 8),
            (35, 28),
            (128, 103),
        ] {
            assert_eq!(super::quorum(validators), quorum, "{validators}");
        }
    }

    // spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: a list
    // counts under its publisher's signing key, at or above the highest
    // sequence seen, until it expires.
    #[test]
    fn a_list_counts_when_its_publisher_signed_it_and_it_has_not_expired() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours], 4);
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
        assert_eq!(unl.lists(0), (None, None));
        assert!(unl.manifests.is_empty());
    }

    #[test]
    fn only_the_configured_publishers_count() {
        let ours = publisher(1);
        let theirs = publisher(2);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours], 4);
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
        assert_eq!(unl.lists(0), (None, None));
    }

    // A publisher rotates its signing key with a newer manifest, which
    // carries over to lists after it; and revokes its key for good.
    #[test]
    fn publishers_rotate_their_keys_and_revoke_them() {
        let mut ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours], 4);
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
        assert_eq!(unl.lists(0), (None, None));
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
        let mut unl = unl(&[&ours], 4);
        let current = list(1, &validators[..5]);
        let mut later = list(2, &validators[1..]);
        later.effective = Some(NOW_S + 60);
        let body = ours.body(&[later.clone(), current.clone()]);
        assert_eq!(unl.take_list(&body, NOW_MS), Ok(Taken::Current));
        unl.check_invariants();
        assert_eq!(unl.lists(0), (Some(&current), Some(&later)));
        assert!(!unl.unl(NOW_MS).unwrap().contains(&validators[5].key()));
        let due = NOW_MS + 60_000;
        assert!(unl.unl(due).unwrap().contains(&validators[5].key()));
        unl.tick(due - 1_000);
        assert_eq!(unl.lists(0).1, Some(&later));
        unl.tick(due);
        unl.check_invariants();
        assert_eq!(unl.lists(0), (Some(&later), None));
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
        assert_eq!(unl.lists(0).1, None);
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

    // spec/decisions/verified-ledgers.md: every publisher must list a
    // validator, and the UNL has a floor.
    #[test]
    fn the_unl_is_what_every_publisher_lists_above_the_floor() {
        let first = publisher(1);
        let second = publisher(2);
        let validators = validators(10..18);
        let mut unl = unl(&[&first, &second], 5);
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
        assert_eq!(
            unl.take_list(&second.body(&[list(1, &validators[1..])]), NOW_MS),
            Ok(Taken::Current)
        );
        unl.check_invariants();
        let mut both: Vec<PublicKey> = validators[1..7].iter().map(TestValidator::key).collect();
        both.sort();
        assert_eq!(unl.unl(NOW_MS).unwrap(), both);
        // Either publisher alone can shrink it, never below the floor.
        assert_eq!(
            unl.take_list(&second.body(&[list(2, &validators[3..])]), NOW_MS),
            Ok(Taken::Current)
        );
        assert_eq!(
            unl.unl(NOW_MS),
            Err(UnlError::BelowFloor {
                validators: 4,
                floor: 5
            })
        );
        // Nor add to it: what only one lists is not on it.
        unl.config.floor = 4;
        assert!(unl.signing_key(&validators[7].key()).is_some());
        let validations: Vec<Vec<u8>> = (validators[2..8].iter())
            .map(|validator| validator.validation([7; 32], 100, NOW_S))
            .collect();
        let tally = tally(&unl, &validations, NOW_MS);
        assert_eq!((tally.counted, tally.quorum, tally.untrusted), (4, 4, 2));
    }

    // Validators rotate their keys with manifests the lists lag behind:
    // the node takes newer ones from the server, when the listed master
    // signed them.
    #[test]
    fn newer_manifests_of_listed_masters_count() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours], 4);
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
    }

    // spec/designs/xrpl-validation.md#8-the-verifier: a validation counts
    // only by a listed master's current key, full, fully canonical, fresh
    // and signed; a master counts once, and not at all if it signed two
    // ledgers of the index.
    #[test]
    fn a_quorum_of_fresh_signatures_validates() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours], 4);
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

    #[test]
    fn signatures_are_fresh_within_rippled_s_window() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours], 4);
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

    // A flood costs at most two signature checks per master: its
    // validation of the ledger, and one of another.
    #[test]
    fn a_flood_costs_two_signature_checks_per_master() {
        let ours = publisher(1);
        let validators = validators(10..15);
        let mut unl = unl(&[&ours], 4);
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
        let mut unl = unl(&[&ours], 4);
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
        let mut unl = unl(&[&publishers[0], &publishers[1]], 4);
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
            ((None, None), (None, None))
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

    // A master's newer manifest one list carries outlives that list at a
    // restart, while another list still names the master with an older
    // one: its signing never goes back to the key it rotated away from.
    #[test]
    fn a_restart_never_takes_a_master_back_to_an_older_key() {
        let publishers = [publisher(1), publisher(2)];
        let validators = validators(10..15);
        let mut unl = unl(&[&publishers[0], &publishers[1]], 4);
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
        assert_eq!(unl.lists(0), (None, None));
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
        assert_eq!(dropped.lists(1), (None, None));
        // So is a publisher's manifest its master did not sign, with the
        // lists verified under it.
        let unsigned = refused(&|state| {
            let other = state["publishers"][0]["manifest"].clone();
            state["publishers"][1]["manifest"] = other;
        })
        .unwrap();
        unsigned.check_invariants();
        assert_eq!(unsigned.lists(1), (None, None));
        assert_eq!(unsigned.publishers[1].manifest, None);
    }
}
