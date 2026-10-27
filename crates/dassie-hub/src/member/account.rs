//! A member's account in a settlement hub, as an external asset
//! (spec/areas/hubs.md#the-members-side), sans-IO.
//!
//! The account talks to the hub's validators, each at its endpoint
//! ([`ExternalAsset::endpoints`]): it submits the member's operations and
//! asks what the log holds, and each validator answers with statements it
//! signs alone (`client.rs`).
//!
//! - **What it trusts.** A statement once f + 1 validators signed it alike
//!   ([`Agreement`]): one of them is honest.
//! - **What it reads, and when.** Every operation executed that touches the
//!   account, in the order of the log, each once. A validator that answers
//!   ends its answer with the newest final block it covered, and has sent
//!   every operation touching the account up to it; the account takes that
//!   word only from the validator's own endpoint. Once a quorum covered a
//!   height, f + 1 honest validators sent every such operation up to it, so
//!   each of them is agreed, and the account reads them, in order, up to
//!   that height (its cursor). A faulty validator can neither make up an
//!   operation nor hide one.
//! - **What it tells its scheme** ([`News`]): its own state as of a block
//!   through which it read everything touching it, each operation it read,
//!   reductions that took effect, and the newest final block up to which it
//!   read everything, which is what a payer waits for before it rejects a
//!   packet whose commitment expired.
//! - **Before it has an account.** It asks for the account of its key
//!   until f + 1 agree the hub has one, and reads as far as they agree it
//!   had none: what opened it after is read in order from there, a peer's
//!   `pay` among it (spec/designs/settlement-hubs.md#5-joining).
//! - **What it mirrors:** the holds it pays into, and those toward it from
//!   the peers its scheme follows ([`HoldMirror`]), from the operations on
//!   them; holds others open toward it in a small pool of their own.
//! - **When its history is gone.** Validators keep a bounded history; one
//!   that no longer keeps what touched the account since its cursor says so
//!   (`Trimmed`). Once f + 1 agree, or once one says so and its cursor
//!   has not moved for minutes (`STUCK_MS`), the account reads its state
//!   and every hold it pays into or takes from anew, as of one block f + 1
//!   agree on, and reads in order from there ([`News::Resynced`]).
//! - **What it submits:** the member's operations, one at a time, to every
//!   validator, again until one executes; signed with the account's next
//!   sequence once sent, and again with the right one after a refusal for
//!   its sequence. A settlement is sent only once the node executed it.
//!   A new account sets its disbursal address before anything else, as the
//!   hub requires.
//! - **A frozen hub** (spec/decisions/token-hubs.md). The account reads a
//!   token hub's freeze from what f + 1 validators sign of the issuer's
//!   hold ([`Statement::Issuer`]), the newest it agreed. While frozen, the
//!   hub takes no operation but claims whose commitments all expire within
//!   the freeze's window, and its validators drop what waits: the account
//!   keeps such an operation, refused or dropped, never counts it as
//!   failed, and sends it again after the thaw. A claim the hub takes goes
//!   meanwhile, beside an operation on its way that waits, with the same
//!   sequence, so that at most one of them ever applies. A claim past the
//!   window, and a redeem not yet sent, go back to the scheme, so the
//!   queue keeps its room for the claims the hub still pays. It takes no
//!   new settlement, withdrawal or redeem, and says it is not synced, so
//!   the hub's method is unavailable until the thaw.
//! - **Room for deadlines.** What has no deadline takes at most
//!   [`MAX_QUEUED_WITHOUT_DEADLINE`] of the queue, so claims and redeems
//!   always find room.
//!
//! What it sends a validator, and takes from one, is the hub's own wire
//! message (`service::wire::Message`): a `Request`, and the `Statements`
//! that answer it.
//!
//! One scheme settles through an account. Deposits from the underlying
//! ledger reach it only as what its state counts; the owner's withdrawals
//! it submits as any operation. Closing is not built yet.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey};
use dassie_ledger::Name;
use dassie_oer::Encode;
use dassie_settlement::{
    AssetEffect, AssetId, AssetKind, Detach, ExternalAsset, Freeze, Holding, LedgerInput,
    LedgerStatus, SchemeError,
};

use super::mirror::HoldMirror;
use crate::client::{AccountView, Agreement, MAX_STATEMENTS, Request, Statement};
use crate::consensus::Config;
use crate::service::MAX_ANSWER;

use crate::ledger::{Detail, Outcome, Refusal};
use crate::operation::{
    AccountNumber, Destination, MemberOperation, Operation, SignedMemberOperation,
};
use crate::promise::{HoldNumber, Promise, Signed};
use crate::rules::{Amount, HubId, MAX_HOLD_MS, Millis, Rules, Terms};
use crate::validators::{MAX_VALIDATORS, ValidatorSet};

/// The node's first hub account: in the default hub of the XRP Ledger
/// testnet.
pub const FIRST: AssetId = AssetId {
    kind: AssetKind::HubTestnetAccount,
    index: 0,
};

/// Most operations waiting to be sent, or on their way.
pub const MAX_QUEUED: usize = 16;
/// Most of them without a deadline: settlements, the owner's withdrawal
/// and the disbursal address. The rest of the queue is kept for claims and
/// redeems, which must go before their deadlines, frozen hub or not.
pub const MAX_QUEUED_WITHOUT_DEADLINE: usize = MAX_QUEUED / 2;
/// Most holds mirrored: those it pays into, and those toward it, which
/// anyone can open, each for a hold reserve.
pub const MAX_HOLDS: usize = 1_024;
/// Most holds mirrored from each payer the scheme follows: the hold it
/// follows, and the next one once that closes.
pub const MAX_HOLDS_PER_PEER: usize = 2;
/// Most holds mirrored toward us from accounts the scheme does not follow,
/// kept so that a peering made just after its peer opened its hold still
/// finds it; the oldest go first.
pub const MAX_STRANGER_HOLDS: usize = 64;
/// Most payers followed: one per peer.
const MAX_FOLLOWED: usize = super::scheme::MAX_PEERS;
/// Most final blocks remembered.
const MAX_FINALS: usize = 64;

/// Most agreed operations held until a quorum covered their height: two
/// of the longest answers a validator gives under `terms`, which carries up
/// to `MAX_ANSWER` statements and always one block whole, so an answer and
/// the next, asked while the first is on its way, always fit.
pub(super) fn max_buffered(terms: &Terms) -> usize {
    MAX_ANSWER
        .saturating_add(usize::from(terms.limits.block_operations))
        .saturating_mul(2)
}

/// Most news between two tellings: what one advance reads, and the rest.
fn max_news(terms: &Terms) -> usize {
    max_buffered(terms)
        .saturating_mul(2)
        .saturating_add(MAX_HOLDS + MAX_STRANGER_HOLDS + MAX_QUEUED + MAX_LOOKUPS + 8)
}

/// A tally of statements in which each validator's signatures on statements
/// still waiting fit a buffer's worth and a message more: an honest answer
/// is never refused.
fn agreement(hub: HubId, set: &ValidatorSet, terms: &Terms) -> Agreement {
    let quota = max_buffered(terms).saturating_add(MAX_STATEMENTS);
    Agreement::new(hub, set.clone(), quota.saturating_mul(set.len()))
}
/// Most messages between two takings: a round of requests and a
/// submission to every validator, a few times over.
const MAX_EFFECTS: usize = 8 * MAX_VALIDATORS;
/// How often it asks while it has holds or operations: a regional hub
/// makes a block final in well under this.
pub const POLL_MS: u64 = 100;
/// How often it asks otherwise.
pub const IDLE_POLL_MS: u64 = 1_000;
/// How long an operation sent waits before it is sent again.
pub const RESUBMIT_MS: u64 = 1_000;
/// How long the account waits for its cursor to move, once a validator
/// said it no longer keeps the history since the cursor, before it reads
/// its state anew. A quorum may no longer keep that history: with f
/// validators silent and an honest one restarted past the cursor, too few
/// cover it, and too few say `Trimmed` for f + 1 to agree, so the cursor
/// never moves.
///
/// Reading anew takes only what f + 1 sign alike, but it is not free: it
/// skips the blocks between the cursor and the block it reads, so a claim
/// a payee made on the hub there gives no fulfillment to pass upstream,
/// and what the hub paid for those packets is a loss on the peer
/// (`PaidLate`); the rest is reconciled through suspense rather than read
/// in order (`member/resync.rs`). So it must not start on one `Trimmed`
/// and an ordinary outage: a validator that restarts after the cursor
/// while another is cut off for a moment, as in a rolling restart. Each
/// heals within a few minutes: a node waits at most a minute before it
/// restarts a failed service, or before it reaches a peer again, and a
/// restarted validator catches up with its peers' last final block within
/// seconds. Five minutes leaves room for both at once, each at its longest
/// wait, and for the account's own asking (`IDLE_POLL_MS`).
pub const STUCK_MS: u64 = 5 * 60_000;
/// How often it asks for its own state, against which the scheme checks
/// its books.
pub const ACCOUNT_EVERY_MS: u64 = 2_000;
/// Most keys looked up at once.
pub const MAX_LOOKUPS: usize = 16;
/// Most recent states of the account kept, to match the holds read anew
/// with the account as of the same block.
const RECENT_VIEWS: usize = 8;

/// The holds of an account as one statement lists them.
type HoldList = Vec<(HoldNumber, crate::client::HoldView)>;

/// Pages of an account's holds as of one block (its height and timestamp):
/// how many there are, and those agreed so far.
type HoldPages = ((u64, Millis), u16, BTreeMap<u16, HoldList>);

/// Why an operation is submitted: what the scheme on the account is told
/// of it once it executed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Purpose {
    /// A settlement: a `hold-open` or `hold-fund` toward a peer, or the
    /// `pay` that opens the account of a peer that has none.
    Settle(Name),
    /// A `redeem` on a hold toward the account.
    Redeem(HoldNumber),
    /// A `claim` on a hold toward the account.
    Claim(HoldNumber),
    /// Setting the disbursal address, which a new account does first.
    Disbursal,
    /// The owner's withdrawal to the underlying ledger.
    Withdraw,
}

/// What the account learnt for its scheme, in the order it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum News {
    /// The account as of the block at `height`, through which every
    /// operation touching it was told, and none since: its balance then
    /// is its balance now.
    Account {
        /// The block's height.
        height: u64,
        /// The account.
        view: AccountView,
    },
    /// The hub has no account for the member's key.
    Unfunded,
    /// An operation that touches the account, in a final block, told once
    /// and in the order of the log.
    Executed {
        /// The block's height.
        height: u64,
        /// Its timestamp.
        timestamp: Millis,
        /// The operation.
        operation: SignedMemberOperation,
        /// What became of it.
        outcome: Outcome,
        /// Why we submitted it, if it is one of ours.
        ours: Option<Purpose>,
    },
    /// A reduction of a hold took effect: `amount` went back to its payer.
    /// A hold reduced to nothing closed.
    Reduced {
        /// The hold.
        hold: HoldNumber,
        /// What went back.
        amount: Amount,
    },
    /// A final block, up to which every operation touching the account was
    /// told.
    Final {
        /// Its height.
        height: u64,
        /// Its timestamp.
        timestamp: Millis,
    },
    /// An operation of ours the hub refused for good: it was never applied,
    /// and is not sent again.
    Dropped {
        /// Why we submitted it.
        purpose: Purpose,
    },
    /// The validators no longer keep what touched the account since its
    /// cursor, so it read its holds anew as of the block at `height`, and
    /// reads in order from there. What happened in between is known only by
    /// its difference: `before` is what it mirrored then, and the holds now
    /// are [`HubAccount::holds`]. An [`Account`](Self::Account) as of
    /// `height` follows, and a [`Final`](Self::Final).
    Resynced {
        /// The block's height.
        height: u64,
        /// Its timestamp.
        timestamp: Millis,
        /// The holds it mirrored before.
        before: Vec<(HoldNumber, HoldMirror)>,
        /// The operation of ours that was on its way and used its sequence
        /// in between: whether it applied is not known.
        unread: Option<(Purpose, MemberOperation)>,
    },
    /// The account of a key the scheme asked about
    /// ([`HubAccount::look_up`]).
    Member {
        /// The key.
        key: PublicKey,
        /// Its account, if it has one.
        account: Option<AccountNumber>,
        /// The block the answer is as of: none as of an older block says
        /// nothing of what happened since.
        height: u64,
    },
}

/// An operation agreed executed, held until a quorum covered its height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Executed {
    pub(super) timestamp: Millis,
    pub(super) operation: SignedMemberOperation,
    pub(super) outcome: Outcome,
}

/// What f + 1 validators last agreed of the issuer's hold on a token
/// hub's escrow ([`Statement::Issuer`]), and the block it is as of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IssuerState {
    pub(super) height: u64,
    /// The block's stamp.
    pub(super) timestamp: Millis,
    /// The stamp of the block from which the hub is frozen.
    pub(super) frozen: Option<Millis>,
    /// Whether the issuer holds the escrow.
    pub(super) held: bool,
}

/// The last expiry a commitment claimed on a hub frozen since the block
/// stamped `since` may have, as the hub's ledger reckons it
/// ([`Ledger::freeze_window_end`](crate::ledger::Ledger::freeze_window_end)):
/// the margin M, the longest a connector holds a packet and the precision
/// after the freeze's stamp.
#[must_use]
pub fn freeze_window_end(terms: &Terms, since: Millis) -> Millis {
    since
        .saturating_add(terms.timers.margin)
        .saturating_add(MAX_HOLD_MS)
        .saturating_add(Config::DEFAULT.precision_ms)
}

/// Whether the hub takes `operation` only after its thaw, while frozen
/// with the window ending at `window`: every operation but a claim whose
/// commitments all expire within it, as
/// [`Ledger::waits`](crate::ledger::Ledger::waits) has it.
pub(super) fn waits(window: Option<Millis>, operation: &MemberOperation) -> bool {
    let Some(end) = window else {
        return false;
    };
    match operation {
        MemberOperation::Claim { fulfilled, .. } => fulfilled
            .iter()
            .any(|item| item.commitment.value.expiry > end),
        _ => true,
    }
}

/// An operation of ours, waiting or on its way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Queued {
    pub(super) purpose: Purpose,
    pub(super) operation: MemberOperation,
    /// It may be sent: a settlement once the node executed it.
    pub(super) ready: bool,
    /// What was sent, and when: one at a time.
    pub(super) sent: Option<(SignedMemberOperation, u64)>,
}

/// A member's account in a hub.
pub struct HubAccount {
    pub(super) id: AssetId,
    pub(super) rules: Rules,
    pub(super) hub: HubId,
    pub(super) set: ValidatorSet,
    pub(super) endpoints: Vec<String>,
    pub(super) key: Arc<NodeKey>,
    pub(super) disbursal: Destination,
    pub(super) scheme: Name,
    // Kept with the books.
    pub(super) number: Option<AccountNumber>,
    /// Every operation touching the account up to this height was read.
    pub(super) cursor: u64,
    /// The height of the last one that may have changed its balance or a
    /// hold toward it.
    pub(super) last_touch: u64,
    pub(super) holds: BTreeMap<HoldNumber, HoldMirror>,
    /// Those of `holds` toward us from accounts the scheme does not follow.
    pub(super) strangers: BTreeSet<HoldNumber>,
    /// The accounts whose holds toward us the scheme follows.
    followed: BTreeSet<AccountNumber>,
    pub(super) queue: VecDeque<Queued>,
    // Learnt from the validators.
    sequence: Option<u64>,
    /// The sequence must be learnt again, from the account as of this
    /// height or later: an operation was refused for it there.
    stale_sequence: Option<u64>,
    /// Our `set-disbursal` was applied, whatever the account last said.
    disbursal_set: bool,
    /// The issuer's hold on a token hub's escrow, as last agreed; kept
    /// with the books, so a restart during a freeze knows it.
    pub(super) issuer: Option<IssuerState>,
    view: Option<(u64, AccountView)>,
    /// An agreed state of the account above the cursor, told once read up
    /// to it.
    waiting_view: Option<(u64, AccountView)>,
    unfunded: bool,
    heard: bool,
    /// The validators no longer keep what touched the account since its
    /// cursor: it reads its state anew.
    resync: bool,
    /// Since when a validator has said it no longer keeps what touched the
    /// account since its cursor, the cursor not having moved since.
    trimmed_since: Option<u64>,
    /// The holds read anew, waiting for the account as of the same block.
    resync_holds: Option<(u64, Millis, HoldList)>,
    /// The pages of the holds read anew agreed so far, as of one block, and
    /// how many there are.
    hold_pages: Option<HoldPages>,
    /// The account as agreed at recent heights.
    recent_views: BTreeMap<u64, AccountView>,
    /// Keys the scheme asked the account of.
    lookups: BTreeMap<[u8; 32], PublicKey>,
    agreement: Agreement,
    /// Per validator, the newest final block it covered from its own
    /// endpoint.
    covered: Vec<u64>,
    /// Per validator, whether a signature of its was dropped or refused since
    /// its last `Final` from its own endpoint: the answer that `Final`
    /// ends is not held whole, so it covers nothing.
    dirty: Vec<bool>,
    finals: BTreeMap<u64, Millis>,
    told_final: u64,
    /// Operations agreed executed, by block and their place in it: the
    /// order of the log.
    buffer: BTreeMap<(u64, u32), Executed>,
    failed: Option<&'static str>,
    stopped: bool,
    now_ms: u64,
    next_poll_ms: u64,
    last_account_ms: Option<u64>,
    wake_ms: Option<u64>,
    news: Vec<News>,
    effects: Vec<AssetEffect>,
}

impl HubAccount {
    /// The account `id` in the hub of `rules`, whose key is `key` (the node
    /// derives it at `node/hub/member/<hub ID>/account`), through which `scheme`
    /// settles, paying out to `disbursal` at shutdown, from the state the
    /// books kept.
    ///
    /// # Errors
    ///
    /// If the rules do not hold, or the state is not one the account
    /// wrote.
    pub fn new(
        id: AssetId,
        rules: Rules,
        key: Arc<NodeKey>,
        disbursal: Destination,
        scheme: Name,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        rules.check().map_err(|_| SchemeError::InvalidConfig)?;
        let set = rules.validator_set().ok_or(SchemeError::InvalidConfig)?;
        let hub = rules.id();
        let endpoints = rules
            .validators
            .iter()
            .map(|a| a.endpoint.clone())
            .collect();
        let n = set.len();
        let mut account = Self {
            id,
            hub,
            agreement: agreement(hub, &set, &rules.terms),
            set,
            rules,
            endpoints,
            key,
            disbursal,
            scheme,
            number: None,
            cursor: 0,
            last_touch: 0,
            holds: BTreeMap::new(),
            strangers: BTreeSet::new(),
            followed: BTreeSet::new(),
            queue: VecDeque::new(),
            sequence: None,
            stale_sequence: None,
            disbursal_set: false,
            issuer: None,
            view: None,
            waiting_view: None,
            unfunded: false,
            heard: false,
            resync: false,
            trimmed_since: None,
            resync_holds: None,
            hold_pages: None,
            recent_views: BTreeMap::new(),
            lookups: BTreeMap::new(),
            covered: alloc::vec![0; n],
            dirty: alloc::vec![false; n],
            finals: BTreeMap::new(),
            told_final: 0,
            buffer: BTreeMap::new(),
            failed: None,
            stopped: false,
            now_ms: 0,
            next_poll_ms: 0,
            last_account_ms: None,
            wake_ms: None,
            news: Vec::new(),
            effects: Vec::new(),
        };
        if !state.is_empty() {
            super::state::restore_account(&mut account, state).ok_or(SchemeError::InvalidData)?;
        }
        Ok(account)
    }

    /// The node key of the validator at `endpoint`, which the node's
    /// driver pins the connection to: coverage counts only from a
    /// validator's own endpoint (spec/areas/hubs.md#the-members-side).
    #[must_use]
    pub fn validator_node(&self, endpoint: usize) -> Option<dassie_identity::PublicKey> {
        self.rules
            .validators
            .get(endpoint)
            .map(|acceptance| acceptance.node)
    }

    /// The hub's ID.
    #[must_use]
    pub const fn hub(&self) -> HubId {
        self.hub
    }

    /// The hub's terms.
    #[must_use]
    pub const fn terms(&self) -> &Terms {
        &self.rules.terms
    }

    /// The hub's rules, as the account was built with them.
    #[must_use]
    pub const fn rules(&self) -> &Rules {
        &self.rules
    }

    /// The account's key.
    #[must_use]
    pub fn key(&self) -> PublicKey {
        self.key.public_key()
    }

    /// The key the account signs with, for the scheme's promises.
    #[must_use]
    pub fn signer(&self) -> Arc<NodeKey> {
        Arc::clone(&self.key)
    }

    /// Signs a promise to a payee with the account key.
    #[must_use]
    pub fn sign<T: Promise>(&self, value: T) -> Signed<T> {
        Signed::sign(value, &self.hub, &self.key)
    }

    /// The account's number, once the hub has said it.
    #[must_use]
    pub const fn number(&self) -> Option<AccountNumber> {
        self.number
    }

    /// The account as last agreed, and the height it is as of.
    #[must_use]
    pub const fn view(&self) -> Option<&(u64, AccountView)> {
        self.view.as_ref()
    }

    /// The sequence the account's next operation carries, as it learnt it:
    /// from the operations of its own it read, which raise it as they used
    /// theirs, and from the states the validators agree on. Never from
    /// what it sent.
    #[must_use]
    pub const fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    /// Every operation touching the account up to this height was read.
    #[must_use]
    pub const fn cursor(&self) -> u64 {
        self.cursor
    }

    /// The stamp of the block from which the hub is frozen, as f + 1
    /// validators last said (spec/decisions/token-hubs.md): its method is
    /// unavailable until the thaw. Never for an XRP hub.
    #[must_use]
    pub fn frozen(&self) -> Option<Millis> {
        self.issuer.and_then(|issuer| issuer.frozen)
    }

    /// Whether the issuer holds the hub's escrow, as f + 1 validators last
    /// said: frozen, or charging a transfer fee. A hub that ended while
    /// held is not frozen, but its escrow pays out only once released.
    #[must_use]
    pub fn escrow_held(&self) -> bool {
        self.issuer.is_some_and(|issuer| issuer.held)
    }

    /// While the hub is frozen, the last expiry a claim on it may carry:
    /// what the scheme claims then, and what the payee's bound on the
    /// commitments it takes keeps every packet it took before the freeze
    /// within (spec/decisions/token-hubs.md).
    #[must_use]
    pub fn freeze_window_end(&self) -> Option<Millis> {
        self.frozen()
            .map(|since| freeze_window_end(&self.rules.terms, since))
    }

    /// A hold it pays into or takes from.
    #[must_use]
    pub fn hold(&self, number: HoldNumber) -> Option<&HoldMirror> {
        self.holds.get(&number)
    }

    /// Every hold it pays into or takes from.
    pub fn holds(&self) -> impl Iterator<Item = (HoldNumber, &HoldMirror)> {
        self.holds.iter().map(|(number, hold)| (*number, hold))
    }

    /// What it learnt since its scheme was last told.
    #[must_use]
    pub fn news(&self) -> &[News] {
        &self.news
    }

    /// Whether it knows its account, and read up to the block it was told
    /// of it at.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        self.failed.is_none()
            && self.number.is_some()
            && self.view.is_some_and(|(height, _)| height <= self.cursor)
    }

    /// What the operations waiting or on their way take from the balance,
    /// at most: amounts, hold reserves and fees.
    #[must_use]
    pub fn committed(&self) -> Amount {
        let fees = &self.rules.terms.fees;
        self.queue
            .iter()
            .map(|queued| {
                let spent = match queued.operation {
                    MemberOperation::HoldOpen { amount, .. } => {
                        amount.saturating_add(fees.hold_reserve)
                    }
                    MemberOperation::HoldFund { amount, .. }
                    | MemberOperation::Pay { amount, .. } => amount,
                    MemberOperation::Withdraw { amount, .. } => amount.saturating_add(fees.ledger),
                    _ => 0,
                };
                spent.saturating_add(fees.operation)
            })
            .fold(0, Amount::saturating_add)
    }

    /// Whether an operation for `purpose` waits or is on its way.
    #[must_use]
    pub fn has(&self, purpose: Purpose) -> bool {
        self.queue.iter().any(|queued| queued.purpose == purpose)
    }

    /// Whether `purpose`'s operation was sent: it may execute.
    #[must_use]
    pub fn was_sent(&self, purpose: Purpose) -> bool {
        self.queue
            .iter()
            .any(|queued| queued.purpose == purpose && queued.sent.is_some())
    }

    /// Queues a settlement, `operation`, sent only once
    /// [`execute`](Self::execute)d.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] while the queue has no room for it, the
    /// account is not synced or the hub is frozen; [`SchemeError::InvalidData`] for
    /// a settlement it has.
    pub fn prepare(
        &mut self,
        settlement: Name,
        operation: MemberOperation,
    ) -> Result<(), SchemeError> {
        self.enqueue(Purpose::Settle(settlement), operation, false)
    }

    /// Queues an operation to send at once. One for the same purpose that
    /// was not sent yet is replaced, so a newer claim to redeem takes the
    /// place of an older one.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] while the queue is full or the account
    /// is not synced, or one for `purpose` is on its way; a withdrawal or a
    /// redeem also while the hub is frozen.
    pub fn submit(
        &mut self,
        purpose: Purpose,
        operation: MemberOperation,
    ) -> Result<(), SchemeError> {
        if let Some(queued) = self.queue.iter_mut().find(|q| q.purpose == purpose) {
            if queued.sent.is_some() {
                return Err(SchemeError::Unavailable);
            }
            queued.operation = operation;
            return Ok(());
        }
        self.enqueue(purpose, operation, true)?;
        self.pump();
        Ok(())
    }

    fn enqueue(
        &mut self,
        purpose: Purpose,
        operation: MemberOperation,
        ready: bool,
    ) -> Result<(), SchemeError> {
        if !self.is_synced() || self.queue.len() >= MAX_QUEUED {
            return Err(SchemeError::Unavailable);
        }
        let without_deadline = self
            .queue
            .iter()
            .filter(|q| !operation_has_deadline(&q.operation))
            .count();
        if !operation_has_deadline(&operation) && without_deadline >= MAX_QUEUED_WITHOUT_DEADLINE {
            return Err(SchemeError::Unavailable);
        }
        // A frozen hub moves nothing until the thaw: no settlement,
        // withdrawal or redeem is taken on to wait for it, so the queue
        // keeps its room for the claims it still pays. A redeem is made
        // again after the thaw, and the reduction's notice waits with it.
        if self.frozen().is_some()
            && matches!(
                purpose,
                Purpose::Settle(_) | Purpose::Withdraw | Purpose::Redeem(_)
            )
        {
            return Err(SchemeError::Unavailable);
        }
        if self.has(purpose) {
            return Err(SchemeError::InvalidData);
        }
        let queued = Queued {
            purpose,
            operation,
            ready,
            sent: None,
        };
        // Deadlines first: a claim or a redeem goes before what waits, but
        // never before what is on its way.
        let at = if operation_has_deadline(&queued.operation) {
            self.queue
                .iter()
                .position(|q| q.sent.is_none() && !operation_has_deadline(&q.operation))
                .unwrap_or(self.queue.len())
        } else {
            self.queue.len()
        };
        self.queue.insert(at, queued);
        Ok(())
    }

    /// The node executed settlement `settlement`: its pending transfer is
    /// on disk, so it may be sent. Whether the account has it.
    pub fn execute(&mut self, settlement: Name) -> bool {
        let found = self
            .queue
            .iter_mut()
            .find(|q| q.purpose == Purpose::Settle(settlement));
        let Some(queued) = found else {
            return false;
        };
        queued.ready = true;
        self.pump();
        true
    }

    /// Forgets a settlement the node could not book. One already sent is
    /// kept: it may execute.
    pub fn discard(&mut self, settlement: Name) {
        self.queue
            .retain(|q| q.purpose != Purpose::Settle(settlement) || q.sent.is_some());
    }

    /// Asks to be woken at `at_ms`: the scheme has something due then.
    pub fn wake(&mut self, at_ms: Option<u64>) {
        self.wake_ms = at_ms;
    }

    /// Asks the validators which account `key` has, until f + 1 agree:
    /// the answer comes as [`News::Member`]. At most [`MAX_LOOKUPS`] at
    /// once; past them, nothing is asked. Returns whether `key` is asked
    /// about.
    pub fn look_up(&mut self, key: PublicKey) -> bool {
        if self.lookups.len() < MAX_LOOKUPS {
            self.lookups.insert(*key.as_bytes(), key);
        }
        self.lookups.contains_key(key.as_bytes())
    }

    /// The accounts whose holds toward us the scheme follows: its peers'.
    /// A hold from one of them that waited among strangers' is followed
    /// from now on.
    pub fn follow(&mut self, payers: impl IntoIterator<Item = AccountNumber>) {
        self.followed = payers.into_iter().take(MAX_FOLLOWED).collect();
        let waiting: Vec<HoldNumber> = self
            .strangers
            .iter()
            .filter(|number| {
                self.holds
                    .get(number)
                    .is_some_and(|hold| self.followed.contains(&hold.payer))
            })
            .copied()
            .collect();
        for number in waiting {
            self.strangers.remove(&number);
            if let Some(mirror) = self.holds.remove(&number) {
                self.take_hold(number, mirror);
            }
        }
    }

    /// How many validators the hub has, and how many of them answered, from
    /// their own endpoints, up to the block the account has read to.
    #[must_use]
    pub fn answering(&self) -> (usize, usize) {
        let up_to_date = self
            .covered
            .iter()
            .filter(|height| **height > 0 && **height >= self.cursor)
            .count();
        (self.set.len(), up_to_date)
    }

    /// Whether it is reading its state anew.
    #[must_use]
    pub const fn is_resyncing(&self) -> bool {
        self.resync
    }

    /// Learns every validator's answer to a request: to all of them.
    fn ask(&mut self, request: &Request) {
        let message = crate::service::wire::Message::Request(request.clone()).to_vec();
        for endpoint in 0..self.endpoints.len() {
            if self.effects.len() >= MAX_EFFECTS {
                return;
            }
            self.effects.push(AssetEffect::SendTo {
                endpoint,
                message: message.clone(),
            });
        }
    }

    /// Asks what it needs to know: its account while it does not know it,
    /// its sequence, or from time to time its state; and what was executed
    /// since its cursor.
    fn poll(&mut self) {
        let account_due = self
            .last_account_ms
            .is_none_or(|at| self.now_ms >= at.saturating_add(ACCOUNT_EVERY_MS));
        if self.number.is_none() || self.stale_sequence.is_some() || account_due {
            self.last_account_ms = Some(self.now_ms);
            self.ask(&Request::Account(self.key.public_key()));
        }
        match self.number {
            Some(account) if self.resync => self.ask(&Request::Holds(account)),
            Some(account) => self.ask(&Request::Since {
                account,
                from: self.cursor.saturating_add(1),
            }),
            None => {}
        }
        let keys: Vec<PublicKey> = self.lookups.values().copied().collect();
        for key in keys {
            self.ask(&Request::Account(key));
        }
        let busy = !self.holds.is_empty()
            || !self.queue.is_empty()
            || !self.lookups.is_empty()
            || self.resync;
        let every = if busy { POLL_MS } else { IDLE_POLL_MS };
        self.next_poll_ms = self.now_ms.saturating_add(every);
    }

    /// When the account reads its state anew for want of its cursor
    /// moving since a validator said it no longer keeps the history since.
    fn stuck_at(&self) -> Option<u64> {
        if self.resync || self.number.is_none() {
            return None;
        }
        self.trimmed_since
            .map(|since| since.saturating_add(STUCK_MS))
    }

    /// One validator no longer keeps the history since the cursor, and the
    /// others have not moved it for `STUCK_MS` since: too few may keep it,
    /// so the account reads its state anew, asking at once.
    fn check_stuck(&mut self) {
        if self.stuck_at().is_some_and(|at| self.now_ms >= at) {
            self.resync = true;
            self.next_poll_ms = self.now_ms;
        }
    }

    /// Sends the next operation, or those on their way again.
    fn pump(&mut self) {
        if self.stopped || self.failed.is_some() || !self.is_synced() {
            return;
        }
        self.ensure_disbursal();
        let window = self.freeze_window_end();
        let now = self.now_ms;
        let on_way = self
            .queue
            .iter()
            .find_map(|q| q.sent.as_ref().map(|(signed, _)| signed.sequence));
        let Some(sequence) = on_way else {
            if let (Some(sequence), None) = (self.sequence, self.stale_sequence) {
                self.send_next(sequence, window);
            }
            return;
        };
        self.resend(now, window);
        // While all that is on its way waits for the thaw, a claim the
        // frozen hub takes goes beside it, with its sequence: whichever
        // applies first uses it, and the others can then never apply.
        let goes = self
            .queue
            .iter()
            .any(|q| q.sent.is_some() && !waits(window, &q.operation));
        if window.is_some() && !goes && self.stale_sequence.is_none() {
            self.send_next(sequence, window);
        }
    }

    /// Sends again what is on its way and the hub takes now, once it has
    /// waited [`RESUBMIT_MS`]: what waits for a thaw goes again after it.
    fn resend(&mut self, now: u64, window: Option<Millis>) {
        let mut again = Vec::new();
        for queued in &mut self.queue {
            if waits(window, &queued.operation) {
                continue;
            }
            if let Some((signed, sent_ms)) = &mut queued.sent
                && now >= sent_ms.saturating_add(RESUBMIT_MS)
            {
                *sent_ms = now;
                again.push(signed.clone());
            }
        }
        for signed in again {
            self.ask(&Request::Submit(Operation::Member(signed)));
        }
    }

    /// Signs the next operation that may go with `sequence`, and sends it.
    fn send_next(&mut self, sequence: u64, window: Option<Millis>) {
        let Some(account) = self.number else {
            return;
        };
        let needs_disbursal = !self.has_disbursal();
        let next = self.queue.iter_mut().find(|q| {
            q.sent.is_none()
                && q.ready
                && (!needs_disbursal || q.purpose == Purpose::Disbursal)
                && !waits(window, &q.operation)
        });
        let Some(queued) = next else {
            return;
        };
        let signed = SignedMemberOperation::sign(
            &self.hub,
            account,
            sequence,
            self.rules.terms.fees.operation,
            queued.operation.clone(),
            &self.key,
        );
        queued.sent = Some((signed.clone(), self.now_ms));
        self.ask(&Request::Submit(Operation::Member(signed)));
    }

    /// While the hub is frozen, a claim that it would not take in the
    /// freeze's window goes back to the scheme, which claims again what the
    /// window holds: one claim per hold at a time, so one that waits would
    /// keep the others from going. One on its way goes back once the
    /// account read every block up to the one it knows the freeze as of:
    /// it did not apply there, and no block after it takes it until the
    /// thaw. Its sequence is unused, so whatever is signed with it next
    /// applies at most once beside it. A redeem not yet sent goes back too,
    /// so that the queue keeps its room for claims: the scheme makes it
    /// again after the thaw. Only as the account takes an input of its own,
    /// so the scheme is told with what else it learnt.
    fn drop_waiting_claims(&mut self) {
        let window = self.freeze_window_end();
        let Some(known) = self.issuer.filter(|_| window.is_some()) else {
            return;
        };
        let read = self.cursor >= known.height;
        let mut dropped = Vec::new();
        self.queue.retain(|q| {
            let drop = match q.purpose {
                Purpose::Claim(_) => (q.sent.is_none() || read) && waits(window, &q.operation),
                Purpose::Redeem(_) => q.sent.is_none(),
                _ => false,
            };
            if drop {
                dropped.push(q.purpose);
            }
            !drop
        });
        for purpose in dropped {
            self.push_news(News::Dropped { purpose });
        }
    }

    /// Whether the account has its disbursal address: as last agreed, or
    /// since ours was applied.
    fn has_disbursal(&self) -> bool {
        self.disbursal_set || self.view.is_some_and(|(_, view)| view.disbursal)
    }

    /// Queues setting the disbursal address of an account without one,
    /// first: the hub takes nothing else from it before.
    ///
    /// Only by a state of the account as of everything it read: after a
    /// restart, one the validators agree on as of an older block may not
    /// show the address it set since, which is not set again.
    fn ensure_disbursal(&mut self) {
        if self.has_disbursal() || self.has(Purpose::Disbursal) {
            return;
        }
        if self.view.is_none_or(|(at, _)| at < self.last_touch) {
            return;
        }
        if self.queue.len() >= MAX_QUEUED {
            return;
        }
        self.queue.push_front(Queued {
            purpose: Purpose::Disbursal,
            operation: MemberOperation::SetDisbursal(self.disbursal),
            ready: true,
            sent: None,
        });
    }
}

/// Whether what is on its way is one operation, or, beside one that waits
/// for a thaw, claims a frozen hub takes: all with one sequence, and at most
/// one of them not a claim.
pub(super) fn on_their_way_hold(queue: &VecDeque<Queued>) -> bool {
    let mut sequences = queue
        .iter()
        .filter_map(|q| q.sent.as_ref().map(|(signed, _)| signed.sequence));
    let one_sequence = sequences
        .next()
        .is_none_or(|first| sequences.all(|sequence| sequence == first));
    let others = queue
        .iter()
        .filter(|q| q.sent.is_some() && !matches!(q.purpose, Purpose::Claim(_)))
        .count();
    one_sequence && others <= 1
}

/// Whether an operation has a deadline: a redeem or a claim.
const fn operation_has_deadline(operation: &MemberOperation) -> bool {
    operation.has_deadline()
}

/// Reading what the validators say.
impl HubAccount {
    /// Takes in one validator's message: signed statements. Anything
    /// else a validator sends a member means nothing to it.
    fn heard_from(&mut self, endpoint: usize, message: &[u8]) {
        let Ok(crate::service::wire::Message::Statements(statements)) =
            crate::service::wire::Message::decode_exact(message)
        else {
            return;
        };
        self.heard = true;
        for signed in statements {
            if !signed.verifies(&self.hub, &self.set) {
                continue;
            }
            if let Statement::Final { height, .. } = signed.statement
                && signed.validator.get() == endpoint
            {
                self.covers(endpoint, height);
            }
            // Only for an account we know: the cursor of one we do not is
            // set when we learn of it, from its state.
            if let Statement::Trimmed { below, .. } = signed.statement
                && below > self.cursor.saturating_add(1)
                && self.number.is_some()
                && self.trimmed_since.is_none()
            {
                self.trimmed_since = Some(self.now_ms);
            }
            if let Some(agreed) = self.agreement.add(signed) {
                self.agreed(agreed);
            }
            if let Some(dropped) = self.agreement.take_dropped() {
                self.uncover(dropped.signers.iter().map(|v| v.get()), dropped.lowest);
            }
        }
        self.advance();
        self.drop_waiting_claims();
        self.pump();
    }

    /// Validator `v` ended an answer with the final block at `height`: it
    /// covered everything up to it, unless the account dropped some of what
    /// it sent since its last `Final`.
    fn covers(&mut self, v: usize, height: u64) {
        let dirty = self.dirty.get_mut(v).map(core::mem::take);
        if dirty == Some(false)
            && let Some(covered) = self.covered.get_mut(v)
        {
            *covered = (*covered).max(height);
        }
    }

    /// Forgets everything read above the cursor, to read it again: no
    /// validator covers anything past the cursor until it answered anew.
    fn start_over(&mut self) {
        self.buffer.clear();
        self.agreement = agreement(self.hub, &self.set, &self.rules.terms);
        let cursor = self.cursor;
        self.uncover(0..self.set.len(), cursor.saturating_add(1));
    }

    /// The account dropped what `validators` sent from `lowest` on: their
    /// coverage stops below it, and the `Final` that ends what they are
    /// sending now covers nothing (spec/areas/hubs.md#the-members-side).
    fn uncover(&mut self, validators: impl Iterator<Item = usize>, lowest: u64) {
        let ceiling = lowest.saturating_sub(1).max(self.cursor);
        for v in validators {
            if let Some(covered) = self.covered.get_mut(v) {
                *covered = (*covered).min(ceiling);
            }
            if let Some(dirty) = self.dirty.get_mut(v) {
                *dirty = true;
            }
        }
    }

    /// A statement f + 1 validators signed alike.
    fn agreed(&mut self, statement: Statement) {
        match statement {
            Statement::Account {
                height,
                key,
                account,
            } if key == self.key.public_key() => self.account_agreed(height, account),
            Statement::Account {
                height,
                key,
                account,
            } => {
                if self.lookups.remove(key.as_bytes()).is_some() {
                    let account = account.map(|(number, _)| number);
                    self.push_news(News::Member {
                        key,
                        account,
                        height,
                    });
                }
            }
            // Its history since the cursor is gone: it reads its state anew.
            Statement::Trimmed { below, .. }
                if below > self.cursor.saturating_add(1) && self.number.is_some() =>
            {
                self.resync = true;
            }
            Statement::Holds {
                height,
                timestamp,
                account,
                page,
                pages,
                holds,
            } if self.resync && Some(account) == self.number && height > self.cursor => {
                self.hold_page((height, timestamp), (page, pages), holds);
            }
            // Whether it touches the account is read in order: a hold
            // opened toward it makes the operations on it its own.
            // Keyed by its place in the block: a faulty leader can put one
            // operation in a block twice, applied and then refused.
            Statement::Executed {
                height,
                timestamp,
                index,
                operation: Operation::Member(operation),
                outcome,
            } if height > self.cursor => {
                if self.buffer.len() >= max_buffered(&self.rules.terms) {
                    self.start_over();
                    return;
                }
                self.buffer.insert(
                    (height, index),
                    Executed {
                        timestamp,
                        operation,
                        outcome,
                    },
                );
            }
            Statement::Final { height, timestamp } => {
                self.finals.insert(height, timestamp);
                while self.finals.len() > MAX_FINALS {
                    self.finals.pop_first();
                }
            }
            issuer @ Statement::Issuer { .. } => self.issuer_agreed(&issuer),
            _ => {}
        }
    }

    /// A state of the issuer's hold f + 1 validators agree on: one of them
    /// is honest, so the hub was in it after that block. Only the newest
    /// counts.
    ///
    /// A freeze that ended moves the deadline of every reduction it
    /// mirrors that had not passed at the freeze back by the freeze's
    /// length, as the hub's thaw does: from the freeze's stamp to the
    /// thaw's, which the account knows only as no later than the block it
    /// learnt of the thaw at, or of the next freeze. So a reduction never
    /// applies here before it does in the hub, and a hold's records never
    /// close early; one that was due during a freeze the account missed
    /// whole applies early.
    fn issuer_agreed(&mut self, statement: &Statement) {
        let Statement::Issuer {
            height,
            timestamp,
            frozen,
            held,
        } = *statement
        else {
            return;
        };
        let state = IssuerState {
            height,
            timestamp,
            frozen,
            held,
        };
        if self
            .issuer
            .is_some_and(|known| known.height >= state.height)
        {
            return;
        }
        let before = self.issuer.replace(state);
        let Some(since) = before.and_then(|before| before.frozen) else {
            return;
        };
        if state.frozen == Some(since) {
            return;
        }
        let until = state.frozen.unwrap_or(state.timestamp);
        let length = until.saturating_sub(since);
        for mirror in self.holds.values_mut() {
            if let Some((_, at)) = mirror.reduction.as_mut()
                && *at > since
            {
                *at = at.saturating_add(length);
            }
        }
    }

    /// The account's state as agreed at `height`.
    fn account_agreed(&mut self, height: u64, account: Option<(AccountNumber, AccountView)>) {
        let Some((number, view)) = account else {
            if self.number.is_none() {
                // Nothing touched an account that did not exist: read up to
                // there, so a `pay` that opens it later is read in order.
                self.cursor = self.cursor.max(height);
                if !self.unfunded {
                    self.unfunded = true;
                    self.push_news(News::Unfunded);
                }
            }
            return;
        };
        match self.number {
            // The hub said it had no account for the key as of the cursor:
            // what opened it since is read in order from there, a peer's
            // `pay` among it, so the scheme knows who paid it in
            // (spec/designs/settlement-hubs.md#5-joining). Its state as of
            // `height` is told once read up to it.
            None if self.cursor > 0 && self.cursor < height => {
                self.number = Some(number);
                self.unfunded = false;
                // It asks at once what it has to read.
                self.next_poll_ms = self.now_ms;
            }
            None => {
                self.number = Some(number);
                self.unfunded = false;
                // Everything before is in the account's state.
                self.cursor = self.cursor.max(height);
                self.last_touch = self.last_touch.max(height);
            }
            Some(known) if known != number => {
                self.failed = Some("the hub names another account for the key");
                return;
            }
            Some(_) => {}
        }
        if self.view.is_none_or(|(at, _)| at <= height) {
            self.view = Some((height, view));
        }
        self.recent_views.insert(height, view);
        while self.recent_views.len() > RECENT_VIEWS {
            self.recent_views.pop_first();
        }
        // The hub's word on the sequence, once it is as of the refusal:
        // nothing of ours was sent since. Otherwise only ever up, as ours
        // execute.
        if self.stale_sequence.is_some_and(|refused| height >= refused) {
            self.stale_sequence = None;
            self.sequence = Some(view.sequence);
        } else {
            self.sequence = Some(self.sequence.unwrap_or(0).max(view.sequence));
        }
        self.waiting_view = Some((height, view));
        self.tell_view();
        self.try_resync();
    }

    /// Tells the account's state once everything up to it was read, and
    /// nothing touching it since.
    fn tell_view(&mut self) {
        let Some((height, view)) = self.waiting_view else {
            return;
        };
        if height > self.cursor {
            return;
        }
        self.waiting_view = None;
        if height >= self.last_touch {
            self.push_news(News::Account { height, view });
        }
    }

    /// Whether an operation touches the account: its own, or one on a hold
    /// it mirrors, or a hold opened toward it, or the `pay` that opened it.
    fn touches(&self, operation: &SignedMemberOperation) -> bool {
        let Some(number) = self.number else {
            return false;
        };
        if operation.account == number {
            return true;
        }
        match &operation.operation {
            MemberOperation::HoldOpen { payee, .. } => *payee == number,
            MemberOperation::Transfer { to, .. } => *to == number,
            MemberOperation::Pay { key, .. } => *key == self.key.public_key(),
            MemberOperation::HoldFund { hold, .. }
            | MemberOperation::HoldReduce { hold, .. }
            | MemberOperation::HoldRelease { hold }
            | MemberOperation::Claim { hold, .. } => self.holds.contains_key(hold),
            MemberOperation::Redeem(claim) => self.holds.contains_key(&claim.value.hold),
            _ => false,
        }
    }

    /// The highest height a quorum of validators covered, each from its
    /// own endpoint.
    fn quorum_covered(&self) -> u64 {
        let mut heights = self.covered.clone();
        heights.sort_unstable_by(|a, b| b.cmp(a));
        heights
            .get(self.set.quorum().saturating_sub(1))
            .copied()
            .unwrap_or(0)
    }

    /// Reads, in order, every operation a quorum covered.
    fn advance(&mut self) {
        let covered = self.quorum_covered();
        if covered <= self.cursor || self.number.is_none() || self.resync {
            return;
        }
        let later = self.buffer.split_off(&(covered.saturating_add(1), 0));
        let ready = core::mem::replace(&mut self.buffer, later);
        for ((height, _), executed) in ready {
            self.read(height, executed);
        }
        self.cursor = covered;
        self.trimmed_since = None;
        self.agreement.forget_below(covered.saturating_add(1));
        self.tell_final();
        self.tell_view();
    }

    /// Tells the newest agreed final block up to the cursor, and applies
    /// the reductions due by its timestamp.
    fn tell_final(&mut self) {
        let newest = self
            .finals
            .range(..=self.cursor)
            .next_back()
            .map(|(height, timestamp)| (*height, *timestamp));
        let Some((height, timestamp)) = newest else {
            return;
        };
        if height <= self.told_final {
            return;
        }
        self.told_final = height;
        self.apply_due(height, timestamp);
        self.push_news(News::Final { height, timestamp });
    }

    /// Applies the reductions due by a block stamped `timestamp`, by the
    /// hub's clock: while the hub is frozen, it stands at the freeze's
    /// stamp.
    fn apply_due(&mut self, height: u64, timestamp: Millis) {
        let timestamp = self
            .frozen()
            .map_or(timestamp, |since| timestamp.min(since));
        let numbers: Vec<HoldNumber> = self.holds.keys().copied().collect();
        for hold in numbers {
            let Some(mirror) = self.holds.get_mut(&hold) else {
                continue;
            };
            let Some(amount) = mirror.apply_due(timestamp) else {
                continue;
            };
            if mirror.amount == 0 {
                self.drop_hold(hold);
            }
            self.last_touch = self.last_touch.max(height);
            self.push_news(News::Reduced { hold, amount });
        }
    }

    /// Reads one operation that touches the account.
    fn read(&mut self, height: u64, executed: Executed) {
        let Executed {
            timestamp,
            operation,
            outcome,
        } = executed;
        if !self.touches(&operation) {
            return;
        }
        self.apply_due(height, timestamp);
        let ours = self.settle_ours(height, &operation, outcome);
        if matches!(outcome, Outcome::Applied { .. }) {
            self.mirror(&operation, outcome, timestamp);
        }
        self.last_touch = self.last_touch.max(height);
        if ours.is_some() || !matches!(outcome, Outcome::Refused(_)) {
            self.push_news(News::Executed {
                height,
                timestamp,
                operation,
                outcome,
                ours,
            });
        }
    }

    /// What became of an operation of ours: the purpose it was for, once
    /// it executed or was refused for good.
    fn settle_ours(
        &mut self,
        height: u64,
        operation: &SignedMemberOperation,
        outcome: Outcome,
    ) -> Option<Purpose> {
        if Some(operation.account) != self.number {
            return None;
        }
        if outcome.used_sequence() {
            let next = operation.sequence.saturating_add(1);
            self.sequence = Some(self.sequence.unwrap_or(0).max(next));
            // What else of ours went with that sequence, or an earlier
            // one, can never apply now: it is signed again with the next.
            for queued in &mut self.queue {
                if queued.sent.as_ref().is_some_and(|(sent, _)| {
                    sent.sequence <= operation.sequence && sent != operation
                }) {
                    queued.sent = None;
                }
            }
        }
        let position = self
            .queue
            .iter()
            .position(|q| q.sent.as_ref().is_some_and(|(sent, _)| sent == operation))?;
        // A frozen hub took it in a block all the same and refused it: it
        // waits for the thaw, kept as it is on its way, and is sent again
        // then. Never a failure: it used no sequence. A claim the account
        // knows waits goes back to the scheme, which claims again what the
        // freeze's window holds (`drop_waiting_claims`).
        if outcome == Outcome::Refused(Refusal::Frozen) {
            let window = self.freeze_window_end();
            let back = self.queue.get(position).is_some_and(|q| {
                matches!(q.purpose, Purpose::Claim(_)) && waits(window, &q.operation)
            });
            if back && let Some(queued) = self.queue.remove(position) {
                self.push_news(News::Dropped {
                    purpose: queued.purpose,
                });
            }
            return None;
        }
        if outcome == Outcome::Refused(Refusal::Sequence) {
            // Signed again with the sequence the hub says.
            if let Some(queued) = self.queue.get_mut(position) {
                queued.sent = None;
            }
            self.stale_sequence = Some(height);
            self.last_account_ms = None;
            return None;
        }
        let queued = self.queue.remove(position)?;
        if let Outcome::Refused(_) = outcome {
            self.push_news(News::Dropped {
                purpose: queued.purpose,
            });
            return None;
        }
        if queued.purpose == Purpose::Disbursal {
            match outcome {
                Outcome::Applied { .. } => self.disbursal_set = true,
                // An address the hub cannot pay: every operation after it
                // would fail and pay its fee.
                _ => self.failed = Some("the hub refused the disbursal address"),
            }
        }
        Some(queued.purpose)
    }

    /// Follows an applied operation on the holds it touches.
    fn mirror(&mut self, operation: &SignedMemberOperation, outcome: Outcome, timestamp: Millis) {
        let Outcome::Applied { detail, .. } = outcome else {
            return;
        };
        let paid = match detail {
            Detail::Paid(paid) => paid,
            _ => 0,
        };
        let notice = self.rules.terms.timers.hold_notice;
        match &operation.operation {
            MemberOperation::HoldOpen { payee, amount } => {
                if let Detail::Hold(number) = detail {
                    let mirror = HoldMirror::opened(operation.account, *payee, *amount);
                    self.take_hold(number, mirror);
                }
            }
            MemberOperation::HoldFund { hold, amount } => {
                if let Some(mirror) = self.holds.get_mut(hold) {
                    mirror.fund(*amount);
                }
            }
            MemberOperation::HoldReduce { hold, amount } => {
                if let Some(mirror) = self.holds.get_mut(hold) {
                    mirror.reduce(*amount, timestamp, notice);
                }
            }
            MemberOperation::HoldRelease { hold } => self.drop_hold(*hold),
            MemberOperation::Redeem(claim) => {
                if let Some(mirror) = self.holds.get_mut(&claim.value.hold) {
                    mirror.redeem(&claim.value, paid);
                }
            }
            MemberOperation::Claim { hold, fulfilled } => {
                if let Some(mirror) = self.holds.get_mut(hold) {
                    mirror.claim(fulfilled, paid);
                }
            }
            _ => {}
        }
    }

    /// Mirrors hold `number`: one of ours, or toward us from a payer the
    /// scheme follows, up to [`MAX_HOLDS`] (and [`MAX_HOLDS_PER_PEER`] from
    /// each payer); any other toward us in a pool of its own, whose oldest
    /// goes first, so holds anyone may open toward us never take the place
    /// of ours or our peers'.
    fn take_hold(&mut self, number: HoldNumber, mirror: HoldMirror) {
        let ours = Some(mirror.payer) == self.number;
        let from_payer = self
            .holds
            .iter()
            .filter(|(n, h)| h.payer == mirror.payer && !self.strangers.contains(n))
            .count();
        let followed =
            ours || (self.followed.contains(&mirror.payer) && from_payer < MAX_HOLDS_PER_PEER);
        if followed {
            if self.followed_holds() < MAX_HOLDS {
                self.holds.insert(number, mirror);
            }
            return;
        }
        self.holds.insert(number, mirror);
        self.strangers.insert(number);
        while self.strangers.len() > MAX_STRANGER_HOLDS {
            if let Some(oldest) = self.strangers.pop_first() {
                self.holds.remove(&oldest);
            }
        }
    }

    /// Forgets hold `number`: it closed.
    fn drop_hold(&mut self, number: HoldNumber) {
        self.holds.remove(&number);
        self.strangers.remove(&number);
    }

    /// How many holds it mirrors that are its own or its peers'.
    fn followed_holds(&self) -> usize {
        self.holds.len().saturating_sub(self.strangers.len())
    }

    /// Gathers the pages of the holds as of one block: the newest block
    /// any page was agreed as of. Once every page is in, the list is whole.
    fn hold_page(
        &mut self,
        (height, timestamp): (u64, Millis),
        (page, pages): (u16, u16),
        holds: HoldList,
    ) {
        let newer = self
            .hold_pages
            .as_ref()
            .is_none_or(|(at, _, _)| *at < (height, timestamp));
        if newer {
            self.hold_pages = Some(((height, timestamp), pages, BTreeMap::new()));
        }
        let Some((at, count, gathered)) = self.hold_pages.as_mut() else {
            return;
        };
        if *at != (height, timestamp) || *count != pages {
            return;
        }
        gathered.insert(page, holds);
        if gathered.len() < usize::from(pages) {
            return;
        }
        let whole: HoldList = gathered.values().flatten().copied().collect();
        self.hold_pages = None;
        self.resync_holds = Some((height, timestamp, whole));
        self.try_resync();
    }

    /// Reads the account anew once the holds and the account agreed as of
    /// one block are both in, above the cursor.
    fn try_resync(&mut self) {
        let Some((height, timestamp, holds)) = self.resync_holds.take() else {
            return;
        };
        if !self.resync || height <= self.cursor {
            return;
        }
        let Some(view) = self.recent_views.get(&height).copied() else {
            self.resync_holds = Some((height, timestamp, holds));
            return;
        };
        self.resynced(height, timestamp, view, holds);
    }

    /// Takes the account and its holds as of the block at `height` for
    /// what it read up to there. A hold keeps what `claim` took above its
    /// W as far as the account knew it: the view does not say, and a
    /// redeem since would have passed it.
    fn resynced(&mut self, height: u64, timestamp: Millis, view: AccountView, holds: HoldList) {
        let unread = self.take_unread(view.sequence);
        let before = core::mem::take(&mut self.holds);
        self.strangers.clear();
        for (number, hold) in holds {
            let mut mirror = HoldMirror::of(&hold);
            if let Some(old) = before.get(&number) {
                mirror.claimed = old.claimed.clone();
                mirror
                    .claimed
                    .retain(|sequence, _| *sequence > hold.through);
            }
            self.take_hold(number, mirror);
        }
        self.sequence = Some(view.sequence);
        self.stale_sequence = None;
        self.view = Some((height, view));
        self.cursor = height;
        self.trimmed_since = None;
        self.last_touch = height;
        self.buffer = self.buffer.split_off(&(height.saturating_add(1), 0));
        self.agreement.forget_below(height.saturating_add(1));
        self.resync = false;
        self.told_final = height;
        if self.waiting_view.is_some_and(|(at, _)| at <= height) {
            self.waiting_view = None;
        }
        self.push_news(News::Resynced {
            height,
            timestamp,
            before: before.into_iter().collect(),
            unread,
        });
        self.push_news(News::Final { height, timestamp });
        self.push_news(News::Account { height, view });
    }

    /// The operation of ours on its way whose sequence the account has used
    /// by now: it executed while its history was not read. Of several on
    /// their way with that sequence, a claim beside one that waited for a
    /// thaw, the one that is not a claim: the state read anew shows whether
    /// it applied, and the claim, sent again, fails at worst. The others
    /// can never apply now, and are signed again.
    fn take_unread(&mut self, sequence: u64) -> Option<(Purpose, MemberOperation)> {
        let used = |q: &Queued| {
            q.sent
                .as_ref()
                .is_some_and(|(signed, _)| signed.sequence < sequence)
        };
        let position = self
            .queue
            .iter()
            .position(|q| used(q) && !matches!(q.purpose, Purpose::Claim(_)))
            .or_else(|| self.queue.iter().position(used))?;
        let queued = self.queue.remove(position)?;
        for other in &mut self.queue {
            if used(other) {
                other.sent = None;
            }
        }
        if queued.purpose == Purpose::Disbursal {
            self.disbursal_set = true;
        }
        Some((queued.purpose, queued.operation))
    }

    /// Keeps news for the scheme. The node tells it after every input, and
    /// one input tells at most a buffer's worth and a reduction per hold;
    /// past the bound the account stops rather than forget any.
    fn push_news(&mut self, news: News) {
        if self.news.len() >= max_news(&self.rules.terms) {
            self.failed = Some("more news than the scheme was told");
            return;
        }
        self.news.push(news);
    }
}

impl ExternalAsset for HubAccount {
    fn id(&self) -> AssetId {
        self.id
    }

    /// Its XRP in the hub: the balance and the account reserve, as last
    /// agreed.
    fn holdings(&self) -> Vec<Holding> {
        self.view
            .iter()
            .map(|(_, view)| Holding {
                code: "XRP".into(),
                issuer: None,
                amount: u128::from(view.balance).saturating_add(u128::from(view.reserve)),
                scale: 6,
                reserve: u128::from(view.reserve),
                freeze: Freeze::default(),
            })
            .collect()
    }

    fn status(&self) -> LedgerStatus {
        if let Some(why) = self.failed {
            return LedgerStatus::Failed(why);
        }
        // A frozen hub's method is unavailable until the thaw
        // (spec/decisions/token-hubs.md): the account follows the hub all
        // the same, but the node moves nothing into it and takes no packet
        // over it meanwhile.
        if self.is_synced() && self.frozen().is_none() {
            LedgerStatus::Synced
        } else if self.is_synced() {
            LedgerStatus::Syncing
        } else if self.unfunded {
            LedgerStatus::Unfunded
        } else if self.heard {
            LedgerStatus::Syncing
        } else {
            LedgerStatus::Connecting
        }
    }

    fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    fn on_ledger(&mut self, now_ms: u64, input: LedgerInput<'_>) {
        self.now_ms = self.now_ms.max(now_ms);
        if self.failed.is_some() || self.stopped {
            return;
        }
        if let LedgerInput::From { endpoint, message } = input
            && endpoint < self.endpoints.len()
        {
            self.heard_from(endpoint, message);
        }
    }

    fn deadline(&self) -> Option<u64> {
        if self.failed.is_some() || self.stopped {
            return None;
        }
        // An operation on its way is sent again only once the account is
        // in step with the hub (`pump`): until then it is not due, or a
        // restored one, sent at 0, would wake the host over and over. One
        // that waits for a thaw is not due before it.
        let window = self.freeze_window_end();
        let resend = self
            .queue
            .iter()
            .filter(|q| !waits(window, &q.operation))
            .filter_map(|q| q.sent.as_ref())
            .filter(|_| self.is_synced())
            .map(|(_, sent)| sent.saturating_add(RESUBMIT_MS))
            .min();
        [
            Some(self.next_poll_ms),
            resend,
            self.wake_ms,
            self.stuck_at(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn on_timer(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
        if self.failed.is_some() || self.stopped {
            return;
        }
        if self.wake_ms.is_some_and(|at| at <= self.now_ms) {
            self.wake_ms = None;
        }
        self.check_stuck();
        self.drop_waiting_claims();
        if self.now_ms >= self.next_poll_ms {
            self.poll();
        }
        self.pump();
    }

    fn told(&mut self) {
        self.news.clear();
    }

    fn take_effects(&mut self, out: &mut Vec<AssetEffect>) {
        out.append(&mut self.effects);
    }

    fn detach(&mut self, scheme: Name, why: Detach) {
        // Its one scheme stopped or went: nothing is read or sent until it
        // is back, after a restart, so nothing is read that it misses.
        let _ = why;
        if scheme == self.scheme {
            self.stopped = true;
        }
    }

    fn state(&self) -> Vec<u8> {
        super::state::account_state(self)
    }

    fn state_version(&self) -> u64 {
        super::state::fingerprint(&self.state())
    }

    fn check_invariants(&self) {
        assert!(self.queue.len() <= MAX_QUEUED);
        assert!(self.followed_holds() <= MAX_HOLDS);
        assert!(self.strangers.len() <= MAX_STRANGER_HOLDS);
        assert!(
            self.strangers.iter().all(|n| self.holds.contains_key(n)),
            "a stranger's hold not mirrored"
        );
        assert!(self.followed.len() <= MAX_FOLLOWED);
        assert!(self.buffer.len() <= max_buffered(&self.rules.terms));
        assert!(self.finals.len() <= MAX_FINALS);
        assert!(self.lookups.len() <= MAX_LOOKUPS);
        assert!(self.recent_views.len() <= RECENT_VIEWS);
        assert!(
            self.resync_holds
                .as_ref()
                .is_none_or(|(height, _, _)| *height > self.cursor),
            "holds read anew are above the cursor"
        );
        assert!(
            self.hold_pages
                .as_ref()
                .is_none_or(|(_, pages, gathered)| gathered.len() < usize::from(*pages)
                    && usize::from(*pages) <= crate::client::MAX_HOLD_PAGES),
            "pages of holds past their bound"
        );
        assert!(self.news.len() <= max_news(&self.rules.terms));
        assert!(self.effects.len() <= MAX_EFFECTS);
        assert_eq!(self.covered.len(), self.set.len());
        assert_eq!(self.dirty.len(), self.set.len());
        assert!(
            on_their_way_hold(&self.queue),
            "one operation on its way at a time, or claims beside it"
        );
        assert!(
            self.buffer.keys().all(|(height, _)| *height > self.cursor),
            "what is held is above the cursor"
        );
        assert!(
            self.last_touch <= self.cursor,
            "nothing touched beyond what was read"
        );
        if let Some(number) = self.number {
            for hold in self.holds.values() {
                assert!(
                    hold.payer == number || hold.payee == number,
                    "a hold mirrored is the account's"
                );
                assert!(hold.claimed.len() <= usize::from(u16::MAX));
            }
        }
        let mut purposes: Vec<Purpose> = self.queue.iter().map(|q| q.purpose).collect();
        purposes.sort_unstable();
        purposes.dedup();
        assert_eq!(
            purposes.len(),
            self.queue.len(),
            "one operation per purpose"
        );
    }
}
