//! The escrow's transactions on the XRP Ledger
//! (spec/designs/settlement-hubs.md#2-founding-a-hub,
//! spec/designs/settlement-hubs.md#6-deposits-and-withdrawals): the
//! sponsor's setup, the check the validators make of it, the payments that
//! carry withdrawals out, and the change of signer list when the validators
//! change.
//!
//! A token hub's escrow holds its token on one trust line to the issuer,
//! which the setup opens before the master key is disabled, and pays its
//! fees from its own XRP: the gas ([`Terms::transaction_fee`]).
//!
//! Every validator builds the same transaction from the same block, so each
//! signs alone and the signatures fit together. None has a last ledger: a
//! withdrawal stays valid until it applies, and its sequence was given to
//! it by the log.

use alloc::vec::Vec;

use dassie_xrpl::api::{AccountObjects, AccountSettings, TrustLine};
use dassie_xrpl::codec::{
    ASF_DISABLE_MASTER, ASF_REQUIRE_DEST, AccountSet, Payment, Signed, SignerEntry, SignerListSet,
    Signing, TF_SET_NO_RIPPLE, TokenAmount, TokenValue, Transact, TrustSet,
};
use dassie_xrpl::keys::PublicKey;
use dassie_xrpl::{AccountId, Wallet};

use crate::ledger::Withdrawal;
use crate::rules::{Asset, Rules, Terms};

/// The network ID a transaction names: the ledger requires it above 1024,
/// and refuses it below.
#[must_use]
pub const fn network_id(network: u32) -> Option<u32> {
    if network > 1024 { Some(network) } else { None }
}

/// The escrow's account.
#[must_use]
pub fn escrow_account(rules: &Rules) -> AccountId {
    AccountId(rules.terms.escrow)
}

/// The signer list of a validator set: each validator's escrow key, weight
/// 1, sorted by account as the ledger keeps them.
#[must_use]
pub fn signer_entries(rules: &Rules) -> Vec<SignerEntry> {
    let mut entries: Vec<SignerEntry> = rules
        .validators
        .iter()
        .filter_map(|acceptance| PublicKey::from_bytes(acceptance.escrow))
        .map(|key| SignerEntry {
            account: key.account(),
            weight: 1,
        })
        .collect();
    entries.sort_unstable_by_key(|entry| entry.account);
    entries
}

/// The quorum of the escrow's signer list: the validators' q.
#[must_use]
pub fn signer_quorum(rules: &Rules) -> u32 {
    let n = rules.validators.len();
    u32::try_from(n.saturating_sub(n.saturating_sub(1) / 3)).unwrap_or(u32::MAX)
}

/// The change of the escrow's signer list to `rules`' validators, at
/// `sequence`, to be multi-signed by the set before them.
#[must_use]
pub fn signer_list_set(rules: &Rules, sequence: u32) -> SignerListSet {
    SignerListSet {
        account: escrow_account(rules),
        signer_quorum: signer_quorum(rules),
        signer_entries: signer_entries(rules),
        fee: rules.terms.transaction_fee(),
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: network_id(rules.terms.network),
        memos: Vec::new(),
        signing: Signing::multi(),
    }
}

/// The Payment that carries withdrawal `sequence` out of the escrow,
/// multi-signed by the validators: of XRP, or of the hub's token, all of
/// its amount or nothing, never partial. Its fee is the ledger fee the
/// withdrawal paid in an XRP hub, and the gas in a token hub, which the
/// escrow's own XRP pays.
#[must_use]
pub fn withdrawal_payment(rules: &Rules, sequence: u32, withdrawal: &Withdrawal) -> Payment {
    let fee = match rules.terms.asset {
        Asset::Xrp => withdrawal.ledger_fee,
        Asset::Token { .. } => rules.terms.transaction_fee(),
    };
    Payment {
        account: escrow_account(rules),
        destination: AccountId(withdrawal.to.account),
        destination_tag: withdrawal.to.tag,
        amount: rules.terms.ledger_amount(withdrawal.amount),
        send_max: None,
        fee,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: network_id(rules.terms.network),
        memos: Vec::new(),
        signing: Signing::multi(),
    }
}

/// The sponsor's setup of the escrow, signed by its master key, in the
/// order it must apply from `sequence` on: the signer list, then
/// `RequireDest`, then for a token hub the trust line to the token's
/// issuer, then the master key disabled, last. `fee` is the ledger's for a
/// single-signed transaction. The escrow is funded first, by an ordinary
/// payment that creates it, with XRP enough for the reserves of the
/// account, its signer list and its line, and the fees.
#[must_use]
pub fn setup(rules: &Rules, master: &Wallet, sequence: u32, fee: u64) -> Vec<Transaction> {
    let kinds = [
        Some(SetupKind::List),
        Some(SetupKind::RequireDest),
        is_token(&rules.terms).then_some(SetupKind::Trust),
        Some(SetupKind::DisableMaster),
    ];
    sequenced(rules, master, kinds.into_iter().flatten(), sequence, fee)
}

/// The setup transactions of `kinds`, in order from `sequence` on.
fn sequenced(
    rules: &Rules,
    master: &Wallet,
    kinds: impl Iterator<Item = SetupKind>,
    sequence: u32,
    fee: u64,
) -> Vec<Transaction> {
    let mut next = sequence;
    let mut transactions = Vec::new();
    for kind in kinds {
        if let Some(transaction) = setup_transaction(rules, master, kind, next, fee) {
            transactions.push(transaction);
            next = next.saturating_add(1);
        }
    }
    transactions
}

/// A signed setup transaction of any kind.
#[derive(Clone, Debug)]
pub enum Transaction {
    /// The signer list.
    List(Signed<SignerListSet>),
    /// A setting.
    Set(Signed<AccountSet>),
    /// The trust line to a token hub's issuer.
    Trust(Signed<TrustSet>),
}

impl Transaction {
    /// The blob to submit.
    #[must_use]
    pub fn blob(&self) -> &[u8] {
        match self {
            Self::List(signed) => &signed.blob,
            Self::Set(signed) => &signed.blob,
            Self::Trust(signed) => &signed.blob,
        }
    }
}

/// Whether the hub holds a token.
const fn is_token(terms: &Terms) -> bool {
    matches!(terms.asset, Asset::Token { .. })
}

/// Whether `line` is the one a token hub's escrow may have beside its
/// signer list, set up or not: its line to the token's issuer, in the
/// token's currency. An XRP hub's escrow has none.
fn is_hub_line(terms: &Terms, line: &TrustLine) -> bool {
    let Asset::Token { currency, issuer } = terms.asset else {
        return false;
    };
    line.is_between(AccountId(terms.escrow), issuer, currency)
}

/// Whether `line` is a token hub's escrow's line as its setup makes it:
/// rippling disabled on the escrow's side, so no payment ripples through
/// it, the largest limit there is, so no deposit is refused for it, and no
/// freeze of the escrow's. That it pays and takes at par on both sides,
/// with no quality that would skew what a withdrawal delivers or a deposit
/// adds, is the reading's: a line with one is not among the lines read
/// ([`AccountObjects::lines`]).
pub(crate) fn is_escrow_line(terms: &Terms, line: &TrustLine) -> bool {
    is_hub_line(terms, line)
        && line.view(AccountId(terms.escrow)).is_some_and(|view| {
            view.ours.no_ripple
                && !view.ours.freeze
                && !view.ours.deep_freeze
                && view.limit == TokenValue::MAX
        })
}

/// Why an escrow is not one the validators take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscrowFault {
    /// It has no account, as a validated ledger shows it.
    NoAccount,
    /// What was read is not as of a validated ledger, or the objects were
    /// read as of an earlier ledger than the settings.
    NotValidated,
    /// Its master key still signs.
    MasterKey,
    /// It has a regular key.
    RegularKey,
    /// Payments without a destination tag reach it.
    NoRequireDest,
    /// Its signer list is not the rules'.
    SignerList,
    /// A token hub's escrow has no line to the issuer, or one not as its
    /// setup makes it.
    TrustLine,
    /// It owns other objects, or more than one page of them: for a token
    /// hub, anything but its signer list and its one line to the issuer.
    Objects,
}

/// The validators' check of the escrow before the first block, and after a
/// reset (spec/designs/settlement-hubs.md#2-founding-a-hub): the master key
/// disabled, no regular key, the signer list exactly as the rules say,
/// `RequireDest` set, and no other objects but, for a token hub, exactly
/// one: its line to the issuer, rippling disabled on its side and the
/// largest limit. A sponsor that kept a way in is caught here.
pub fn check_escrow(
    rules: &Rules,
    settings: &AccountSettings,
    objects: &AccountObjects,
) -> Result<(), EscrowFault> {
    // From the ledger where the master key is disabled and no regular key
    // is set, only the signer list can add objects: the objects must be
    // read as of that ledger or later, or the master key could have made
    // one in between.
    if !settings.validated || !objects.validated || objects.ledger_index < settings.ledger_index {
        return Err(EscrowFault::NotValidated);
    }
    if !settings.master_disabled() {
        return Err(EscrowFault::MasterKey);
    }
    if settings.regular_key.is_some() {
        return Err(EscrowFault::RegularKey);
    }
    if !settings.requires_destination_tag() {
        return Err(EscrowFault::NoRequireDest);
    }
    if foreign_objects(&rules.terms, objects) {
        return Err(EscrowFault::Objects);
    }
    let list = objects
        .signer_list
        .as_ref()
        .ok_or(EscrowFault::SignerList)?;
    if list.quorum != signer_quorum(rules) || list.entries != signer_entries(rules) {
        return Err(EscrowFault::SignerList);
    }
    if is_token(&rules.terms) && !line_set_up(&rules.terms, objects) {
        return Err(EscrowFault::TrustLine);
    }
    Ok(())
}

/// Whether the escrow owns anything it may not: another page of objects,
/// or beside its signer list anything but a token hub's one line to the
/// issuer, which an XRP hub's escrow does not have.
fn foreign_objects(terms: &Terms, objects: &AccountObjects) -> bool {
    let hub_lines = objects
        .lines
        .iter()
        .filter(|line| is_hub_line(terms, line))
        .count();
    objects.marker.is_some() || hub_lines > 1 || objects.others > hub_lines
}

/// Whether the escrow's line to a token hub's issuer is there, as its
/// setup makes it.
fn line_set_up(terms: &Terms, objects: &AccountObjects) -> bool {
    objects.lines.iter().any(|line| is_escrow_line(terms, line))
}

/// What the sponsor does next to set up the escrow, from what the ledger
/// shows of it (spec/designs/settlement-hubs.md#2-founding-a-hub).
#[derive(Clone, Debug)]
pub enum SetupStep {
    /// The account does not exist yet: fund it, with a payment that
    /// creates it.
    Fund,
    /// What was read is not as of a validated ledger: read it again.
    Wait,
    /// Submit these, signed by the master key, in order.
    Submit(Vec<Transaction>),
    /// It is set up as the rules say.
    Done,
    /// It is not, and the master key can no longer fix it, or never made
    /// what is wrong: a regular key, or objects other than its signer list.
    Broken(EscrowFault),
}

/// What the escrow's account shows, as of a validated ledger.
#[derive(Clone, Debug)]
pub struct EscrowSeen {
    /// The sequence its next transaction uses.
    pub sequence: u32,
    /// Its settings.
    pub settings: AccountSettings,
    /// What it owns.
    pub objects: AccountObjects,
}

/// The sponsor's next step in setting up the escrow of `rules`, whose
/// master key is `master`, from what the ledger shows of it (`None`: no
/// such account). The setup transactions it is still missing go from the
/// account's sequence on, the master key disabled last; `fee` is the
/// ledger's for one signature.
#[must_use]
pub fn setup_step(
    rules: &Rules,
    master: &Wallet,
    seen: Option<&EscrowSeen>,
    fee: u64,
) -> SetupStep {
    let Some(seen) = seen else {
        return SetupStep::Fund;
    };
    let (settings, objects) = (&seen.settings, &seen.objects);
    if !settings.validated || !objects.validated {
        return SetupStep::Wait;
    }
    if settings.master_disabled() {
        return match check_escrow(rules, settings, objects) {
            Ok(()) => SetupStep::Done,
            Err(EscrowFault::NotValidated) => SetupStep::Wait,
            Err(fault) => SetupStep::Broken(fault),
        };
    }
    if settings.regular_key.is_some() {
        return SetupStep::Broken(EscrowFault::RegularKey);
    }
    if foreign_objects(&rules.terms, objects) {
        return SetupStep::Broken(EscrowFault::Objects);
    }
    let listed = objects.signer_list.as_ref().is_some_and(|list| {
        list.quorum == signer_quorum(rules) && list.entries == signer_entries(rules)
    });
    let lined = !is_token(&rules.terms) || line_set_up(&rules.terms, objects);
    let missing = [
        (!listed).then_some(SetupKind::List),
        (!settings.requires_destination_tag()).then_some(SetupKind::RequireDest),
        (!lined).then_some(SetupKind::Trust),
        Some(SetupKind::DisableMaster),
    ];
    SetupStep::Submit(sequenced(
        rules,
        master,
        missing.into_iter().flatten(),
        seen.sequence,
        fee,
    ))
}

/// One of the setup's transactions.
#[derive(Clone, Copy)]
enum SetupKind {
    List,
    RequireDest,
    Trust,
    DisableMaster,
}

/// The setup transaction of `kind`; `None` for a line of an XRP hub,
/// which has none.
fn setup_transaction(
    rules: &Rules,
    master: &Wallet,
    kind: SetupKind,
    sequence: u32,
    fee: u64,
) -> Option<Transaction> {
    let flag = |flag| AccountSet {
        account: escrow_account(rules),
        set_flag: Some(flag),
        clear_flag: None,
        fee,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: network_id(rules.terms.network),
        memos: Vec::new(),
        signing: Signing::by(master.public_key()),
    };
    Some(match kind {
        SetupKind::List => Transaction::List(
            SignerListSet {
                fee,
                signing: Signing::by(master.public_key()),
                ..signer_list_set(rules, sequence)
            }
            .sign(master),
        ),
        SetupKind::RequireDest => Transaction::Set(flag(ASF_REQUIRE_DEST).sign(master)),
        SetupKind::Trust => {
            Transaction::Trust(trust_set(rules, master, sequence, fee)?.sign(master))
        }
        SetupKind::DisableMaster => Transaction::Set(flag(ASF_DISABLE_MASTER).sign(master)),
    })
}

/// The escrow's `TrustSet` to a token hub's issuer: the largest limit, and
/// rippling disabled on its side. `None` for an XRP hub.
fn trust_set(rules: &Rules, master: &Wallet, sequence: u32, fee: u64) -> Option<TrustSet> {
    let Asset::Token { currency, issuer } = rules.terms.asset else {
        return None;
    };
    Some(TrustSet {
        account: escrow_account(rules),
        limit: TokenAmount {
            value: TokenValue::MAX,
            currency,
            issuer,
        },
        fee,
        sequence,
        last_ledger_sequence: None,
        flags: TF_SET_NO_RIPPLE,
        network_id: network_id(rules.terms.network),
        memos: Vec::new(),
        signing: Signing::by(master.public_key()),
    })
}
