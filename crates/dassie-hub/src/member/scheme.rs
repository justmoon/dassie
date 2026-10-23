//! The `hub-xrp-testnet` settlement scheme (spec/areas/hubs.md#the-members-side),
//! sans-IO: a member's side of peering over a settlement hub, through its
//! [`HubAccount`].
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
//! Deposits and withdrawals by the owner, moving funds between the hub and
//! the node's XRPL account, and joining a hub by being paid are not built
//! yet: [`deposit`](SettlementScheme::deposit) is unsupported.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use dassie_identity::{NodeKey, PublicKey};
use dassie_ledger::{LedgerId, Name};
use dassie_oer::{Encode, Reader};
use dassie_settlement::policy::redeem_threshold;
use dassie_settlement::{
    AssetId, Commitments, Decided, Effect, ExternalAsset, LedgerStatus, LossCause,
    PreparedSettlement, Promised, ReconcileCause, SchemeError, SettlementScheme,
};
use rand_core::CryptoRng;

use super::account::{HubAccount, News, Purpose};
use super::payee::{Claiming, Payee};
use super::payer::{Loss, Payer, Snapshot};
use super::peering::Membership;
use super::{UNITS_PER_DROP, digest_name, units};
use crate::client::AccountView;
use crate::digest::Digest;
use crate::ledger::{Detail, Outcome};
use crate::operation::{AccountNumber, MemberOperation, Operation, SignedMemberOperation};
use crate::promise::{Claim, Commitment, HoldNumber, Signed};
use crate::rules::{Amount, HubId, Terms};

/// The scheme's ID.
pub const SCHEME_ID: &str = "hub-xrp-testnet";

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

/// A peer over the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Peer {
    pub(super) account: AccountNumber,
    pub(super) key: PublicKey,
    /// Our hold toward it.
    pub(super) payer: Payer,
    /// Its hold toward us.
    pub(super) payee: Payee,
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
}

/// A settlement of ours not yet applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settling {
    pub(super) peer: Name,
    pub(super) drops: Amount,
}

/// The `hub-xrp-testnet` scheme.
pub struct HubScheme {
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
    // Learnt.
    reserve: Amount,
    strangers: BTreeMap<HoldNumber, SignedMemberOperation>,
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
    /// If the state is not one the scheme wrote, or is not in step with
    /// the account's.
    pub fn new(
        account: &HubAccount,
        max_packet: u64,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let mut scheme = Self {
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
            reserve: 0,
            strangers: BTreeMap::new(),
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
            // whose operations we never read.
            News::Member {
                key,
                account: Some(number),
            } => {
                for peer in self.peers.values_mut() {
                    if peer.key == *key && peer.account == *number {
                        peer.payee.key_confirmed = true;
                    }
                }
            }
            News::Member { account: None, .. } => {}
            News::Reduced { .. } | News::Unfunded => {}
        }
    }

    /// Makes the books hold what the account and the holds toward us from
    /// our peers held as of a block: on the first sync, the owner's
    /// deposit; after, a difference reconciled through suspense.
    fn match_balance(
        &mut self,
        account: &HubAccount,
        view: &AccountView,
        effects: &mut Vec<Effect>,
    ) {
        self.account = account.number();
        let toward_us: Amount = self
            .peers
            .values()
            .filter_map(|peer| peer.payee.hold)
            .filter_map(|hold| account.hold(hold).map(|mirror| mirror.amount))
            .fold(0, Amount::saturating_add);
        let target = units(view.balance.saturating_add(toward_us));
        if !self.synced {
            self.synced = true;
            if target > self.booked {
                effects.push(Effect::Deposit {
                    amount: target - self.booked,
                });
                self.booked = target;
            }
        }
        if target != self.booked {
            effects.push(Effect::Reconcile {
                amount: target.abs_diff(self.booked),
                gain: target > self.booked,
                cause: ReconcileCause::Unexplained,
            });
            self.booked = target;
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
                self.settled(settlement, operation, applied, detail, effects)
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
        self.peers
            .values()
            .filter_map(|peer| peer.payee.next_due(timing))
            .min()
    }

    fn peer(&mut self, name: Name) -> Option<&mut Peer> {
        self.peers.get_mut(&name)
    }

    /// Whether `peer` may peer as `membership`: as it did before, or, once
    /// nothing of its old membership is left to settle, as another. A peer
    /// that came back with another account would leave behind what we
    /// fulfilled and never claimed; with another key, it would have us
    /// take promises the hub refuses.
    fn may_peer(&self, peer: Name, membership: &Membership) -> Result<(), SchemeError> {
        match self.peers.get(&peer) {
            Some(known)
                if (known.account != membership.account || known.key != membership.key)
                    && known.unresolved() =>
            {
                Err(SchemeError::Unavailable)
            }
            _ => Ok(()),
        }
    }
}

impl SettlementScheme for HubScheme {
    fn id(&self) -> Name {
        dassie_ledger::name!(SCHEME_ID)
    }

    fn ledger(&self) -> LedgerId {
        LedgerId::HubXrpTestnetXrp
    }

    fn asset(&self) -> Option<AssetId> {
        Some(super::account::FIRST)
    }

    fn supported_versions(&self) -> &'static [u16] {
        &[1]
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
        let news = account.news().to_vec();
        for item in &news {
            self.tell(account, item, effects);
        }
        // Holds toward us are followed from our peers only.
        account.follow(self.peers.values().map(|peer| peer.account));
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

    fn peering_info(&self, out: &mut Vec<u8>) {
        out.clear();
        if let Some(account) = self.account {
            Membership {
                hub: self.hub,
                account,
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
        Membership::read(peering_info, &self.hub)?;
        self.peering_info(out);
        if out.is_empty() {
            return Err(SchemeError::Unavailable);
        }
        Ok(())
    }

    fn accept_peering_request(
        &mut self,
        peer: Name,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError> {
        let membership = Membership::read(data, &self.hub)?;
        if !self.peers.contains_key(&peer) && self.peers.len() >= MAX_PEERS {
            return Err(SchemeError::Unavailable);
        }
        self.may_peer(peer, &membership)?;
        response.clear();
        Ok(data.to_vec())
    }

    fn finalize_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        _data: &[u8],
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
            if known.account == membership.account && known.key == membership.key {
                // Peered again as before: its holds stay, and so do our
                // records of them.
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
    /// or the balance cannot pay it.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        asset: Option<&mut dyn ExternalAsset>,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        let membership = Membership::read(peer_state, &self.hub)?;
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        let drops = amount / UNITS_PER_DROP;
        if drops == 0 || self.settling.len() >= MAX_SETTLING {
            return Err(SchemeError::Unavailable);
        }
        let opening = self.settling.values().any(|s| s.peer == peer);
        let hold = self.peers.get(&peer).and_then(|p| p.payer.hold);
        // Opening a hold locks its reserve besides what it holds.
        let reserve = match hold {
            Some(_) => 0,
            None if opening => return Err(SchemeError::Unavailable),
            None => self.terms.fees.hold_reserve,
        };
        let balance = account.view().map_or(0, |(_, view)| view.balance);
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
        let mut bytes = [0u8; 16];
        rng.fill_bytes(&mut bytes);
        let id = Name::hex(&bytes, false);
        account.prepare(id, operation)?;
        self.settling.insert(id, Settling { peer, drops });
        Ok(PreparedSettlement {
            id,
            amount: drops * UNITS_PER_DROP,
            message: bytes.to_vec(),
            peer_state: None,
        })
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
    /// written together, so one that executed was booked with them.
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
            if !account.execute(*settlement) {
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

    fn handle_message(&mut self, _peer: Name, _message: &[u8], _effects: &mut Vec<Effect>) {}

    fn deposit(&mut self, _amount: u64, _effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
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
        if self.synced {
            assert!(self.account.is_some(), "a synced scheme knows its account");
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

    /// The hub, and our account in it once it has its number.
    fn membership(&self) -> Option<dassie_settlement::Membership> {
        Some(dassie_settlement::Membership {
            hub: *self.hub.0.as_bytes(),
            account: self.account?.0,
        })
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
