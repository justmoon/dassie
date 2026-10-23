//! The binary form of the transactions the node signs or checks
//! (spec/areas/xrpl.md#the-transactions): a `Payment` of XRP or of a token,
//! with memos and perhaps a destination tag; a `TrustSet` that opens or
//! changes a line to a token's issuer; and what an account that several
//! keys control is set up with: an `AccountSet` that requires destination
//! tags or disables the master key, a `SetRegularKey`, and a
//! `SignerListSet`.
//!
//! Fields go in the ledger's canonical order, by type code and then field
//! code; the decoder takes only that order, each field once, and only the
//! fields of these kinds (`codec/wire.rs`), which is all the fake ledger needs to
//! check what it is sent. A Payment has no paths and is never partial. It
//! may name a `SendMax`, the most it spends of another asset than it
//! delivers, which the ledger then converts through its default paths, such
//! as an AMM pool ([`sign_send_max`]).
//!
//! A transaction is signed by one key ([`Transact::sign`]), or by several
//! signers of the sender's signer list, each alone, whose signatures are
//! put together afterwards ([`Transact::multi_sign`],
//! [`Transact::with_signers`]).

mod wire;

use alloc::vec::Vec;

use sha2::{Digest, Sha512};

pub use crate::amount::{Amount, Currency, TokenAmount, TokenValue};
use crate::hex_upper;
use crate::keys::{AccountId, Key, PublicKey, Signature, Wallet};
use wire::{Body, Decoded, Fields, Mode, bits};

/// Most memos on one Payment, and most bytes in a memo's field.
pub const MAX_MEMOS: usize = 4;
/// Most bytes in a memo's type or data.
pub const MAX_MEMO_BYTES: usize = 256;
/// Most signers of one multi-signed transaction, and most entries in a
/// signer list: the ledger's own limit.
pub const MAX_SIGNERS: usize = 32;

/// What a single signature covers, before the fields.
const SIGNING_PREFIX: [u8; 4] = *b"STX\0";
/// What each signature of a multi-signed transaction covers, before the
/// fields; the signer's account follows them.
const MULTI_SIGNING_PREFIX: [u8; 4] = *b"SMT\0";
/// What a transaction's hash covers, before its blob.
const HASH_PREFIX: [u8; 4] = *b"TXN\0";

// Transaction types.
const PAYMENT: u16 = 0;
const ACCOUNT_SET: u16 = 3;
const SET_REGULAR_KEY: u16 = 5;
const SIGNER_LIST_SET: u16 = 12;
const TRUST_SET: u16 = 20;

/// A Payment's flag that lets it deliver less than its amount; nothing
/// here sets it, and the decoder refuses it.
pub const TF_PARTIAL_PAYMENT: u32 = 0x0002_0000;
/// A TrustSet's flag that disables rippling on the sender's side.
pub const TF_SET_NO_RIPPLE: u32 = 0x0002_0000;
/// A TrustSet's flag that enables rippling on the sender's side.
pub const TF_CLEAR_NO_RIPPLE: u32 = 0x0004_0000;
/// A TrustSet's flag that freezes the line on the sender's side: the
/// other side pays its balance only to the sender.
pub const TF_SET_FREEZE: u32 = 0x0010_0000;
/// A TrustSet's flag that lifts the sender's freeze.
pub const TF_CLEAR_FREEZE: u32 = 0x0020_0000;
/// A TrustSet's flag that deep-freezes the line on the sender's side: the
/// other side neither pays nor receives, but to and from the sender. Only
/// on a line frozen already, or with [`TF_SET_FREEZE`].
pub const TF_SET_DEEP_FREEZE: u32 = 0x0040_0000;
/// A TrustSet's flag that lifts the sender's deep freeze.
pub const TF_CLEAR_DEEP_FREEZE: u32 = 0x0080_0000;
/// An AccountSet's flag that requires a destination tag on Payments to the
/// account, as [`ASF_REQUIRE_DEST`] set does.
pub const TF_REQUIRE_DEST_TAG: u32 = 0x0001_0000;
/// An AccountSet's flag that lifts that, as [`ASF_REQUIRE_DEST`] cleared
/// does.
pub const TF_OPTIONAL_DEST_TAG: u32 = 0x0002_0000;

/// An AccountSet's flag number (`asf`): Payments to the account must name
/// a destination tag.
pub const ASF_REQUIRE_DEST: u32 = 1;
/// An AccountSet's flag number: the master key no longer signs, only the
/// regular key or the signer list. Set only with the master key itself,
/// and only while the account has another way to sign.
pub const ASF_DISABLE_MASTER: u32 = 4;

/// A memo: a type and data, both bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memo {
    /// What kind of memo it is.
    pub memo_type: Vec<u8>,
    /// What it says.
    pub memo_data: Vec<u8>,
}

/// One signature of a multi-signed transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signer {
    /// The account it signs as: an entry of the sender's signer list.
    pub account: AccountId,
    /// The key it signed with: that account's master key, or its regular
    /// key.
    pub signing_pub_key: PublicKey,
    /// The signature, on the transaction and the account
    /// ([`Transact::multi_signing_payload`]).
    pub signature: Signature,
}

/// An entry of a signer list: an account, and what its signature weighs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignerEntry {
    /// The account. It need not exist on the ledger: then only its master
    /// key signs for it.
    pub account: AccountId,
    /// Its weight, above zero.
    pub weight: u16,
}

/// Who signs a transaction, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signing {
    /// One key signs: the sender's master key, or its regular key.
    Single {
        /// The key (`SigningPubKey`).
        public_key: PublicKey,
        /// Its signature (`TxnSignature`), once signed.
        signature: Option<Signature>,
    },
    /// Signers of the sender's signer list sign, each alone:
    /// `SigningPubKey` is empty, and the signatures are in `Signers`,
    /// sorted by account. None until put together
    /// ([`Transact::with_signers`]).
    Multi(Vec<Signer>),
}

impl Signing {
    /// Signed by `public_key`, once signed.
    #[must_use]
    pub const fn by(public_key: PublicKey) -> Self {
        Self::Single {
            public_key,
            signature: None,
        }
    }

    /// Signed by signers, once their signatures are put together.
    #[must_use]
    pub const fn multi() -> Self {
        Self::Multi(Vec::new())
    }

    /// Whether it carries a signature: one key's, or at least one signer's.
    #[must_use]
    pub fn is_signed(&self) -> bool {
        match self {
            Self::Single { signature, .. } => signature.is_some(),
            Self::Multi(signers) => !signers.is_empty(),
        }
    }

    /// How many signers signed: 0 for a single signature.
    #[must_use]
    pub fn signers(&self) -> usize {
        match self {
            Self::Single { .. } => 0,
            Self::Multi(signers) => signers.len(),
        }
    }
}

/// The fee a multi-signed transaction pays at least: `base_fee` for each
/// signer, and once more, as the ledger charges.
#[must_use]
pub fn multi_signed_fee(base_fee: u64, signers: usize) -> u64 {
    let signers = u64::try_from(signers).unwrap_or(u64::MAX);
    base_fee.saturating_mul(signers.saturating_add(1))
}

/// A Payment of XRP or of a token: no paths, never partial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payment {
    /// Who pays.
    pub account: AccountId,
    /// Who is paid.
    pub destination: AccountId,
    /// The destination tag, which names what it is for at the destination:
    /// a payee that requires one takes no Payment without it.
    pub destination_tag: Option<u32>,
    /// How much it delivers, all of it or nothing: drops, or a token
    /// amount not below zero.
    pub amount: Amount,
    /// The most it spends, through the ledger's default paths: of another
    /// asset than `amount`, which the ledger converts, or of the same
    /// token, to pay its issuer's transfer fee. Never XRP for XRP. `None`
    /// spends `amount` itself, as the settlement schemes' Payments do.
    pub send_max: Option<Amount>,
    /// The fee, in drops.
    pub fee: u64,
    /// The payer's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in; with none, it stays
    /// valid until its sequence is used.
    pub last_ledger_sequence: Option<u32>,
    /// Flags; 0 for the node's, and never [`TF_PARTIAL_PAYMENT`].
    pub flags: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
    /// Who signs it.
    pub signing: Signing,
}

/// A TrustSet: opens or changes the sender's line to a token's issuer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustSet {
    /// Whose line it is.
    pub account: AccountId,
    /// The line: the issuer, the currency, and as value the most the
    /// sender holds of it, not below zero.
    pub limit: TokenAmount,
    /// The fee, in drops.
    pub fee: u64,
    /// The sender's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in, if any.
    pub last_ledger_sequence: Option<u32>,
    /// Flags, such as [`TF_SET_NO_RIPPLE`].
    pub flags: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
    /// Who signs it.
    pub signing: Signing,
}

/// An AccountSet: sets or clears one of the account's settings, such as
/// [`ASF_REQUIRE_DEST`] or [`ASF_DISABLE_MASTER`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSet {
    /// Whose settings.
    pub account: AccountId,
    /// The setting it turns on (`asf`).
    pub set_flag: Option<u32>,
    /// The setting it turns off, another than `set_flag`.
    pub clear_flag: Option<u32>,
    /// The fee, in drops.
    pub fee: u64,
    /// The sender's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in, if any.
    pub last_ledger_sequence: Option<u32>,
    /// Flags, such as [`TF_REQUIRE_DEST_TAG`].
    pub flags: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
    /// Who signs it.
    pub signing: Signing,
}

/// A SetRegularKey: names the account's regular key, which signs for it as
/// the master key does, or takes it away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetRegularKey {
    /// Whose key.
    pub account: AccountId,
    /// The regular key, as the account of its public key; `None` removes
    /// it.
    pub regular_key: Option<AccountId>,
    /// The fee, in drops.
    pub fee: u64,
    /// The sender's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in, if any.
    pub last_ledger_sequence: Option<u32>,
    /// Flags.
    pub flags: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
    /// Who signs it.
    pub signing: Signing,
}

/// A SignerListSet: replaces the account's signer list, whose signers
/// together sign for it once their weights reach the quorum; a quorum of
/// zero and no entries deletes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignerListSet {
    /// Whose list.
    pub account: AccountId,
    /// The weight the signers of a transaction must reach together.
    pub signer_quorum: u32,
    /// The signers, at most [`MAX_SIGNERS`] (see [`check_signer_list`]).
    pub signer_entries: Vec<SignerEntry>,
    /// The fee, in drops.
    pub fee: u64,
    /// The sender's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in, if any.
    pub last_ledger_sequence: Option<u32>,
    /// Flags.
    pub flags: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
    /// Who signs it.
    pub signing: Signing,
}

/// A transaction the codec reads: any of its kinds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transaction {
    /// A Payment.
    Payment(Payment),
    /// A TrustSet.
    TrustSet(TrustSet),
    /// An AccountSet.
    AccountSet(AccountSet),
    /// A SetRegularKey.
    SetRegularKey(SetRegularKey),
    /// A SignerListSet.
    SignerListSet(SignerListSet),
}

/// A signed transaction, ready to submit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed<T = Payment> {
    /// The transaction.
    pub tx: T,
    /// Its binary form.
    pub blob: Vec<u8>,
    /// Its hash.
    pub hash: [u8; 32],
}

impl<T> Signed<T> {
    /// The hash as the ledger writes it, in uppercase hex.
    #[must_use]
    pub fn hash_hex(&self) -> alloc::string::String {
        hex_upper(&self.hash)
    }
}

/// Why bytes are not a transaction the codec reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// It ended early.
    Truncated,
    /// A field is missing, unknown, out of order, or its header is longer
    /// than it needs to be.
    Field,
    /// A value is out of range, such as an amount, or another kind than
    /// asked for.
    Value,
    /// Bytes are left over.
    Trailing,
}

/// Why signatures cannot be put together into a multi-signed transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignersError {
    /// None, or more than [`MAX_SIGNERS`].
    Count,
    /// Two of one account.
    Duplicate,
    /// One signs as the sender, which never signs for itself so.
    Sender,
    /// A signature is not its key's on the transaction and its account.
    Signature,
}

/// The hash of a signed transaction's blob: SHA-512Half of `TXN\0‖blob`.
#[must_use]
pub fn hash(blob: &[u8]) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update(HASH_PREFIX);
    hasher.update(blob);
    let digest = hasher.finalize();
    let mut out = [0; 32];
    out.copy_from_slice(digest.get(..32).unwrap_or(&[0; 32]));
    out
}

fn written(tx: &impl Fields, mode: Mode, prefix: &[u8]) -> Vec<u8> {
    tx.check();
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(prefix);
    wire::write(&tx.body(), mode, &mut out);
    out
}

fn signed<T: Fields>(tx: T) -> Signed<T> {
    let blob = written(&tx, Mode::Blob, &[]);
    Signed {
        hash: hash(&blob),
        tx,
        blob,
    }
}

/// What every kind of transaction does: be signed, written and read.
pub trait Transact: Fields {
    /// Who sends it.
    fn sender(&self) -> AccountId {
        self.body().account
    }

    /// The sender's sequence it uses.
    fn sequence(&self) -> u32 {
        self.body().sequence
    }

    /// Its fee, in drops.
    fn fee(&self) -> u64 {
        self.body().fee
    }

    /// The last ledger it may be in, if it names one.
    fn last_ledger_sequence(&self) -> Option<u32> {
        self.body().last_ledger_sequence
    }

    /// Who signs it.
    fn signing(&self) -> &Signing {
        self.body().signing
    }

    /// What one key signs: `STX\0` and the fields, signature and signers
    /// left out.
    fn signing_payload(&self) -> Vec<u8> {
        written(self, Mode::Single, &SIGNING_PREFIX)
    }

    /// What `signer` signs of it, multi-signed: `SMT\0`, the fields with
    /// `SigningPubKey` empty and signature and signers left out, and the
    /// signer's account. Each signer signs alone, so their signatures can be
    /// gathered in any order and at any time.
    fn multi_signing_payload(&self, signer: AccountId) -> Vec<u8> {
        let mut out = written(self, Mode::Multi, &MULTI_SIGNING_PREFIX);
        out.extend_from_slice(&signer.0);
        out
    }

    /// Its binary form, with the signature or the signers it has: what was
    /// submitted, since the form is canonical.
    fn blob(&self) -> Vec<u8> {
        written(self, Mode::Blob, &[])
    }

    /// Signs it with `key`, the one it names.
    ///
    /// # Panics
    ///
    /// If it names another key, or is to be multi-signed, or it holds what
    /// the ledger would refuse: an amount beyond the ledger's or below
    /// zero, a `SendMax` of XRP for XRP, a partial Payment, more memos or
    /// signer entries than the codec writes (bugs: who signs builds what it
    /// signs).
    fn sign<K: Key + ?Sized>(mut self, key: &K) -> Signed<Self> {
        let public_key = key.public_key();
        assert!(
            matches!(self.signing(), Signing::Single { public_key: named, .. } if *named == public_key),
            "signed by the key it names"
        );
        let signature = key.sign(&self.signing_payload());
        *self.signing_mut() = Signing::Single {
            public_key,
            signature: Some(signature),
        };
        signed(self)
    }

    /// The signature of `key` on it, as `signer`, an entry of the sender's
    /// signer list whose master key or regular key `key` is: one part of a
    /// multi-signed transaction ([`with_signers`](Self::with_signers)).
    fn multi_sign<K: Key + ?Sized>(&self, signer: AccountId, key: &K) -> Signer {
        Signer {
            account: signer,
            signing_pub_key: key.public_key(),
            signature: key.sign(&self.multi_signing_payload(signer)),
        }
    }

    /// Whether `signer`'s signature is its key's on this transaction, as
    /// its account. Whether the key may sign for that account is the
    /// ledger's to say, as of when the transaction applies.
    fn verify_signer(&self, signer: &Signer) -> bool {
        signer.signing_pub_key.verify(
            &self.multi_signing_payload(signer.account),
            &signer.signature,
        )
    }

    /// The transaction multi-signed by `signers`, sorted by account as the
    /// ledger requires, ready to submit. Its fee should be at least
    /// [`multi_signed_fee`] for that many.
    ///
    /// # Errors
    ///
    /// The signers cannot sign it together ([`SignersError`]).
    fn with_signers(mut self, mut signers: Vec<Signer>) -> Result<Signed<Self>, SignersError> {
        if signers.is_empty() || signers.len() > MAX_SIGNERS {
            return Err(SignersError::Count);
        }
        signers.sort_unstable_by_key(|signer| signer.account);
        if signers
            .iter()
            .zip(signers.iter().skip(1))
            .any(|(a, b)| a.account == b.account)
        {
            return Err(SignersError::Duplicate);
        }
        if signers.iter().any(|signer| signer.account == self.sender()) {
            return Err(SignersError::Sender);
        }
        if !signers.iter().all(|signer| self.verify_signer(signer)) {
            return Err(SignersError::Signature);
        }
        *self.signing_mut() = Signing::Multi(signers);
        Ok(signed(self))
    }

    /// Whether it is signed, and validly: by the key it names, or by
    /// signers sorted by account, each once, none the sender, and each
    /// signature its key's. Whether those keys may sign for the sender is
    /// the ledger's to say.
    fn signatures_valid(&self) -> bool {
        match self.signing() {
            Signing::Single {
                public_key,
                signature: Some(signature),
            } => public_key.verify(&self.signing_payload(), signature),
            Signing::Single {
                signature: None, ..
            } => false,
            Signing::Multi(signers) => {
                let sender = self.sender();
                let sorted = signers
                    .iter()
                    .zip(signers.iter().skip(1))
                    .all(|(a, b)| a.account < b.account);
                (1..=MAX_SIGNERS).contains(&signers.len())
                    && sorted
                    && signers
                        .iter()
                        .all(|signer| signer.account != sender && self.verify_signer(signer))
            }
        }
    }

    /// The transaction in `bytes`, signed or not, if it is one of this kind
    /// the codec reads: only its fields, each once and in canonical order,
    /// to the last byte.
    ///
    /// # Errors
    ///
    /// Why it is not.
    fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::from_fields(wire::read(bytes)?)
    }
}

/// The common fields of `$tx`, a transaction of `$kind`, and none of the
/// others.
macro_rules! common_body {
    ($tx:expr, $kind:expr) => {
        Body {
            kind: $kind,
            network_id: $tx.network_id,
            flags: $tx.flags,
            sequence: $tx.sequence,
            destination_tag: None,
            last_ledger_sequence: $tx.last_ledger_sequence,
            set_flag: None,
            clear_flag: None,
            signer_quorum: None,
            amount: None,
            limit_amount: None,
            fee: $tx.fee,
            send_max: None,
            signing: &$tx.signing,
            account: $tx.account,
            destination: None,
            regular_key: None,
            signer_entries: &[],
            memos: &$tx.memos,
        }
    };
}

/// Whether `send_max` may go with `amount`: an amount the ledger takes,
/// and never XRP for XRP, which the ledger refuses
/// (`temBAD_SEND_XRP_MAX`).
fn send_max_fits(amount: &Amount, send_max: &Amount) -> bool {
    send_max.is_payable() && !matches!((amount, send_max), (Amount::Xrp(_), Amount::Xrp(_)))
}

const PAYMENT_REQUIRED: u32 =
    wire::COMMON_REQUIRED | bits(&[wire::AMOUNT_FIELD, wire::DESTINATION]);
const PAYMENT_ALLOWED: u32 =
    wire::COMMON_ALLOWED | PAYMENT_REQUIRED | bits(&[wire::SEND_MAX, wire::DESTINATION_TAG]);

impl Fields for Payment {
    fn body(&self) -> Body<'_> {
        Body {
            amount: Some(self.amount),
            send_max: self.send_max,
            destination: Some(self.destination),
            destination_tag: self.destination_tag,
            ..common_body!(self, PAYMENT)
        }
    }

    fn signing_mut(&mut self) -> &mut Signing {
        &mut self.signing
    }

    fn check(&self) {
        assert!(self.amount.is_payable(), "amounts within the ledger's");
        assert!(
            self.send_max
                .is_none_or(|send_max| send_max_fits(&self.amount, &send_max)),
            "a SendMax the ledger takes"
        );
        assert_eq!(
            self.flags & TF_PARTIAL_PAYMENT,
            0,
            "never a partial payment"
        );
    }

    /// Amounts not below zero, a `SendMax` that is not XRP for XRP, and
    /// not partial.
    fn from_fields(mut fields: Decoded) -> Result<Self, DecodeError> {
        fields.expect(PAYMENT, PAYMENT_REQUIRED, PAYMENT_ALLOWED)?;
        let amount = fields.amount.ok_or(DecodeError::Field)?;
        let negative = |amount: &Amount| amount.token().is_some_and(|t| t.value.is_negative());
        if fields.flags & TF_PARTIAL_PAYMENT != 0
            || negative(&amount)
            || fields
                .send_max
                .is_some_and(|send_max| !send_max_fits(&amount, &send_max))
        {
            return Err(DecodeError::Value);
        }
        Ok(Self {
            account: fields.account()?,
            destination: fields.destination.ok_or(DecodeError::Field)?,
            destination_tag: fields.destination_tag,
            amount,
            send_max: fields.send_max,
            fee: fields.fee,
            sequence: fields.sequence,
            last_ledger_sequence: fields.last_ledger_sequence,
            flags: fields.flags,
            network_id: fields.network_id,
            signing: fields.signing()?,
            memos: fields.memos,
        })
    }
}

impl Transact for Payment {}

const TRUST_SET_REQUIRED: u32 = wire::COMMON_REQUIRED | wire::bit(wire::LIMIT_AMOUNT);

impl Fields for TrustSet {
    fn body(&self) -> Body<'_> {
        Body {
            limit_amount: Some(Amount::Token(self.limit)),
            ..common_body!(self, TRUST_SET)
        }
    }

    fn signing_mut(&mut self) -> &mut Signing {
        &mut self.signing
    }

    fn check(&self) {
        assert!(
            self.limit.is_valid() && !self.limit.value.is_negative(),
            "a limit the ledger takes"
        );
    }

    /// A token limit not below zero; no qualities.
    fn from_fields(mut fields: Decoded) -> Result<Self, DecodeError> {
        fields.expect(
            TRUST_SET,
            TRUST_SET_REQUIRED,
            wire::COMMON_ALLOWED | TRUST_SET_REQUIRED,
        )?;
        let Some(Amount::Token(limit)) = fields.limit_amount else {
            return Err(DecodeError::Value);
        };
        if limit.value.is_negative() {
            return Err(DecodeError::Value);
        }
        Ok(Self {
            account: fields.account()?,
            limit,
            fee: fields.fee,
            sequence: fields.sequence,
            last_ledger_sequence: fields.last_ledger_sequence,
            flags: fields.flags,
            network_id: fields.network_id,
            signing: fields.signing()?,
            memos: fields.memos,
        })
    }
}

impl Transact for TrustSet {}

impl Fields for AccountSet {
    fn body(&self) -> Body<'_> {
        Body {
            set_flag: self.set_flag,
            clear_flag: self.clear_flag,
            ..common_body!(self, ACCOUNT_SET)
        }
    }

    fn signing_mut(&mut self) -> &mut Signing {
        &mut self.signing
    }

    fn from_fields(mut fields: Decoded) -> Result<Self, DecodeError> {
        fields.expect(
            ACCOUNT_SET,
            wire::COMMON_REQUIRED,
            wire::COMMON_ALLOWED | bits(&[wire::SET_FLAG, wire::CLEAR_FLAG]),
        )?;
        Ok(Self {
            account: fields.account()?,
            set_flag: fields.set_flag,
            clear_flag: fields.clear_flag,
            fee: fields.fee,
            sequence: fields.sequence,
            last_ledger_sequence: fields.last_ledger_sequence,
            flags: fields.flags,
            network_id: fields.network_id,
            signing: fields.signing()?,
            memos: fields.memos,
        })
    }
}

impl Transact for AccountSet {}

impl Fields for SetRegularKey {
    fn body(&self) -> Body<'_> {
        Body {
            regular_key: self.regular_key,
            ..common_body!(self, SET_REGULAR_KEY)
        }
    }

    fn signing_mut(&mut self) -> &mut Signing {
        &mut self.signing
    }

    fn from_fields(mut fields: Decoded) -> Result<Self, DecodeError> {
        fields.expect(
            SET_REGULAR_KEY,
            wire::COMMON_REQUIRED,
            wire::COMMON_ALLOWED | wire::bit(wire::REGULAR_KEY),
        )?;
        Ok(Self {
            account: fields.account()?,
            regular_key: fields.regular_key,
            fee: fields.fee,
            sequence: fields.sequence,
            last_ledger_sequence: fields.last_ledger_sequence,
            flags: fields.flags,
            network_id: fields.network_id,
            signing: fields.signing()?,
            memos: fields.memos,
        })
    }
}

impl Transact for SetRegularKey {}

const SIGNER_LIST_SET_REQUIRED: u32 = wire::COMMON_REQUIRED | wire::bit(wire::SIGNER_QUORUM);

impl Fields for SignerListSet {
    fn body(&self) -> Body<'_> {
        Body {
            signer_quorum: Some(self.signer_quorum),
            signer_entries: &self.signer_entries,
            ..common_body!(self, SIGNER_LIST_SET)
        }
    }

    fn signing_mut(&mut self) -> &mut Signing {
        &mut self.signing
    }

    /// Any quorum and entries the codec can hold: whether the ledger takes
    /// them is its to say ([`check_signer_list`]).
    fn from_fields(mut fields: Decoded) -> Result<Self, DecodeError> {
        fields.expect(
            SIGNER_LIST_SET,
            SIGNER_LIST_SET_REQUIRED,
            wire::COMMON_ALLOWED | SIGNER_LIST_SET_REQUIRED | wire::bit(wire::SIGNER_ENTRIES),
        )?;
        Ok(Self {
            account: fields.account()?,
            signer_quorum: fields.signer_quorum.ok_or(DecodeError::Field)?,
            signer_entries: core::mem::take(&mut fields.signer_entries),
            fee: fields.fee,
            sequence: fields.sequence,
            last_ledger_sequence: fields.last_ledger_sequence,
            flags: fields.flags,
            network_id: fields.network_id,
            signing: fields.signing()?,
            memos: fields.memos,
        })
    }
}

impl Transact for SignerListSet {}

impl Fields for Transaction {
    fn body(&self) -> Body<'_> {
        match self {
            Self::Payment(tx) => tx.body(),
            Self::TrustSet(tx) => tx.body(),
            Self::AccountSet(tx) => tx.body(),
            Self::SetRegularKey(tx) => tx.body(),
            Self::SignerListSet(tx) => tx.body(),
        }
    }

    fn signing_mut(&mut self) -> &mut Signing {
        match self {
            Self::Payment(tx) => tx.signing_mut(),
            Self::TrustSet(tx) => tx.signing_mut(),
            Self::AccountSet(tx) => tx.signing_mut(),
            Self::SetRegularKey(tx) => tx.signing_mut(),
            Self::SignerListSet(tx) => tx.signing_mut(),
        }
    }

    fn check(&self) {
        match self {
            Self::Payment(tx) => tx.check(),
            Self::TrustSet(tx) => tx.check(),
            Self::AccountSet(tx) => tx.check(),
            Self::SetRegularKey(tx) => tx.check(),
            Self::SignerListSet(tx) => tx.check(),
        }
    }

    /// Whichever kind it is.
    fn from_fields(fields: Decoded) -> Result<Self, DecodeError> {
        match fields.kind {
            PAYMENT => Payment::from_fields(fields).map(Self::Payment),
            TRUST_SET => TrustSet::from_fields(fields).map(Self::TrustSet),
            ACCOUNT_SET => AccountSet::from_fields(fields).map(Self::AccountSet),
            SET_REGULAR_KEY => SetRegularKey::from_fields(fields).map(Self::SetRegularKey),
            SIGNER_LIST_SET => SignerListSet::from_fields(fields).map(Self::SignerListSet),
            _ => Err(DecodeError::Value),
        }
    }
}

impl Transact for Transaction {}

/// Why a signer list is none the ledger would set, with the code it
/// refuses it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignerListError {
    /// No entries, or more than [`MAX_SIGNERS`] (`temMALFORMED`).
    Count,
    /// An entry of no weight (`temBAD_WEIGHT`).
    Weight,
    /// An entry twice, or the account itself (`temBAD_SIGNER`).
    Signer,
    /// A quorum of zero, or above what all entries weigh together
    /// (`temBAD_QUORUM`).
    Quorum,
}

impl SignerListError {
    /// The code the ledger refuses the list with.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Count => "temMALFORMED",
            Self::Weight => "temBAD_WEIGHT",
            Self::Signer => "temBAD_SIGNER",
            Self::Quorum => "temBAD_QUORUM",
        }
    }
}

/// Whether `account` may set a signer list of `entries` with `quorum`, as
/// the ledger checks it before anything else.
///
/// # Errors
///
/// Why not ([`SignerListError`]).
pub fn check_signer_list(
    account: AccountId,
    quorum: u32,
    entries: &[SignerEntry],
) -> Result<(), SignerListError> {
    if entries.is_empty() || entries.len() > MAX_SIGNERS {
        return Err(SignerListError::Count);
    }
    if entries.iter().any(|entry| entry.weight == 0) {
        return Err(SignerListError::Weight);
    }
    let twice = entries.iter().enumerate().any(|(index, entry)| {
        entries
            .iter()
            .skip(index.saturating_add(1))
            .any(|other| other.account == entry.account)
    });
    if twice || entries.iter().any(|entry| entry.account == account) {
        return Err(SignerListError::Signer);
    }
    let total: u32 = entries.iter().map(|entry| u32::from(entry.weight)).sum();
    if quorum == 0 || total < quorum {
        return Err(SignerListError::Quorum);
    }
    Ok(())
}

/// A Payment that delivers exactly `amount` and spends at most `send_max`,
/// through the ledger's default paths: a swap through an AMM pool, when
/// the two are XRP and a token ([`sign_send_max`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendMaxTerms {
    /// Who is paid: another account, or the payer itself for a swap.
    pub destination: AccountId,
    /// What it delivers, all of it or nothing.
    pub amount: Amount,
    /// The most it spends.
    pub send_max: Amount,
    /// The fee, in drops.
    pub fee: u64,
    /// The payer's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in.
    pub last_ledger_sequence: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
}

/// Why terms are no Payment the ledger would take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermsError {
    /// The amount is zero, below zero or beyond the ledger's.
    Amount,
    /// `SendMax` is zero, below zero or beyond the ledger's, or XRP for
    /// XRP.
    SendMax,
    /// The fee is beyond all the XRP there is.
    Fee,
    /// More memos than [`MAX_MEMOS`], or one longer than
    /// [`MAX_MEMO_BYTES`].
    Memos,
    /// It pays the payer the same asset it spends.
    Redundant,
}

/// Signs, with `wallet`, a Payment from its account on `terms`: one that
/// delivers all of `terms.amount` or fails, spending at most
/// `terms.send_max`, with no paths and no flags, so the ledger converts
/// through its default paths and never pays part of it. Who calls it
/// keeps the sequence: this signs, and sends nothing.
///
/// # Errors
///
/// Why `terms` are no Payment the ledger would take ([`TermsError`]).
pub fn sign_send_max(wallet: &Wallet, terms: SendMaxTerms) -> Result<Signed<Payment>, TermsError> {
    if terms.amount.is_zero() || !terms.amount.is_payable() {
        return Err(TermsError::Amount);
    }
    if terms.send_max.is_zero() || !send_max_fits(&terms.amount, &terms.send_max) {
        return Err(TermsError::SendMax);
    }
    if terms.fee > crate::MAX_DROPS {
        return Err(TermsError::Fee);
    }
    let long = |memo: &Memo| {
        memo.memo_type.len() > MAX_MEMO_BYTES || memo.memo_data.len() > MAX_MEMO_BYTES
    };
    if terms.memos.len() > MAX_MEMOS || terms.memos.iter().any(long) {
        return Err(TermsError::Memos);
    }
    let account = wallet.account();
    if terms.destination == account && terms.amount.asset() == terms.send_max.asset() {
        return Err(TermsError::Redundant);
    }
    let payment = Payment {
        account,
        destination: terms.destination,
        destination_tag: None,
        amount: terms.amount,
        send_max: Some(terms.send_max),
        fee: terms.fee,
        sequence: terms.sequence,
        last_ledger_sequence: Some(terms.last_ledger_sequence),
        flags: 0,
        network_id: terms.network_id,
        memos: terms.memos,
        signing: Signing::by(wallet.public_key()),
    };
    Ok(payment.sign(wallet))
}

#[cfg(test)]
mod tests;
