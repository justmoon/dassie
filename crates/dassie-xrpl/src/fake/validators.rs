//! The fake ledger's validators and the publisher that lists them
//! (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
//!
//! [`VALIDATORS`] validators, enough that one can be withheld and the rest
//! still make a quorum, each with an Ed25519 master key, a secp256k1
//! signing key and the manifest that names it, sign every ledger that
//! closes; a publisher with a master key and a signing key of its own signs
//! their list, which it serves as rippled's list sites do. Every key comes
//! from the fake's seed, so the fake is the same each run; and a fake of
//! another seed has validators of its own, so two fakes' UNLs share no key,
//! as the two networks' do.
//!
//! Validations are signed for real, or, in a simulation, with a keyed hash
//! under a secret only the fake and [`KeyedSignatures`] hold
//! ([`FakeLedger::sign_validations_with`]): a simulation closes ledgers
//! fast for long, and real signatures would cost much of its time. A lying
//! server holds neither the keys nor the secret, so it can withhold or
//! replay validations, never forge them.

use alloc::vec::Vec;

use super::FakeLedger;
use crate::keys::{Key, PublicKey, Secp256k1Key, Signature, Wallet, sha512_half};
use crate::manifest::{Manifest, REVOKED};
use crate::unl::ValidationSignatures;
use crate::validation::{FULL, FULLY_CANONICAL, Validation};
use crate::validator_list::{List, Message, SignedBlob};

/// The fake's validators: five, so one can be withheld and four, a quorum
/// of five, still validate.
pub const VALIDATORS: usize = 5;

/// The floor on the UNL a node that names the fake's publisher sets: one
/// validator below all of them, so a revoked one still leaves a UNL.
pub const UNL_FLOOR: usize = VALIDATORS - 1;

/// The seed the fake testnet's keys come from, unless it is given another.
pub const DEFAULT_SEED: [u8; 32] = *b"dassie fake ledger, testnet keys";

/// The seed the fake mainnet's keys come from: another than the testnet's,
/// so the two networks' UNLs share no key, as live.
pub const MAINNET_SEED: [u8; 32] = *b"dassie fake ledger, mainnet keys";

/// The most a signing key is derived again past a value that is no key:
/// SHA-512Half lands past secp256k1's order with odds of 2^-128.
const MAX_KEY_TRIES: u32 = 8;

/// A validator's key it signs validations with, as of a ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SignerKey {
    secret: [u8; 32],
    key: PublicKey,
}

#[derive(Clone, Debug)]
struct Validator {
    master_seed: [u8; 16],
    master_key: PublicKey,
    /// Its manifest's sequence; [`REVOKED`] once it revoked its key.
    sequence: u32,
    signing: SignerKey,
    manifest: Vec<u8>,
    /// Whether it signs no ledgers, as one offline.
    withheld: bool,
}

#[derive(Clone, Debug)]
struct Publisher {
    master_key: PublicKey,
    manifest: Vec<u8>,
    signing_seed: [u8; 16],
    /// The last list's sequence.
    list_sequence: u32,
    /// The last list, as its site serves it.
    body: Vec<u8>,
}

/// The validators, their publisher, and how validations are signed.
#[derive(Clone, Debug)]
pub(super) struct Validators {
    seed: [u8; 32],
    /// Which set of keys: a reset with new keys counts it up.
    epoch: u32,
    validators: Vec<Validator>,
    publisher: Publisher,
    /// The secret validations are signed with in a simulation; none, real
    /// signatures.
    keyed: Option<[u8; 32]>,
}

/// SHA-512Half of `parts`, each tagged by what it derives.
fn derive(parts: &[&[u8]]) -> [u8; 32] {
    let mut message = Vec::new();
    for part in parts {
        message.extend_from_slice(part);
    }
    sha512_half(&message)
}

fn seed16(bytes: [u8; 32]) -> [u8; 16] {
    let mut seed = [0; 16];
    seed.copy_from_slice(bytes.get(..16).unwrap_or(&[0; 16]));
    seed
}

impl SignerKey {
    /// The signing key `index` of `validator` of `epoch` takes from `seed`.
    fn derive(seed: &[u8; 32], epoch: u32, validator: usize, index: u32) -> Self {
        let validator = u32::try_from(validator).unwrap_or(u32::MAX);
        let found = (0..MAX_KEY_TRIES).find_map(|attempt| {
            let secret = derive(&[
                seed,
                b"signing",
                &epoch.to_be_bytes(),
                &validator.to_be_bytes(),
                &index.to_be_bytes(),
                &attempt.to_be_bytes(),
            ]);
            Secp256k1Key::from_secret(&secret).map(|key| Self {
                secret,
                key: key.public_key(),
            })
        });
        assert!(found.is_some(), "SHA-512Half lands on a secp256k1 key");
        found.unwrap_or(Self {
            secret: [0; 32],
            key: PublicKey::built_in([2; 33]),
        })
    }

    fn secret_key(&self) -> Option<Secp256k1Key> {
        Secp256k1Key::from_secret(&self.secret)
    }
}

impl Validator {
    fn new(seed: &[u8; 32], epoch: u32, index: usize) -> Self {
        let number = u32::try_from(index).unwrap_or(u32::MAX);
        let master_seed = seed16(derive(&[
            seed,
            b"validator",
            &epoch.to_be_bytes(),
            &number.to_be_bytes(),
        ]));
        let master_key = Wallet::from_seed(master_seed).public_key();
        let mut validator = Self {
            master_seed,
            master_key,
            sequence: 1,
            signing: SignerKey::derive(seed, epoch, index, 1),
            manifest: Vec::new(),
            withheld: false,
        };
        validator.sign_manifest();
        validator
    }

    /// Signs its manifest as its keys stand.
    fn sign_manifest(&mut self) {
        let master = Wallet::from_seed(self.master_seed);
        let manifest = if self.sequence == REVOKED {
            Manifest::sign(REVOKED, &master, None, None)
        } else {
            let signing = self.signing.secret_key();
            assert!(signing.is_some(), "a signing key derived");
            let signing = signing.as_ref().map(|key| key as &dyn Key);
            Manifest::sign(self.sequence, &master, signing, None)
        };
        self.manifest = manifest.to_bytes();
    }

    fn signs(&self) -> bool {
        !self.withheld && self.sequence != REVOKED
    }
}

impl Validators {
    /// The validators of `seed` and their publisher's first list, which
    /// expires at `expiration`, in seconds since 2000-01-01.
    pub fn new(seed: [u8; 32], expiration: u32) -> Self {
        let master_seed = seed16(derive(&[&seed, b"publisher"]));
        let signing_seed = seed16(derive(&[&seed, b"publisher signing"]));
        let master = Wallet::from_seed(master_seed);
        let signing = Wallet::from_seed(signing_seed);
        let manifest = Manifest::sign(1, &master, Some(&signing), None).to_bytes();
        let mut validators = Self {
            seed,
            epoch: 0,
            validators: Vec::new(),
            publisher: Publisher {
                master_key: master.public_key(),
                manifest,
                signing_seed,
                list_sequence: 0,
                body: Vec::new(),
            },
            keyed: None,
        };
        validators.rekey(expiration);
        validators
    }

    /// New validators of the next epoch, and a list of them.
    pub fn rekey(&mut self, expiration: u32) {
        self.epoch = self.epoch.saturating_add(1);
        self.validators = (0..VALIDATORS)
            .map(|index| Validator::new(&self.seed, self.epoch, index))
            .collect();
        self.publish(expiration, None);
    }

    /// Publishes a list of every validator that has not revoked its key,
    /// with its manifest as it stands, the next sequence, expiring at
    /// `expiration` and in effect from `effective`, if later.
    pub fn publish(&mut self, expiration: u32, effective: Option<u32>) {
        self.publisher.list_sequence = self.publisher.list_sequence.saturating_add(1);
        let validators = (self.validators.iter())
            .filter(|validator| validator.sequence != REVOKED)
            .map(|validator| {
                let manifest = Manifest::from_bytes(&validator.manifest).ok();
                (validator.master_key, manifest)
            })
            .collect();
        let list = List {
            sequence: self.publisher.list_sequence,
            expiration,
            effective,
            validators,
        };
        let signing = Wallet::from_seed(self.publisher.signing_seed);
        self.publisher.body = Message {
            publisher: self.publisher.master_key,
            manifest: self.publisher.manifest.clone(),
            blobs: alloc::vec![SignedBlob::sign(&list, &signing)],
        }
        .to_json();
        self.check_invariants();
    }

    /// The validator `index` signs with a new key from now on, under a
    /// manifest of the next sequence; a list carries it only once
    /// published. Nothing for a validator that revoked its key.
    pub fn rotate(&mut self, index: usize) {
        let (seed, epoch) = (self.seed, self.epoch);
        if let Some(validator) = self.validators.get_mut(index)
            && validator.sequence != REVOKED
        {
            validator.sequence = validator.sequence.saturating_add(1).min(REVOKED - 1);
            validator.signing = SignerKey::derive(&seed, epoch, index, validator.sequence);
            validator.sign_manifest();
        }
    }

    /// The validator `index` revokes its master key: it signs nothing
    /// more.
    pub fn revoke(&mut self, index: usize) {
        if let Some(validator) = self.validators.get_mut(index) {
            validator.sequence = REVOKED;
            validator.sign_manifest();
        }
    }

    /// The validator `index` signs no ledgers while `withheld`.
    pub fn withhold(&mut self, index: usize, withheld: bool) {
        if let Some(validator) = self.validators.get_mut(index) {
            validator.withheld = withheld;
        }
    }

    /// Validations are signed with a keyed hash under `secret` from now on.
    pub const fn key_with(&mut self, secret: [u8; 32]) {
        self.keyed = Some(secret);
    }

    /// The keys of the validators that sign a ledger closing now.
    pub fn signers(&self) -> Vec<SignerKey> {
        (self.validators.iter())
            .filter(|validator| validator.signs())
            .map(|validator| validator.signing)
            .collect()
    }

    /// The publisher's master key.
    pub const fn publisher_key(&self) -> PublicKey {
        self.publisher.master_key
    }

    /// The list, as the publisher's site serves it.
    pub fn list(&self) -> &[u8] {
        &self.publisher.body
    }

    /// The validators' master keys.
    pub fn masters(&self) -> Vec<PublicKey> {
        self.validators.iter().map(|v| v.master_key).collect()
    }

    /// The newest manifest of the validator whose master or current signing
    /// key `key` is, with its master key, its signing key and its sequence.
    pub fn manifest(&self, key: &PublicKey) -> Option<(&[u8], PublicKey, Option<PublicKey>, u32)> {
        let validator = (self.validators.iter())
            .find(|v| v.master_key == *key || (v.sequence != REVOKED && v.signing.key == *key))?;
        let signing = (validator.sequence != REVOKED).then_some(validator.signing.key);
        Some((
            &validator.manifest,
            validator.master_key,
            signing,
            validator.sequence,
        ))
    }

    /// The validations of the ledger of `hash` and `index`, closed at
    /// `close_time`, by `signers`, each signed as validations are signed
    /// now.
    pub fn validations(
        &self,
        (hash, index, close_time): ([u8; 32], u32, u32),
        signers: &[SignerKey],
    ) -> Vec<Vec<u8>> {
        signers
            .iter()
            .filter_map(|signer| {
                let validation = unsigned(hash, index, close_time, signer.key)?;
                let signed = match self.keyed {
                    Some(secret) => keyed(validation, &secret)?,
                    None => validation.signed(&signer.secret_key()?),
                };
                Some(signed.to_bytes())
            })
            .collect()
    }

    /// The validations a server forges for the ledger: each under the key
    /// of a validator that signed it, signed with a key that is not it, or
    /// under another secret.
    pub fn forged(
        &self,
        (hash, index, close_time): ([u8; 32], u32, u32),
        signers: &[SignerKey],
    ) -> Vec<Vec<u8>> {
        let forger = SignerKey::derive(&self.seed, u32::MAX, 0, 0);
        signers
            .iter()
            .filter_map(|signer| {
                let validation = unsigned(hash, index, close_time, forger.key)?;
                let mut forged = match self.keyed {
                    Some(_) => keyed(validation, &forger.secret)?,
                    None => validation.signed(&forger.secret_key()?),
                };
                forged.signing_key = signer.key;
                Some(forged.to_bytes())
            })
            .collect()
    }

    pub fn check_invariants(&self) {
        assert_eq!(self.validators.len(), VALIDATORS, "the fake's validators");
        let masters = self.masters();
        assert!(
            (masters.iter().enumerate())
                .all(|(at, key)| !masters.get(..at).unwrap_or_default().contains(key)),
            "each validator its own master key"
        );
    }
}

/// A full validation of the ledger, signed at its close, not yet signed.
fn unsigned(hash: [u8; 32], index: u32, close_time: u32, key: PublicKey) -> Option<Validation> {
    Some(Validation {
        flags: FULL | FULLY_CANONICAL,
        ledger_hash: hash,
        ledger_sequence: index,
        close_time: None,
        load_fee: None,
        amendments: None,
        signing_time: close_time,
        signing_key: key,
        signature: Signature::from_slice(&[0])?,
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
    })
}

/// The keyed hash of `validation` under `secret`.
fn keyed_hash(secret: &[u8; 32], validation: &Validation) -> [u8; 32] {
    derive(&[secret, &validation.signing_bytes()])
}

fn keyed(mut validation: Validation, secret: &[u8; 32]) -> Option<Validation> {
    validation.signature = Signature::from_slice(&keyed_hash(secret, &validation))?;
    Some(validation)
}

/// The simulation's check of a validation's signature: the keyed hash
/// under the secret the fake signs with
/// ([`FakeLedger::sign_validations_with`]).
#[derive(Clone, Copy)]
pub struct KeyedSignatures {
    secret: [u8; 32],
}

impl KeyedSignatures {
    /// The check under `secret`.
    #[must_use]
    pub const fn new(secret: [u8; 32]) -> Self {
        Self { secret }
    }
}

impl core::fmt::Debug for KeyedSignatures {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KeyedSignatures").finish_non_exhaustive()
    }
}

impl ValidationSignatures for KeyedSignatures {
    fn signed(&self, validation: &Validation) -> bool {
        validation.signature.as_bytes() == keyed_hash(&self.secret, validation)
    }
}

/// The validators, their keys and their list.
impl FakeLedger {
    /// Signs validations with a keyed hash under `secret` from now on, as a
    /// simulation checks them with [`KeyedSignatures`]: much cheaper than
    /// real signatures, and as unforgeable to a server without the secret.
    pub fn sign_validations_with(&mut self, secret: [u8; 32]) {
        self.validators.key_with(secret);
    }

    /// The master key of the publisher of the fake's validator list.
    #[must_use]
    pub const fn publisher_key(&self) -> PublicKey {
        self.validators.publisher_key()
    }

    /// The validator list, as the publisher's site serves it.
    #[must_use]
    pub fn validator_list(&self) -> Vec<u8> {
        self.validators.list().to_vec()
    }

    /// The validators' master keys.
    #[must_use]
    pub fn validator_keys(&self) -> Vec<PublicKey> {
        self.validators.masters()
    }

    /// Validator `index` signs with a new key from the next ledger, which
    /// the `manifest` method gives at once and a list only once published.
    pub fn rotate_validator(&mut self, index: usize) {
        self.validators.rotate(index);
    }

    /// Validator `index` revokes its master key, and signs no more.
    pub fn revoke_validator(&mut self, index: usize) {
        self.validators.revoke(index);
    }

    /// Validator `index` signs no ledger that closes while `withheld`, as
    /// one offline.
    pub fn withhold_validator(&mut self, index: usize, withheld: bool) {
        self.validators.withhold(index, withheld);
    }

    /// The publisher publishes a new list of the validators with their
    /// manifests as they stand, expiring at `expiration` and in effect from
    /// `effective`, in seconds since 2000-01-01.
    pub fn publish_validator_list(&mut self, expiration: u32, effective: Option<u32>) {
        self.validators.publish(expiration, effective);
    }

    /// New validators, with new master keys, and a list of them that
    /// expires at `expiration`, as a network re-keyed at a reset.
    pub fn rekey_validators(&mut self, expiration: u32) {
        self.validators.rekey(expiration);
    }
}
