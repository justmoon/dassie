//! A hub's settlement scheme (spec/areas/hubs.md#the-members-side),
//! sans-IO: a member's side of peering over a settlement hub, through its
//! [`HubAccount`]. Its ID and ledger name the hub's underlying ledger and
//! its asset (spec/decisions/hub-scheme-names.md), as the hub's rules give
//! it ([`scheme_of`]): `hub-xrpl-testnet-xrp` for XRP, `hub-xrpl-testnet-dsusd`
//! for DSUSD. Amounts are in the asset's smallest unit, drops of XRP or
//! millionths of the token, which the books count alike
//! ([`UNITS_PER_DROP`]).
//!
//! - **Peering** exchanges the hub's ID and each side's account number and
//!   key; a peer in another hub is refused.
//! - **Settling is funding the hold** toward the peer: the first settlement
//!   opens it, each later one adds to it, sent once the node executed it,
//!   and finalized once an agreed statement shows it applied. Settling stays
//!   on request (spec/areas/settlement.md#settling-on-request): the payee
//!   asks when its room runs low, and the payer funds by what it is asked.
//! - **What it books.** Funding a hold is our outgoing settlement and the
//!   peer's incoming one, which the peer books once agreed statements show
//!   its hold grew; a hold's reserve is the ledger's, and never booked. A
//!   redeem or a claim moves money within the payee's settlement asset, and
//!   books only its fee. What a peering costs beyond its hold is a loss on
//!   the peer ([`Effect::PeerLoss`]). Whatever else changes the balance is
//!   reconciled against the account as agreed, through suspense.
//! - **Its promises** ([`Commitments`]): see `payer.rs` and `payee.rs`.
//!
//! - **The owner's moves.** A deposit is credited by the validators'
//!   attestations, a withdrawal debits the account, and a withdrawal that
//!   comes back is credited again: the account's state counts each, and
//!   the scheme books their growth as the owner's deposits and withdrawals
//!   ([`SettlementScheme::pay_owner`] submits a withdrawal).
//!
//! - **Joining by being paid** (spec/designs/settlement-hubs.md#5-joining).
//!   A scheme whose key the hub has no account for runs all the same, once
//!   f + 1 validators agree there is none: it holds nothing, presents its
//!   key with no account at peering, and peers with members that have
//!   one. With such a peer no packet carries promises
//!   ([`Commitments::commits_with`]), and a settlement to it is a `pay` of
//!   at least the hub's minimum for a new account to its key, which opens
//!   its account: the payer's outgoing settlement once the hub executed
//!   it, and the payee's incoming one once its account reads the `pay`,
//!   in order, from where the hub last said it had none. Its account then
//!   sets its disbursal address first, and the peering goes on as any
//!   other.
//!
//! [`deposit`](SettlementScheme::deposit) is unsupported: the owner
//! deposits from the underlying ledger.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use dassie_identity::{NodeKey, PublicKey};
use dassie_ledger::{LedgerId, Name};
use dassie_oer::{Encode, Reader};
use dassie_settlement::policy::redeem_threshold;
use dassie_settlement::{
    AssetId, Commitments, Decided, Effect, ExternalAsset, LedgerStatus, LossCause, OwnerPayment,
    OwnerPaymentState, PaymentFailure, PreparedSettlement, Promised, ReconcileCause, SchemeError,
    SettlementScheme,
};
use rand_core::CryptoRng;

use super::account::{HubAccount, News, Purpose};
use super::payee::{Claiming, Payee};
use super::payer::{Loss, Payer, Snapshot};
use super::peering::{Membership, NONE};
use super::{UNITS_PER_DROP, digest_name, units};
use crate::client::AccountView;
use crate::digest::Digest;
use crate::ledger::TESTNET_NETWORK;
use crate::ledger::{Detail, Outcome};
use crate::operation::{AccountNumber, MemberOperation, Operation, SignedMemberOperation};
use crate::promise::{Claim, Commitment, HoldNumber, Signed};
use crate::rules::{Amount, Asset, HubId, Terms};

/// The ID of the scheme over a hub of XRP on the XRP Ledger testnet.
pub const SCHEME_ID: &str = "hub-xrpl-testnet-xrp";

/// The ID of the scheme over a hub of DSUSD on the XRP Ledger testnet.
pub const DSUSD_SCHEME_ID: &str = "hub-xrpl-testnet-dsusd";

/// The scheme over a hub of `terms`, and its ledger: XRP gives
/// [`SCHEME_ID`], DSUSD [`DSUSD_SCHEME_ID`], both on the XRP Ledger
/// testnet. `None` for any other asset or network, which no scheme runs:
/// the IDs on mainnet are not built yet (spec/roadmap.md#mainnet-hubs).
#[must_use]
pub fn scheme_of(terms: &Terms) -> Option<(&'static str, LedgerId)> {
    if terms.network != TESTNET_NETWORK {
        return None;
    }
    match terms.asset {
        Asset::Xrp => Some((SCHEME_ID, LedgerId::HubXrplTestnetXrp)),
        Asset::Token { currency, issuer }
            if currency == dassie_xrpl::dsusd::CURRENCY && issuer == dassie_xrpl::dsusd::ISSUER =>
        {
            Some((DSUSD_SCHEME_ID, LedgerId::HubXrplTestnetDsusdUsd))
        }
        Asset::Token { .. } => None,
    }
}

/// Most peers over the hub.
pub const MAX_PEERS: usize = 256;
/// Most settlements prepared and not yet applied.
const MAX_SETTLING: usize = 64;
/// Most holds opened toward us by accounts not yet our peers, kept so a
/// peering made just after its peer opened its hold still finds it.
const MAX_STRANGERS: usize = 64;
/// Most losses waiting to be booked.
const MAX_LOSSES: usize = MAX_PEERS * super::payer::MAX_PACKETS;
/// Most waits decided and not yet taken.
const MAX_DECIDED: usize = MAX_PEERS * super::payer::MAX_PACKETS;
/// How often the scheme asks the hub for the account of a peer that has
/// none in its records, so it learns of one a deposit opened without
/// paying the peer in first. Reads cost nothing.
pub const LOOK_UP_EVERY_MS: u64 = 60_000;
/// How long the scheme waits after its `pay` to a peer failed before it
/// pays that peer in again: each try costs an operation's fee, and what
/// failed it, as a full account table, does not pass at once.
pub const PAY_AGAIN_MS: u64 = 60_000;

/// A peer over the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Peer {
    /// Its account, or [`NONE`] until the hub has one for its key.
    pub(super) account: AccountNumber,
    pub(super) key: PublicKey,
    /// Our hold toward it.
    pub(super) payer: Payer,
    /// Its hold toward us.
    pub(super) payee: Payee,
    /// While it has no account in our records: when to ask the hub for
    /// one next. Not kept across restarts: a restart asks at once.
    pub(super) look_up_ms: u64,
    /// While it has no account in our records: before when not to pay it
    /// in, since our last `pay` to it failed. Not kept across restarts.
    pub(super) pay_after_ms: u64,
}

impl Peer {
    /// Whether its records hold anything not yet settled: a hold with funds
    /// either way, a packet of ours in flight or waiting for the hub, or a
    /// packet of its in flight or fulfilled that no claim covers yet.
    pub(super) fn unresolved(&self) -> bool {
        let ours = (self.payer.hold.is_some() && self.payer.snapshot.amount > 0)
            || self.payer.has_open_packets();
        let theirs = (self.payee.hold.is_some() && self.payee.snapshot.amount > 0)
            || self.payee.has_unclaimed();
        ours || theirs
    }

    /// Whether it presents itself as the member `membership` names: by its
    /// key, and its account unless one of the two was made before the hub
    /// had one for the key, which never names another account.
    fn is(&self, membership: &Membership) -> bool {
        self.key == membership.key
            && (self.account == membership.account
                || self.account == NONE
                || membership.account == NONE)
    }
}

/// A settlement of ours not yet applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settling {
    pub(super) peer: Name,
    pub(super) drops: Amount,
}

/// Our `pay` whose sequence the account used while the history of what it
/// did was gone (spec/areas/hubs.md#when-the-history-is-gone): whether it
/// applied, the hub's account for the key it paid in says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UnreadPay {
    /// The key it paid in.
    pub(super) key: PublicKey,
    /// The block the account read anew as of: the `pay` had executed by
    /// then.
    pub(super) height: u64,
}

/// The scheme over one hub.
pub struct HubScheme {
    /// Its ID and ledger, which the hub's asset gives ([`scheme_of`]).
    id: Name,
    ledger: LedgerId,
    /// The account it settles through.
    asset: AssetId,
    pub(super) hub: HubId,
    pub(super) key: Arc<NodeKey>,
    pub(super) terms: Terms,
    /// The largest packet on the ledger, in internal units: what the
    /// threshold to redeem is counted in.
    pub(super) max_packet: u64,
    // Kept with the books.
    pub(super) booked: u64,
    pub(super) synced: bool,
    pub(super) cursor: u64,
    pub(super) account: Option<AccountNumber>,
    pub(super) peers: BTreeMap<Name, Peer>,
    pub(super) settling: BTreeMap<Name, Settling>,
    /// What the account had been credited by deposits, debited by
    /// withdrawals, and credited by withdrawals that came back, as of the
    /// last state the books were matched to.
    pub(super) counters: Option<(Amount, Amount, Amount)>,
    // Learnt.
    reserve: Amount,
    /// The account reserve as of the last state the books were matched to.
    account_reserve: Option<Amount>,
    /// The owner's last withdrawal, by its number, and what became of it.
    pub(super) withdrawal: Option<(u64, OwnerPaymentState)>,
    /// How many withdrawals the owner made: the next one's number.
    pub(super) withdrawals_made: u64,
    /// What our withdrawals took that the scheme booked when the hub
    /// executed them (what each paid out, as the owner's, and its ledger
    /// fee, as a fee), which the account's withdrawals counter has not
    /// shown yet: that much of its growth is booked already.
    pub(super) booked_ahead: Amount,
    strangers: BTreeMap<HoldNumber, SignedMemberOperation>,
    /// Our settlements that are a `pay` whose outcome the account never
    /// read, by settlement: still on their way until the hub says whether
    /// it has an account for the key. Kept with the books.
    pub(super) unread_pays: BTreeMap<Name, UnreadPay>,
    losses: VecDeque<(Name, Loss)>,
    decided: VecDeque<Decided>,
    now_ms: u64,
}

/// The account `asset` is, if it is one.
fn account_of(asset: &mut dyn ExternalAsset) -> Option<&mut HubAccount> {
    let any: &mut dyn Any = asset;
    any.downcast_mut()
}

/// A loss's ID: a hash of the operation that caused it, its cause, the
/// hold and the packet's sequence on it, so that each packet's loss is
/// booked once, however many one operation paid.
pub(super) fn loss_id(loss: &Loss) -> Name {
    use sha2::{Digest as _, Sha256};
    let cause: u8 = match loss.cause {
        LossCause::ClaimFee => 0,
        LossCause::PaidLate => 1,
        LossCause::Cheated => 2,
    };
    let mut hash = Sha256::new();
    hash.update(b"dassie-hub/loss");
    hash.update(loss.operation.as_bytes());
    hash.update([cause]);
    hash.update(loss.hold.0.to_be_bytes());
    hash.update(loss.sequence.to_be_bytes());
    let bytes: [u8; 32] = hash.finalize().into();
    Name::hex(&bytes, false)
}

/// The ID of the ledger fee of our withdrawal `operation`: never a hash,
/// so never the ID of the operation's own fee.
fn ledger_fee_id(operation: &Digest) -> Name {
    let mut bytes = [0; 31];
    bytes.copy_from_slice(operation.as_bytes().get(..31).unwrap_or(&[0; 31]));
    let hex = Name::hex(&bytes, false);
    let mut text = alloc::string::String::from(hex.as_str());
    text.push_str("lf");
    Name::new(&text).unwrap_or(hex)
}

/// The ID of the owner's withdrawal `number`.
fn withdrawal_name(number: u64) -> Name {
    Name::hex(&number.to_be_bytes(), false)
}

/// The hash of a member's operation, as the journal keys what it moved.
fn operation_hash(operation: &SignedMemberOperation) -> Digest {
    Operation::Member(operation.clone()).hash()
}

impl HubScheme {
    /// The scheme that settles through `account`, whose largest packet on
    /// the ledger is `max_packet`, with what the books last checkpointed
    /// and the state they kept.
    ///
    /// # Errors
    ///
    /// If the hub holds an asset no scheme runs over ([`scheme_of`]), or the
    /// state is not one the scheme wrote, or is not in step with the
    /// account's.
    pub fn new(
        account: &HubAccount,
        max_packet: u64,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let (id, ledger) = scheme_of(account.terms()).ok_or(SchemeError::InvalidConfig)?;
        let mut scheme = Self {
            id: Name::new(id).map_err(|_| SchemeError::InvalidConfig)?,
            ledger,
            asset: ExternalAsset::id(account),
            hub: account.hub(),
            key: account.signer(),
            terms: account.terms().clone(),
            max_packet,
            booked: checkpoint.unwrap_or(0),
            synced: false,
            cursor: 0,
            account: None,
            peers: BTreeMap::new(),
            settling: BTreeMap::new(),
            counters: None,
            reserve: 0,
            account_reserve: None,
            withdrawal: None,
            withdrawals_made: 0,
            booked_ahead: 0,
            strangers: BTreeMap::new(),
            unread_pays: BTreeMap::new(),
            losses: VecDeque::new(),
            decided: VecDeque::new(),
            now_ms: 0,
        };
        if !state.is_empty() {
            super::state::restore_scheme(&mut scheme, state).ok_or(SchemeError::InvalidData)?;
        }
        // The two are written in the same batch: a scheme behind its
        // account would miss what the account read.
        if scheme.cursor != account.cursor() || scheme.account != account.number() {
            return Err(SchemeError::InvalidData);
        }
        Ok(scheme)
    }

    /// The hub's margin M: how much earlier than its Prepare a commitment
    /// expires.
    const fn margin(&self) -> u64 {
        self.terms.timers.margin
    }

    /// Takes one piece of news from the account.
    fn tell(&mut self, account: &mut HubAccount, news: &News, effects: &mut Vec<Effect>) {
        match news {
            News::Account { height, view } => {
                self.cursor = self.cursor.max(*height);
                self.match_balance(account, view, effects);
            }
            News::Executed {
                height,
                operation,
                outcome,
                ours,
                ..
            } => {
                self.cursor = self.cursor.max(*height);
                match ours {
                    Some(purpose) => self.own(*purpose, operation, *outcome, effects),
                    // Signed with our key, but not through us: its fee is
                    // ours all the same, and the rest is reconciled.
                    None if Some(operation.account) == self.account => {
                        if let Outcome::Applied { fee, .. } | Outcome::Failed { fee, .. } = outcome
                        {
                            self.pay_fee(*fee, operation_hash(operation), effects);
                        }
                    }
                    None => self.other(operation, *outcome, effects),
                }
            }
            News::Final { height, timestamp } => {
                self.cursor = self.cursor.max(*height);
                for (name, peer) in &mut self.peers {
                    for request_id in peer.payer.final_block(*timestamp) {
                        self.decided.push_back(Decided {
                            peer: *name,
                            request_id,
                            fulfillment: None,
                        });
                    }
                    peer.payee.final_block(*timestamp);
                }
            }
            News::Dropped { purpose } => self.dropped(*purpose, effects),
            News::Resynced {
                height,
                before,
                unread,
                ..
            } => {
                self.cursor = self.cursor.max(*height);
                self.resynced(account, *height, before, unread.as_ref(), effects);
            }
            // The hub confirms the key a peer gave us, for a hold toward us
            // whose operations we never read; or names the account of a
            // peer that had none when it peered, which a deposit opened.
            News::Member {
                key,
                account,
                height,
            } => {
                self.pay_resolved(key, *account, *height, effects);
                let Some(number) = account else {
                    return;
                };
                for peer in self.peers.values_mut() {
                    if peer.key == *key && peer.account == NONE {
                        peer.account = *number;
                    }
                    if peer.key == *key && peer.account == *number {
                        peer.payee.key_confirmed = true;
                    }
                }
            }
            // The hub has no account for our key: we hold nothing in it,
            // and run, to be paid in by a peer
            // (spec/designs/settlement-hubs.md#5-joining).
            News::Unfunded => self.synced = true,
            News::Reduced { .. } => {}
        }
    }

    /// What holds toward us from our peers hold, as the account mirrors
    /// them, in drops.
    fn toward_us(&self, account: &HubAccount) -> Amount {
        self.peers
            .values()
            .filter_map(|peer| peer.payee.hold)
            .filter_map(|hold| account.hold(hold).map(|mirror| mirror.amount))
            .fold(0, Amount::saturating_add)
    }

    /// What we may spend of the account's balance, in drops: what its
    /// newest state says it holds, but never more than the books hold of
    /// it. The account learns its state before the scheme is told what led
    /// there (a deposit, whose attestations no operation shows, is told
    /// with the state that follows it), and what we spend is booked when
    /// the hub executes it: spending a deposit the books do not hold yet
    /// would take them below nothing, where they cannot go.
    fn spendable_balance(&self, account: &HubAccount) -> Amount {
        let newest = account.view().map_or(0, |(_, view)| view.balance);
        let booked = (self.booked / UNITS_PER_DROP).saturating_sub(self.toward_us(account));
        newest.min(booked)
    }

    /// Makes the books hold what the account and the holds toward us from
    /// our peers held as of a block: on the first sync, the owner's
    /// deposit; on the first state of an account opened since the hub said
    /// it had none, what deposits credited it as the owner's, never more,
    /// and its reserve as the reserve's; after, the owner's deposits and
    /// withdrawals since the last state, by how far the account's counters
    /// grew, and the rest reconciled through suspense, as the reserve's
    /// when the account reserve changed. An unread `pay` of ours counts as
    /// still ours until the hub says whether it applied, as the books hold
    /// it pending.
    fn match_balance(
        &mut self,
        account: &HubAccount,
        view: &AccountView,
        effects: &mut Vec<Effect>,
    ) {
        self.account = account.number();
        let target = units(
            view.balance
                .saturating_add(self.toward_us(account))
                .saturating_add(self.unread_paying()),
        );
        // Opened since the hub said it had no account for our key: only
        // what deposits credited it is the owner's, and what a peer's `pay`
        // brought was booked as its settlement when the account read it.
        let opened = self.synced && self.counters.is_none();
        if opened {
            let deposits = units(view.deposited.saturating_add(view.returned));
            let added = target.saturating_sub(self.booked).min(deposits);
            if added > 0 {
                effects.push(Effect::Deposit { amount: added });
                self.booked = self.booked.saturating_add(added);
            }
        } else if self.synced {
            self.book_moves(view, effects);
        } else {
            self.synced = true;
            if target > self.booked {
                effects.push(Effect::Deposit {
                    amount: target - self.booked,
                });
                self.booked = target;
            }
        }
        let (deposited, withdrawn, returned) = self.counters.unwrap_or_default();
        self.counters = Some((
            deposited.max(view.deposited),
            withdrawn.max(view.withdrawn),
            returned.max(view.returned),
        ));
        // An account opened since locked its reserve from what opened it:
        // nothing else it holds less than what was booked of it for.
        let reserve_moved = self
            .account_reserve
            .map_or(opened && target < self.booked, |reserve| {
                reserve != view.reserve
            });
        self.account_reserve = Some(view.reserve);
        if target != self.booked {
            effects.push(Effect::Reconcile {
                amount: target.abs_diff(self.booked),
                gain: target > self.booked,
                cause: if reserve_moved {
                    ReconcileCause::Reserve
                } else {
                    ReconcileCause::Unexplained
                },
            });
            self.booked = target;
        }
    }

    /// Books the owner's deposits and withdrawals since the last state the
    /// books were matched to: what the account's counters grew by. A
    /// deposit is credited by the validators' attestations, which never
    /// reach the account as an operation, so only the counters tell it
    /// from a difference nothing explains
    /// (spec/areas/hubs.md#the-members-side).
    fn book_moves(&mut self, view: &AccountView, effects: &mut Vec<Effect>) {
        let Some((deposited, withdrawn, returned)) = self.counters else {
            return;
        };
        let added = units(
            view.deposited
                .saturating_sub(deposited)
                .saturating_add(view.returned.saturating_sub(returned)),
        );
        if added > 0 {
            effects.push(Effect::Deposit { amount: added });
            self.booked = self.booked.saturating_add(added);
        }
        let grown = view.withdrawn.saturating_sub(withdrawn);
        let ahead = grown.min(self.booked_ahead);
        self.booked_ahead -= ahead;
        let taken = units(grown - ahead);
        if taken > 0 {
            effects.push(Effect::Withdrawal { amount: taken });
            self.booked = self.booked.saturating_sub(taken);
        }
    }

    /// What became of the owner's withdrawal on its way, which posts the
    /// node's hold on the owner's funds if it applied, and voids it
    /// otherwise: one whose outcome the scheme cannot tell, the account's
    /// counters book. Whether one was on its way.
    pub(super) fn withdrawal_done(
        &mut self,
        outcome: OwnerPaymentState,
        effects: &mut Vec<Effect>,
    ) -> bool {
        let Some((number, OwnerPaymentState::InFlight)) = self.withdrawal else {
            return false;
        };
        self.withdrawal = Some((number, outcome));
        effects.push(Effect::OwnerPaid {
            id: withdrawal_name(number),
            applied: outcome == OwnerPaymentState::Applied,
        });
        true
    }

    /// The hub executed our withdrawal of `amount`: what it paid out is the
    /// owner's withdrawal, its hold posted now, and its ledger fee a fee,
    /// both kept back from the growth of the withdrawals counter that shows
    /// them.
    fn withdrew(&mut self, operation: Digest, amount: Amount, effects: &mut Vec<Effect>) {
        if self.withdrawal_done(OwnerPaymentState::Applied, effects) {
            self.booked = self.booked.saturating_sub(units(amount));
            self.booked_ahead = self.booked_ahead.saturating_add(amount);
        }
        let fee = self.terms.fees.ledger;
        if fee > 0 {
            effects.push(Effect::Fee {
                amount: units(fee),
                id: ledger_fee_id(&operation),
            });
            self.booked = self.booked.saturating_sub(units(fee));
            self.booked_ahead = self.booked_ahead.saturating_add(fee);
        }
    }

    /// Books one of our operations the hub executed.
    fn own(
        &mut self,
        purpose: Purpose,
        operation: &SignedMemberOperation,
        outcome: Outcome,
        effects: &mut Vec<Effect>,
    ) {
        let (fee, applied, detail) = match outcome {
            Outcome::Applied { fee, detail } => (fee, true, detail),
            Outcome::Failed { fee, .. } => (fee, false, Detail::None),
            Outcome::Refused(_) => return,
        };
        let id = operation_hash(operation);
        match purpose {
            Purpose::Settle(settlement) => {
                if let (Outcome::Failed { .. }, MemberOperation::Pay { .. }) =
                    (outcome, &operation.operation)
                {
                    self.not_paid_in(settlement);
                }
                self.settled(settlement, operation, applied, detail, effects);
            }
            Purpose::Claim(hold) => {
                self.claimed_on_hub(hold, operation, applied);
                // The fee of a claim is the payer's doing: its claim was
                // missing or late. Booked now, in order with the rest.
                if let Some(peer) = self.payer_of(hold) {
                    let loss = Loss {
                        units: units(fee),
                        cause: LossCause::ClaimFee,
                        operation: id,
                        hold,
                        sequence: 0,
                    };
                    self.book_loss(peer, loss, effects);
                    return;
                }
            }
            Purpose::Withdraw => {
                if let (true, MemberOperation::Withdraw { amount, .. }) =
                    (applied, &operation.operation)
                {
                    self.withdrew(id, *amount, effects);
                } else {
                    self.withdrawal_done(OwnerPaymentState::Failed(PaymentFailure::Other), effects);
                }
            }
            Purpose::Redeem(_) | Purpose::Disbursal => {}
        }
        self.pay_fee(fee, id, effects);
    }

    fn pay_fee(&mut self, fee: Amount, id: Digest, effects: &mut Vec<Effect>) {
        if fee > 0 {
            effects.push(Effect::Fee {
                amount: units(fee),
                id: digest_name(&id),
            });
            self.booked = self.booked.saturating_sub(units(fee));
        }
    }

    /// The peer whose hold toward us `hold` is.
    fn payer_of(&self, hold: HoldNumber) -> Option<Name> {
        self.peers
            .iter()
            .find(|(_, peer)| peer.payee.hold == Some(hold))
            .map(|(name, _)| *name)
    }

    /// The peer our hold `hold` is toward.
    fn payee_of(&self, hold: HoldNumber) -> Option<Name> {
        self.peers
            .iter()
            .find(|(_, peer)| peer.payer.hold == Some(hold))
            .map(|(name, _)| *name)
    }

    /// Our `pay` for `settlement` failed. We ask the hub at once for the
    /// account of the peer's key: one a deposit opened first fails it, and
    /// the next settlement then funds a hold toward it. Whatever failed
    /// it, the peer is not paid in again for [`PAY_AGAIN_MS`], since each
    /// try costs a fee.
    fn not_paid_in(&mut self, settlement: Name) {
        let now = self.now_ms;
        let Some(settling) = self.settling.get(&settlement) else {
            return;
        };
        if let Some(peer) = self
            .peers
            .get_mut(&settling.peer)
            .filter(|peer| peer.account == NONE)
        {
            peer.look_up_ms = now;
            peer.pay_after_ms = now.saturating_add(PAY_AGAIN_MS);
        }
    }

    /// The hub's account for `key` as of the block at `height`, for each
    /// unread `pay` of ours to it. One there means the `pay` applied: the
    /// hub opened it with our `pay`, or had it already, which takes a
    /// deposit for the key racing our `pay`; then it is booked as made
    /// rather than risk paying the peer in twice. None as of a block the
    /// `pay` had executed by means it failed, and it is cancelled; none as
    /// of an older block says nothing, and is asked again.
    ///
    /// While it waited, the account's state counted it as still ours, as
    /// the books hold it ([`Self::match_balance`]): cancelled, what that
    /// counted is taken back through suspense.
    fn pay_resolved(
        &mut self,
        key: &PublicKey,
        number: Option<AccountNumber>,
        height: u64,
        effects: &mut Vec<Effect>,
    ) {
        let answered: Vec<Name> = self
            .unread_pays
            .iter()
            .filter(|(_, unread)| {
                unread.key == *key && (number.is_some() || height >= unread.height)
            })
            .map(|(settlement, _)| *settlement)
            .collect();
        for settlement in answered {
            self.unread_pays.remove(&settlement);
            let Some(settling) = self.settling.remove(&settlement) else {
                continue;
            };
            let amount = units(settling.drops);
            if number.is_some() {
                effects.push(Effect::Finalize { settlement });
            } else {
                effects.push(Effect::Cancel { settlement });
                effects.push(Effect::Reconcile {
                    amount,
                    gain: false,
                    cause: ReconcileCause::Unexplained,
                });
            }
            self.booked = self.booked.saturating_sub(amount);
        }
    }

    /// What our unread `pay`s move, in drops: still ours until the hub
    /// says whether they applied.
    fn unread_paying(&self) -> Amount {
        self.unread_pays
            .keys()
            .filter_map(|settlement| self.settling.get(settlement))
            .fold(0, |sum, settling| sum.saturating_add(settling.drops))
    }

    /// One of our settlements applied, or failed.
    fn settled(
        &mut self,
        settlement: Name,
        operation: &SignedMemberOperation,
        applied: bool,
        detail: Detail,
        effects: &mut Vec<Effect>,
    ) {
        let Some(settling) = self.settling.remove(&settlement) else {
            return;
        };
        if !applied {
            effects.push(Effect::Cancel { settlement });
            return;
        }
        effects.push(Effect::Finalize { settlement });
        self.booked = self.booked.saturating_sub(units(settling.drops));
        // Our `pay` opened the peer's account: the peering goes on over it.
        if let (MemberOperation::Pay { key, .. }, Detail::Account(number)) =
            (&operation.operation, detail)
            && let Some(peer) = self.peers.get_mut(&settling.peer)
            && peer.key == *key
            && peer.account == NONE
        {
            peer.account = number;
            peer.payee.key_confirmed = true;
        }
        if let (MemberOperation::HoldOpen { .. }, Detail::Hold(number)) =
            (&operation.operation, detail)
        {
            if let Some(peer) = self.peers.get_mut(&settling.peer) {
                peer.payer.hold = Some(number);
            }
            // The hold's reserve is the ledger's: never booked.
            let reserve = units(self.terms.fees.hold_reserve);
            effects.push(Effect::Reconcile {
                amount: reserve,
                gain: false,
                cause: ReconcileCause::Reserve,
            });
            self.booked = self.booked.saturating_sub(reserve);
        }
    }

    /// Our claim on the hub on `hold` executed: the packets it paid are
    /// paid; those of a claim that failed may be claimed again, alone, a
    /// bounded number of times while their commitments last.
    fn claimed_on_hub(
        &mut self,
        hold: HoldNumber,
        operation: &SignedMemberOperation,
        applied: bool,
    ) {
        let MemberOperation::Claim { fulfilled, .. } = &operation.operation else {
            return;
        };
        let sequences: Vec<u64> = fulfilled
            .iter()
            .map(|f| f.commitment.value.sequence)
            .collect();
        if let Some(name) = self.payer_of(hold)
            && let Some(peer) = self.peers.get_mut(&name)
        {
            if applied {
                peer.payee.set_claiming(&sequences, Claiming::Paid);
            } else {
                peer.payee.claim_failed(&sequences);
            }
        }
    }

    /// An operation of ours the hub refused for good.
    pub(super) fn dropped(&mut self, purpose: Purpose, effects: &mut Vec<Effect>) {
        match purpose {
            Purpose::Settle(settlement) => {
                if self.settling.remove(&settlement).is_some() {
                    effects.push(Effect::Cancel { settlement });
                }
            }
            Purpose::Claim(hold) => {
                if let Some(name) = self.payer_of(hold)
                    && let Some(peer) = self.peers.get_mut(&name)
                {
                    let sequences: Vec<u64> = peer.payee.taken.keys().copied().collect();
                    for sequence in sequences {
                        if let Some(done) = peer
                            .payee
                            .taken
                            .get_mut(&sequence)
                            .and_then(|t| t.done.as_mut())
                            && done.claiming == Claiming::Submitted
                        {
                            done.claiming = Claiming::No;
                        }
                    }
                }
            }
            Purpose::Withdraw => {
                self.withdrawal_done(OwnerPaymentState::Failed(PaymentFailure::Other), effects);
            }
            Purpose::Redeem(_) | Purpose::Disbursal => {}
        }
    }

    /// Books an operation of another member's that touches us.
    fn other(
        &mut self,
        operation: &SignedMemberOperation,
        outcome: Outcome,
        effects: &mut Vec<Effect>,
    ) {
        let Outcome::Applied { detail, .. } = outcome else {
            return;
        };
        let peer = self
            .peers
            .iter()
            .find(|(_, peer)| peer.account == operation.account)
            .map(|(name, _)| *name);
        match (&operation.operation, detail) {
            (MemberOperation::HoldOpen { payee, amount }, Detail::Hold(number))
                if Some(*payee) == self.account =>
            {
                match peer {
                    Some(name) => self.hold_toward_us(name, number, operation, *amount, effects),
                    None => self.stranger(number, operation),
                }
            }
            (MemberOperation::HoldFund { hold, amount }, _) => {
                if let Some(name) = peer.filter(|name| self.payer_of(*hold) == Some(*name)) {
                    self.hold_toward_us(name, *hold, operation, *amount, effects);
                }
            }
            // A peer paid us into the hub: its settlement, reserve and all.
            // Anyone else's `pay` is reconciled with the account's state.
            (MemberOperation::Pay { key, amount }, Detail::Account(number))
                if *key == self.key.public_key() && Some(number) == self.account =>
            {
                if let Some(name) = peer {
                    let amount = units(*amount);
                    effects.push(Effect::IncomingSettlement {
                        peer: name,
                        amount,
                        id: Some(digest_name(&operation_hash(operation))),
                    });
                    self.booked = self.booked.saturating_add(amount);
                }
            }
            (MemberOperation::Claim { hold, fulfilled }, Detail::Paid(paid)) => {
                if let Some(name) = self.payee_of(*hold) {
                    self.peer_claimed(name, fulfilled, paid, operation_hash(operation));
                }
            }
            (MemberOperation::HoldRelease { hold }, _) => {
                if let Some(name) = self.payee_of(*hold)
                    && let Some(peer) = self.peers.get_mut(&name)
                {
                    // It went back to us; the next settlement opens another,
                    // with records of its own.
                    peer.payer = Payer::new();
                }
            }
            _ => {}
        }
    }

    /// The hold `number` toward us from peer `name` was opened or funded
    /// with `amount`: its incoming settlement.
    fn hold_toward_us(
        &mut self,
        name: Name,
        number: HoldNumber,
        operation: &SignedMemberOperation,
        amount: Amount,
        effects: &mut Vec<Effect>,
    ) {
        let hub = self.hub;
        let Some(peer) = self.peers.get_mut(&name) else {
            return;
        };
        if peer.payee.hold.is_some_and(|hold| hold != number) {
            // A second hold from the same peer: its promises name the first.
            return;
        }
        peer.payee.hold = Some(number);
        if operation.verifies(&hub, &peer.key) {
            peer.payee.key_confirmed = true;
        }
        let amount = units(amount);
        effects.push(Effect::IncomingSettlement {
            peer: name,
            amount,
            id: Some(digest_name(&operation_hash(operation))),
        });
        self.booked = self.booked.saturating_add(amount);
    }

    /// A hold opened toward us by an account not our peer: kept a while, in
    /// case its peering is being made.
    fn stranger(&mut self, number: HoldNumber, operation: &SignedMemberOperation) {
        self.strangers.insert(number, operation.clone());
        while self.strangers.len() > MAX_STRANGERS {
            self.strangers.pop_first();
        }
    }

    /// Peer `name` won a claim on the hub on our hold toward it.
    fn peer_claimed(
        &mut self,
        name: Name,
        fulfilled: &[crate::promise::Fulfilled],
        paid: Amount,
        operation: Digest,
    ) {
        let Some(peer) = self.peers.get_mut(&name) else {
            return;
        };
        let (decided, losses) = peer.payer.claimed(fulfilled, paid, operation);
        for (request_id, fulfillment) in decided {
            if self.decided.len() < MAX_DECIDED {
                self.decided.push_back(Decided {
                    peer: name,
                    request_id,
                    fulfillment: Some(fulfillment),
                });
            }
        }
        for loss in losses {
            self.queue_loss(name, loss);
        }
    }

    pub(super) fn queue_loss(&mut self, peer: Name, loss: Loss) {
        assert!(self.losses.len() < MAX_LOSSES, "losses past their bound");
        self.losses.push_back((peer, loss));
    }

    /// Books the losses waiting: those found while the node handed the
    /// scheme a packet, with no effects to report them in.
    fn book_losses(&mut self, effects: &mut Vec<Effect>) {
        while let Some((peer, loss)) = self.losses.pop_front() {
            self.book_loss(peer, loss, effects);
        }
    }

    fn book_loss(&mut self, peer: Name, loss: Loss, effects: &mut Vec<Effect>) {
        if loss.cause == LossCause::ClaimFee {
            self.booked = self.booked.saturating_sub(loss.units);
        }
        if loss.units > 0 {
            effects.push(Effect::PeerLoss {
                peer,
                amount: loss.units,
                cause: loss.cause,
                id: loss_id(&loss),
            });
        }
    }

    /// Takes in what the account mirrors of each peer's holds.
    fn refresh(&mut self, account: &HubAccount) {
        self.account = account.number();
        let snapshot = |hold: Option<HoldNumber>| {
            hold.and_then(|hold| account.hold(hold))
                .map_or_else(Snapshot::default, |mirror| Snapshot {
                    amount: mirror.amount,
                    taken: mirror.taken(),
                    through: mirror.through,
                    reduction: mirror.reduction,
                })
        };
        let mut paying: Amount = 0;
        for peer in self.peers.values_mut() {
            // A hold that closed, released or reduced to nothing, takes its
            // records with it: the next one starts at nothing, as the hub
            // starts it (spec/areas/hubs.md#packets-promises-and-claims).
            if peer
                .payer
                .hold
                .is_some_and(|hold| account.hold(hold).is_none())
            {
                peer.payer = Payer::new();
            }
            if peer
                .payee
                .hold
                .is_some_and(|hold| account.hold(hold).is_none())
            {
                peer.payee = Payee::fresh(peer.payee.key_confirmed);
            }
            peer.payer.snapshot = snapshot(peer.payer.hold);
            peer.payee.snapshot = snapshot(peer.payee.hold);
            if peer.payer.hold.is_some() {
                paying = paying.saturating_add(1);
            }
        }
        let account_reserve = account.view().map_or(0, |(_, view)| view.reserve);
        self.reserve =
            account_reserve.saturating_add(paying.saturating_mul(self.terms.fees.hold_reserve));
    }

    /// Finds the hold a peer opened toward us before its peering was made.
    fn adopt_holds(&mut self, account: &HubAccount, effects: &mut Vec<Effect>) {
        let waiting: Vec<(Name, AccountNumber)> = self
            .peers
            .iter()
            .filter(|(_, peer)| peer.payee.hold.is_none())
            .map(|(name, peer)| (*name, peer.account))
            .collect();
        for (name, number) in waiting {
            let found = self
                .strangers
                .iter()
                .find(|(_, operation)| operation.account == number)
                .map(|(hold, operation)| (*hold, operation.clone()));
            let Some((hold, operation)) = found else {
                continue;
            };
            self.strangers.remove(&hold);
            let amount = account.hold(hold).map_or(0, |mirror| mirror.amount);
            self.hold_toward_us(name, hold, &operation, amount, effects);
        }
    }

    /// Claims on the hub what our peers' claims left uncovered, once late.
    fn claim_missing(&mut self, account: &mut HubAccount) {
        let timing = (self.terms.timers.allowance, self.margin());
        let limit = usize::from(self.terms.limits.commitments_per_claim);
        let room = usize::from(self.terms.limits.claimed_per_hold);
        for peer in self.peers.values_mut() {
            let Some(hold) = peer.payee.hold else {
                continue;
            };
            if account.has(Purpose::Claim(hold)) {
                continue;
            }
            let claimed = account.hold(hold).map_or(0, |mirror| mirror.claimed.len());
            let limit = limit.min(room.saturating_sub(claimed));
            let due = peer.payee.due(self.now_ms, timing, limit);
            if due.is_empty() {
                continue;
            }
            let sequences: Vec<u64> = due.iter().map(|f| f.commitment.value.sequence).collect();
            let operation = MemberOperation::Claim {
                hold,
                fulfilled: due,
            };
            if account.submit(Purpose::Claim(hold), operation).is_ok() {
                peer.payee.set_claiming(&sequences, Claiming::Submitted);
            }
        }
    }

    /// Redeems a peer's newest claim once what it adds reaches the
    /// threshold: each redeem costs a fee.
    fn redeem(&mut self, account: &mut HubAccount) {
        let threshold = redeem_threshold(self.max_packet);
        for peer in self.peers.values() {
            let (Some(hold), Some(claim)) = (peer.payee.hold, peer.payee.accepted) else {
                continue;
            };
            let Some(mirror) = account.hold(hold) else {
                continue;
            };
            if claim.value.through <= mirror.through || account.was_sent(Purpose::Redeem(hold)) {
                continue;
            }
            let adds = claim
                .value
                .total
                .saturating_sub(mirror.total)
                .saturating_sub(mirror.claimed_through(claim.value.through));
            if units(adds) >= threshold {
                let _ = account.submit(Purpose::Redeem(hold), MemberOperation::Redeem(claim));
            }
        }
    }

    /// When the next claim of ours falls due.
    fn next_due(&self) -> Option<u64> {
        let timing = (self.terms.timers.allowance, self.margin());
        let now = self.now_ms;
        // What was due by now and is still waiting could not be claimed now,
        // as when a claim on its hold is already on its way: it is claimed
        // with the next news or poll, not by waking the account at once,
        // over and over.
        self.peers
            .values()
            .filter_map(|peer| peer.payee.next_due(now, timing))
            .chain(
                self.peers
                    .values()
                    .filter(|peer| peer.account == NONE)
                    .map(|peer| peer.look_up_ms),
            )
            .filter(|at| *at > now)
            .min()
    }

    /// Asks the hub for the accounts of keys whose account we wait for:
    /// the key of each unread `pay` of ours, at every turn until the hub
    /// answers as of a block after it, before anything else; and each peer
    /// without an account in our records, every [`LOOK_UP_EVERY_MS`], so
    /// that one a deposit opened is learnt without paying the peer in. A
    /// key the account cannot ask about yet is asked about at the next
    /// turn.
    fn ask_accounts(&mut self, account: &mut HubAccount) {
        for unread in self.unread_pays.values() {
            account.look_up(unread.key);
        }
        let now = self.now_ms;
        for peer in self.peers.values_mut() {
            if peer.account == NONE && peer.look_up_ms <= now && account.look_up(peer.key) {
                peer.look_up_ms = now.saturating_add(LOOK_UP_EVERY_MS);
            }
        }
    }

    fn peer(&mut self, name: Name) -> Option<&mut Peer> {
        self.peers.get_mut(&name)
    }

    /// Whether `peer` may peer as `membership`: as it did before, or, once
    /// nothing of its old membership is left to settle, as another. A peer
    /// that came back with another account would leave behind what we
    /// fulfilled and never claimed; with another key, it would have us
    /// take promises the hub refuses.
    ///
    /// Two members without an account do not peer: neither could pay the
    /// other in.
    fn may_peer(&self, peer: Name, membership: &Membership) -> Result<(), SchemeError> {
        if !self.can_pay_in(membership) {
            return Err(SchemeError::Unavailable);
        }
        match self.peers.get(&peer) {
            Some(known) if !known.is(membership) && known.unresolved() => {
                Err(SchemeError::Unavailable)
            }
            _ => Ok(()),
        }
    }

    /// A settlement to `peer`, which has no account in the hub yet: a
    /// `pay` of `drops` to the key it gave, which opens its account with
    /// them (spec/designs/settlement-hubs.md#5-joining). At least the hub's
    /// minimum for a new account, never less, and at most what our account
    /// can spend; one at a time, so it is paid in once, and not for
    /// [`PAY_AGAIN_MS`] after one failed.
    fn prepare_pay_in(
        &mut self,
        rng: &mut dyn CryptoRng,
        account: &mut HubAccount,
        peer: Name,
        drops: Amount,
        membership: &Membership,
        opening: bool,
    ) -> Result<PreparedSettlement, SchemeError> {
        let minimum = self.terms.new_account_minimum();
        let failed_lately = self
            .peers
            .get(&peer)
            .is_some_and(|known| self.now_ms < known.pay_after_ms);
        if opening || failed_lately || drops < minimum {
            return Err(SchemeError::Unavailable);
        }
        let spendable = self
            .spendable_balance(account)
            .saturating_sub(account.committed())
            .saturating_sub(self.terms.fees.operation);
        let drops = drops.min(spendable);
        if drops < minimum {
            return Err(SchemeError::Unavailable);
        }
        let operation = MemberOperation::Pay {
            key: membership.key,
            amount: drops,
        };
        self.queue_settlement(rng, account, peer, drops, operation)
    }

    /// Queues `operation`, our settlement of `drops` to `peer`, to send
    /// once the node executed it.
    fn queue_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        account: &mut HubAccount,
        peer: Name,
        drops: Amount,
        operation: MemberOperation,
    ) -> Result<PreparedSettlement, SchemeError> {
        let mut bytes = [0u8; 16];
        rng.fill_bytes(&mut bytes);
        let id = Name::hex(&bytes, false);
        account.prepare(id, operation)?;
        self.settling.insert(id, Settling { peer, drops });
        Ok(PreparedSettlement {
            id,
            amount: drops * UNITS_PER_DROP,
            message: bytes.to_vec(),
        })
    }

    /// Whether one of us has an account: the other can be paid in.
    fn can_pay_in(&self, membership: &Membership) -> bool {
        self.account.is_some() || membership.account != NONE
    }

    /// The account of `peer`, as the scheme knows it: the one it gave at
    /// peering, or, if it had none then, the one the hub opened for its key
    /// since. None for a peer without one, and for one the scheme does not
    /// know.
    #[must_use]
    pub fn peer_account(&self, peer: Name) -> Option<AccountNumber> {
        self.peers
            .get(&peer)
            .map(|peer| peer.account)
            .filter(|number| *number != NONE)
    }
}

impl SettlementScheme for HubScheme {
    fn id(&self) -> Name {
        self.id
    }

    fn ledger(&self) -> LedgerId {
        self.ledger
    }

    fn asset(&self) -> Option<AssetId> {
        Some(self.asset)
    }

    fn balance(&self) -> u64 {
        self.booked
    }

    fn status(&self) -> LedgerStatus {
        if self.synced {
            LedgerStatus::Synced
        } else {
            LedgerStatus::Syncing
        }
    }

    fn has_synced(&self) -> bool {
        self.synced
    }

    fn unit(&self) -> u64 {
        UNITS_PER_DROP
    }

    fn reserve(&self) -> u64 {
        units(self.reserve)
    }

    fn state(&self) -> Vec<u8> {
        super::state::scheme_state(self)
    }

    fn state_version(&self) -> u64 {
        super::state::fingerprint(&self.state())
    }

    fn on_asset(&mut self, now_ms: u64, asset: &mut dyn ExternalAsset, effects: &mut Vec<Effect>) {
        self.now_ms = self.now_ms.max(now_ms);
        let account = account_of(asset);
        assert!(
            account.is_some(),
            "the scheme settles through a hub account"
        );
        let Some(account) = account else {
            return;
        };
        assert_eq!(account.hub(), self.hub, "the scheme's own hub");
        // An account learnt since may read what opened it before its state
        // is told: a peer's `pay`, or a hold opened toward it.
        self.account = account.number();
        let news = account.news().to_vec();
        for item in &news {
            self.tell(account, item, effects);
        }
        // Holds toward us are followed from our peers only.
        account.follow(
            self.peers
                .values()
                .map(|peer| peer.account)
                .filter(|number| *number != NONE),
        );
        self.ask_accounts(account);
        if !account.has(Purpose::Withdraw) {
            // Not queued, and not told: only the counters can say.
            self.withdrawal_done(OwnerPaymentState::Unknown, effects);
        }
        self.refresh(account);
        self.adopt_holds(account, effects);
        if self.synced {
            self.claim_missing(account);
            self.redeem(account);
        }
        self.book_losses(effects);
        account.wake(self.next_due());
        self.cursor = account.cursor();
    }

    /// Our membership: with our account, or, once f + 1 validators agree
    /// the hub has none for our key, with none, to be paid in.
    fn peering_info(&self, out: &mut Vec<u8>) {
        out.clear();
        if self.synced || self.account.is_some() {
            Membership {
                hub: self.hub,
                account: self.account.unwrap_or(NONE),
                key: self.key.public_key(),
            }
            .write(out);
        }
    }

    fn create_peering_request(
        &mut self,
        _peer: Name,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        let membership = Membership::read(peering_info, &self.hub)?;
        self.peering_info(out);
        if out.is_empty() || !self.can_pay_in(&membership) {
            out.clear();
            return Err(SchemeError::Unavailable);
        }
        Ok(())
    }

    fn accept_peering_request(&mut self, peer: Name, data: &[u8]) -> Result<Vec<u8>, SchemeError> {
        let membership = Membership::read(data, &self.hub)?;
        if !self.peers.contains_key(&peer) && self.peers.len() >= MAX_PEERS {
            return Err(SchemeError::Unavailable);
        }
        self.may_peer(peer, &membership)?;
        Ok(data.to_vec())
    }

    fn finalize_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        let membership = Membership::read(peering_info, &self.hub)?;
        self.may_peer(peer, &membership)?;
        Ok(peering_info.to_vec())
    }

    fn on_peered(&mut self, peer: Name, state: &[u8]) {
        let membership = Membership::read(state, &self.hub);
        assert!(membership.is_ok(), "a peer state the scheme did not return");
        let Ok(membership) = membership else {
            return;
        };
        if let Some(known) = self.peers.get_mut(&peer) {
            if known.is(&membership) {
                // Peered again as before: its holds stay, and so do our
                // records of them. An account it has now that it had not
                // then is the one the hub opened for its key.
                if known.account == NONE {
                    known.account = membership.account;
                }
                return;
            }
            if known.unresolved() {
                // Refused when the peering was made; anything that came up
                // since keeps the old records until it is resolved.
                return;
            }
        }
        if self.peers.len() >= MAX_PEERS && !self.peers.contains_key(&peer) {
            return;
        }
        self.peers.insert(
            peer,
            Peer {
                account: membership.account,
                key: membership.key,
                payer: Payer::new(),
                payee: Payee::default(),
                look_up_ms: 0,
                pay_after_ms: 0,
            },
        );
    }

    /// The channel closed only once its records were resolved
    /// ([`keeps_channel`](SettlementScheme::keeps_channel)), so they go
    /// (spec/decisions/closing-channels.md).
    fn on_closed(&mut self, peer: Name) {
        assert!(
            self.peers
                .get(&peer)
                .is_none_or(|known| !known.unresolved()),
            "a channel closes once the hub's records of it are resolved"
        );
        self.peers.remove(&peer);
    }

    /// A hold with funds either way, or a packet no claim covers yet, still
    /// needs the channel: what it settles is not on the hub's ledger yet.
    fn keeps_channel(&self, peer: Name) -> bool {
        self.peers.get(&peer).is_some_and(Peer::unresolved)
    }

    /// Funds the hold toward `peer` by whole drops of `amount`, opening it
    /// on the first settlement. Unavailable while the hold is being opened,
    /// or the balance cannot pay it. A peer without an account is paid in
    /// instead, with at least the hub's minimum for a new account.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        asset: Option<&mut dyn ExternalAsset>,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        let mut membership = Membership::read(peer_state, &self.hub)?;
        // What we learnt since it peered: the account the hub opened for
        // its key.
        if let Some(known) = self.peers.get(&peer).filter(|known| known.is(&membership)) {
            membership.account = known.account;
        }
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        let drops = amount / UNITS_PER_DROP;
        if drops == 0 || self.settling.len() >= MAX_SETTLING {
            return Err(SchemeError::Unavailable);
        }
        let opening = self.settling.values().any(|s| s.peer == peer);
        if membership.account == NONE {
            return self.prepare_pay_in(rng, account, peer, drops, &membership, opening);
        }
        let hold = self.peers.get(&peer).and_then(|p| p.payer.hold);
        // Opening a hold locks its reserve besides what it holds.
        let reserve = match hold {
            Some(_) => 0,
            None if opening => return Err(SchemeError::Unavailable),
            None => self.terms.fees.hold_reserve,
        };
        let balance = self.spendable_balance(account);
        let spendable = balance
            .saturating_sub(account.committed())
            .saturating_sub(self.terms.fees.operation)
            .saturating_sub(reserve);
        // At most what the account can spend: a node asked to settle funds
        // the hold with what it has (spec/areas/hubs.md#in-the-node).
        let drops = drops.min(spendable);
        if drops == 0 {
            return Err(SchemeError::Unavailable);
        }
        let operation = match hold {
            Some(hold) => MemberOperation::HoldFund {
                hold,
                amount: drops,
            },
            None => MemberOperation::HoldOpen {
                payee: membership.account,
                amount: drops,
            },
        };
        self.queue_settlement(rng, account, peer, drops, operation)
    }

    fn discard_settlement(&mut self, asset: Option<&mut dyn ExternalAsset>, settlement: Name) {
        if let Some(account) = asset.and_then(account_of) {
            account.discard(settlement);
            if !account.has(Purpose::Settle(settlement)) {
                self.settling.remove(&settlement);
            }
        }
    }

    fn settlement_message(&self, settlement: Name) -> Vec<u8> {
        super::state::hex_bytes(settlement.as_str()).unwrap_or_default()
    }

    fn execute_settlement(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        settlement: Name,
        effects: &mut Vec<Effect>,
    ) {
        let account = asset.and_then(account_of);
        assert!(
            account.is_some(),
            "the scheme settles through a hub account"
        );
        if let Some(account) = account
            && account.execute(settlement)
        {
            return;
        }
        self.settling.remove(&settlement);
        effects.push(Effect::Cancel { settlement });
    }

    /// A settlement the account still has is sent, or is on its way; one
    /// it does not have was never sent: the account and the scheme are
    /// written together, so one that executed was booked with them. An
    /// unread `pay` waits for the hub's word on the key it paid in.
    fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        let account = asset.and_then(account_of);
        assert!(
            account.is_some(),
            "the scheme settles through a hub account"
        );
        let Some(account) = account else {
            return;
        };
        for (settlement, _) in pending {
            if !self.unread_pays.contains_key(settlement) && !account.execute(*settlement) {
                self.settling.remove(settlement);
                effects.push(Effect::Cancel {
                    settlement: *settlement,
                });
            }
        }
    }

    /// The message books nothing: the hub says what arrived, once.
    fn handle_settlement(
        &mut self,
        _peer: Name,
        _amount: u64,
        data: &[u8],
        _peer_state: &[u8],
        _effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        if data.len() == 16 {
            Ok(())
        } else {
            Err(SchemeError::InvalidData)
        }
    }

    fn deposit(&mut self, _amount: u64, _effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    /// A withdrawal from the hub to `payment`'s address on the XRP Ledger,
    /// with its tag: the hub takes it and its ledger fee from the balance,
    /// and its escrow pays it. Booked as the owner's withdrawal from the
    /// account's counters once an agreed state shows it.
    fn pay_owner(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        payment: &OwnerPayment,
    ) -> Result<Name, SchemeError> {
        let destination =
            dassie_xrpl::AccountId::parse(&payment.destination).ok_or(SchemeError::InvalidData)?;
        let drops = payment.amount / UNITS_PER_DROP;
        let fees = self.terms.fees;
        let unpayable = destination.0 == self.terms.escrow || destination.0 == [0; 20];
        if unpayable || payment.memo.is_some() || drops < fees.least_payout {
            return Err(SchemeError::InvalidData);
        }
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        if account.has(Purpose::Withdraw) {
            return Err(SchemeError::Unavailable);
        }
        let balance = self.spendable_balance(account);
        let spendable = balance
            .saturating_sub(account.committed())
            .saturating_sub(fees.operation)
            .saturating_sub(fees.ledger);
        if spendable < drops {
            return Err(SchemeError::Unavailable);
        }
        let to = crate::operation::Destination {
            account: destination.0,
            tag: payment.tag,
        };
        account.submit(
            Purpose::Withdraw,
            MemberOperation::Withdraw { amount: drops, to },
        )?;
        self.withdrawals_made = self.withdrawals_made.saturating_add(1);
        self.withdrawal = Some((self.withdrawals_made, OwnerPaymentState::InFlight));
        Ok(withdrawal_name(self.withdrawals_made))
    }

    fn owner_payment(&self, id: Name) -> OwnerPaymentState {
        match self.withdrawal {
            Some((number, outcome)) if withdrawal_name(number) == id => outcome,
            _ => OwnerPaymentState::Unknown,
        }
    }

    fn commitments(&mut self) -> Option<&mut dyn Commitments> {
        Some(self)
    }

    fn check_invariants(&self) {
        assert!(self.peers.len() <= MAX_PEERS);
        assert!(self.settling.len() <= MAX_SETTLING);
        assert!(self.strangers.len() <= MAX_STRANGERS);
        assert!(self.losses.len() <= MAX_LOSSES);
        assert!(self.decided.len() <= MAX_DECIDED);
        assert!(
            self.booked <= dassie_ledger::MAX_TOTAL,
            "the books hold at most a total"
        );
        for peer in self.peers.values() {
            peer.payer.check_invariants();
            peer.payee.check_invariants();
        }
        assert!(
            self.settling
                .values()
                .all(|s| self.peers.contains_key(&s.peer) || s.drops > 0),
            "a settlement moves whole drops"
        );
        assert!(
            self.unread_pays
                .keys()
                .all(|settlement| self.settling.contains_key(settlement)),
            "an unread pay is a settlement on its way"
        );
        if self.counters.is_some() {
            assert!(self.synced, "the owner's moves count from the first sync");
            assert!(
                self.account.is_some(),
                "the owner's moves count in an account"
            );
        }
    }
}

impl Commitments for HubScheme {
    fn commit(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        self.now_ms = self.now_ms.max(now_ms);
        let margin = self.margin();
        let (hub, key) = (self.hub, Arc::clone(&self.key));
        let peer = self.peer(peer).ok_or(SchemeError::Unavailable)?;
        let commitment = peer.payer.commit(request_id, promised, margin)?;
        out.clear();
        out.extend_from_slice(&Signed::sign(commitment, &hub, &key).to_vec());
        Ok(())
    }

    fn check_commitment(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        commitment: &[u8],
    ) -> Result<(), SchemeError> {
        self.now_ms = self.now_ms.max(now_ms);
        let (hub, margin) = (self.hub, self.margin());
        let signed = decode_exact(commitment, Signed::<Commitment>::decode)?;
        let peer = self.peer(peer).ok_or(SchemeError::InvalidData)?;
        let key = peer.key;
        peer.payee
            .check(request_id, promised, signed, (&hub, &key, margin))
    }

    fn on_fulfilled(&mut self, now_ms: u64, peer: Name, request_id: u32, fulfillment: [u8; 32]) {
        self.now_ms = self.now_ms.max(now_ms);
        let now = self.now_ms;
        if let Some(peer) = self.peer(peer) {
            peer.payee.fulfilled(now, request_id, fulfillment);
        }
    }

    fn on_released(&mut self, peer: Name, request_id: u32) {
        let now = self.now_ms;
        if let Some(peer) = self.peer(peer) {
            peer.payee.released(now, request_id);
        }
    }

    fn claim(&mut self, now_ms: u64, peer: Name, request_id: u32, taken: bool, out: &mut Vec<u8>) {
        self.now_ms = self.now_ms.max(now_ms);
        out.clear();
        let (hub, key) = (self.hub, Arc::clone(&self.key));
        let Some(state) = self.peer(peer) else {
            return;
        };
        let (claim, loss) = state.payer.fulfilled(request_id, taken);
        if let Some(claim) = claim {
            out.extend_from_slice(&Signed::sign(claim, &hub, &key).to_vec());
        }
        if let Some(loss) = loss {
            self.queue_loss(peer, loss);
        }
    }

    fn on_claim(&mut self, now_ms: u64, peer: Name, _request_id: u32, claim: &[u8]) {
        self.now_ms = self.now_ms.max(now_ms);
        let hub = self.hub;
        let Ok(signed) = decode_exact(claim, Signed::<Claim>::decode) else {
            return;
        };
        if let Some(peer) = self.peer(peer) {
            let key = peer.key;
            peer.payee.claim(signed, &hub, &key);
        }
    }

    fn on_hub_wait(
        &mut self,
        now_ms: u64,
        peer: Name,
        request_id: u32,
        condition: [u8; 32],
        _backstop_ms: u64,
    ) {
        self.now_ms = self.now_ms.max(now_ms);
        let Some(state) = self.peer(peer) else {
            return;
        };
        let matches = state
            .payer
            .requests
            .get(&request_id)
            .and_then(|sequence| state.payer.packets.get(sequence))
            .is_some_and(|packet| packet.commitment.condition == condition);
        if !matches {
            return;
        }
        if let Some(fulfillment) = state.payer.wait(request_id)
            && self.decided.len() < MAX_DECIDED
        {
            self.decided.push_back(Decided {
                peer,
                request_id,
                fulfillment,
            });
        }
    }

    fn on_answered(&mut self, peer: Name, request_id: u32) {
        let Some(state) = self.peer(peer) else {
            return;
        };
        if let Some(loss) = state.payer.answered(request_id) {
            self.queue_loss(peer, loss);
        }
    }

    fn take_decided(&mut self, out: &mut Vec<Decided>) {
        out.extend(self.decided.drain(..));
    }

    /// What is left in our hold toward `peer` and in its hold toward us,
    /// as the account mirrored them from the last block it read through.
    fn holds(&self, peer: Name) -> dassie_settlement::HoldsWith {
        self.peers
            .get(&peer)
            .map_or_else(dassie_settlement::HoldsWith::default, |peer| {
                dassie_settlement::HoldsWith {
                    toward: units(peer.payer.snapshot.amount),
                    from: units(peer.payee.snapshot.amount),
                }
            })
    }

    /// The room the peer's hold toward us has for its packets, any
    /// reduction waiting counted: none without a hold.
    fn room_from(&self, peer: Name) -> Option<u64> {
        let peer = self.peers.get(&peer)?;
        let room = peer
            .payee
            .hold
            .map_or(0, |_| peer.payee.room(crate::Millis::MAX));
        Some(units(room))
    }

    /// The hub, and our account in it once it has its number. None before,
    /// also while the hub has no account for our key: the node does not yet
    /// carry packets on a peering without promises
    /// (spec/roadmap.md#joining-by-being-paid), so a member to be paid in
    /// is not advertised, and peers keep peering with it on any other
    /// scheme. Its peering info presents it with no account all the same.
    fn membership(&self) -> Option<dassie_settlement::Membership> {
        let account = self.account?;
        Some(dassie_settlement::Membership {
            hub: *self.hub.0.as_bytes(),
            account: account.0,
        })
    }

    /// Not while either of us has no account: the payer has no hold to
    /// promise from.
    fn commits_with(&self, peer: Name) -> bool {
        let keyless = self.peers.get(&peer).is_some_and(|p| p.account == NONE);
        self.account.is_some() && !keyless
    }

    /// The hub's minimum for a new account, for a peer without one, while
    /// we have one to pay it in from.
    fn opening_minimum(&self, peer: Name) -> Option<u64> {
        let keyless = self.peers.get(&peer)?.account == NONE;
        (keyless && self.account.is_some()).then(|| units(self.terms.new_account_minimum()))
    }
}

/// Decodes a value that fills `bytes`.
fn decode_exact<T>(
    bytes: &[u8],
    decode: impl FnOnce(&mut Reader<'_>) -> Result<T, crate::codec::HubDecodeError>,
) -> Result<T, SchemeError> {
    let mut reader = Reader::new(bytes);
    let value = decode(&mut reader).map_err(|_| SchemeError::InvalidData)?;
    reader.finish().map_err(|_| SchemeError::InvalidData)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use dassie_identity::NodeKey;
    use dassie_settlement::SettlementScheme;

    use super::*;
    use crate::operation::Destination;
    use crate::rules::Rules;
    use crate::rules::tests::{rules, token_terms};

    /// `rules` with `terms`, each acceptance signed again.
    fn with_terms(rules: &Rules, terms: Terms) -> Rules {
        let validators = rules
            .validators
            .iter()
            .enumerate()
            .map(|(index, acceptance)| {
                let seed = u8::try_from(index + 1).unwrap();
                acceptance
                    .clone()
                    .sign(&NodeKey::from_bytes(&[seed; 32]), &terms)
            })
            .collect();
        Rules { terms, validators }
    }

    fn scheme(rules: Rules, id: &str) -> Result<HubScheme, SchemeError> {
        let account = HubAccount::new(
            super::super::account_id(id).unwrap(),
            rules,
            Arc::new(NodeKey::from_bytes(&[42; 32])),
            Destination {
                account: [5; 20],
                tag: None,
            },
            Name::new(id).unwrap(),
            &[],
        )?;
        HubScheme::new(&account, 1_000_000, None, &[])
    }

    // spec/decisions/hub-scheme-names.md: the hub's asset names its scheme
    // and its ledger, and the scheme settles through the account it was
    // made with; an asset no scheme runs over is refused.
    #[test]
    fn a_hubs_asset_names_its_scheme_and_ledger() {
        let (xrp, _) = rules(4);
        let made = scheme(xrp.clone(), SCHEME_ID).unwrap();
        assert_eq!(made.id().as_str(), SCHEME_ID);
        assert_eq!(made.ledger(), LedgerId::HubXrplTestnetXrp);
        assert_eq!(made.asset(), super::super::account_id(SCHEME_ID));
        assert_eq!(made.unit(), UNITS_PER_DROP);

        let dsusd = with_terms(&xrp, token_terms(xrp.terms.clone()));
        dsusd.check().unwrap();
        let made = scheme(dsusd.clone(), DSUSD_SCHEME_ID).unwrap();
        assert_eq!(made.id().as_str(), DSUSD_SCHEME_ID);
        assert_eq!(made.ledger(), LedgerId::HubXrplTestnetDsusdUsd);
        assert_eq!(made.asset(), super::super::account_id(DSUSD_SCHEME_ID));
        assert_eq!(made.unit(), UNITS_PER_DROP, "a millionth is a drop's units");
        assert_ne!(
            super::super::account_id(SCHEME_ID),
            super::super::account_id(DSUSD_SCHEME_ID),
            "an account per hub"
        );

        let mut other = dsusd.terms.clone();
        other.asset = Asset::Token {
            currency: dassie_xrpl::rlusd::CURRENCY,
            issuer: dassie_xrpl::rlusd::ISSUER,
        };
        assert_eq!(scheme_of(&other), None, "no scheme over another token");
        let mut mainnet = xrp.terms.clone();
        mainnet.network = 0;
        assert_eq!(
            scheme_of(&mainnet),
            None,
            "mainnet's hub schemes are not built"
        );
        let other = with_terms(&xrp, other);
        assert!(matches!(
            scheme(other, DSUSD_SCHEME_ID),
            Err(SchemeError::InvalidConfig)
        ));
        assert_eq!(super::super::account_id("xrpl-testnet-xrp"), None);
    }
}
