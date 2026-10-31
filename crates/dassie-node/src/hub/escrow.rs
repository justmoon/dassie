//! Following a hub's escrow on the XRP Ledger, as a validator does
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals), without
//! IO: the requests to send on its own WebSocket to its XRPL server, the
//! validator lists to fetch, and what it attests from what it proved.
//!
//! It takes nothing on the server's word. A [`Verifier`] of its own,
//! trusting the hub's network's UNL, proves everything it attests against
//! ledgers that UNL validated (spec/designs/xrpl-validation.md#12-who-else-uses-it):
//!
//! - **Transactions by their proofs, found by the history the escrow's
//!   state names.** The escrow's account root and a token hub's line each
//!   name the transaction that last changed them, and each proven
//!   transaction's metadata the one that changed them before: from each
//!   anchor, the watcher walks these back to the ledger its last walk
//!   reached (`Walk`), as the node's account walks its own
//!   (spec/areas/xrpl.md#following-the-account), so a transaction its
//!   server leaves out of the escrow's history and of its stream is found
//!   all the same. The stream and the history (`account_tx`) only hint at
//!   hashes, sooner; each hint is proven, oldest first and many at a time,
//!   and its proof kept for the walk, which starts no higher than the
//!   hints proven cover and steps through their proofs rather than proving
//!   each in turn (`walk_top`): so a burst of payments to the
//!   escrow delays it no more than proving their hints takes.
//!   Each transaction is proven by its own bytes and metadata in a
//!   validated ledger, and read as `dassie_hub::escrow` reads it: a
//!   deposit, the outcome of a pending withdrawal, the old validators'
//!   change of the signer list, and for a token hub the issuer's clawback.
//!   A hint that proves nothing is attested never. The cursor passes a
//!   ledger only once a walk passed it, and no further than the newest
//!   proven ledger. A walk holds one transaction per object at a time, and
//!   one with no room yet among what waits for the hub waits rather than
//!   being dropped; a hint the stream brings past their bound
//!   ([`MAX_HINTS`]) has the history read again from its ledger: so no
//!   number of small payments to the escrow makes it skip a deposit.
//! - **State by state proofs, at a fresh anchor.** The escrow's setup is
//!   checked, while no quorum has, from proofs of its account root, its
//!   signer list and a token hub's line at one validated ledger: as soon
//!   as one of the escrow's own transactions is proven to apply, as the
//!   sponsor's setup does, or a founding asks, at most once per anchor,
//!   and every [`ESCROW_CHECK_EVERY_MS`] besides; a token
//!   hub's issuer's hold (a freeze of the escrow's line, a global freeze, a
//!   transfer fee) from proofs of the line and the issuer's account root,
//!   named by the ledger it last changed in, which the watcher finds by
//!   proving the two as of the ledgers before (`HoldSearch`), so that
//!   validators name the same ledger however busy the issuer and the
//!   escrow are; and attested only where it differs from the hub's. What
//!   is judged as of the newest ledger is judged only from a proof as of
//!   the newest anchor, while it is fresh ([`ANCHOR_FRESH_MS`]): an old
//!   anchor delays attestations, never invents them.
//! - **Resets by the verifier.** A reset of the test network is attested
//!   only once a ledger of the new chain is proven under the UNL
//!   (spec/decisions/ledger-resets.md), never from what the server says
//!   of its validated ledger, which only tells the watcher to wait.
//!
//! - **Connecting:** check the network, and where the server's validated
//!   ledger stands against the newest proven ([`chain`]); subscribe to the
//!   escrow; read the escrow's history for hints from the last ledger
//!   attested, a window back ([`RESCAN_LEDGERS`]), where a restarted
//!   watcher's first walk ends too, since an attestation lost in a crash
//!   must go again and the hub refuses one sent twice; then follow the
//!   stream. A server that lies, refuses or stops answering is left for a
//!   new connection ([`Out::Reconnect`]), and what it hinted is forgotten:
//!   the next connection reads the history again, and the walk goes on.
//! - **What it submits:** the escrow's transactions a quorum signed, once
//!   it follows the escrow, and only while the issuer holds a token hub's
//!   escrow neither as of the hub's last final block nor as of a later
//!   state it proved, judged when it sends, not when it queues: one queued
//!   before a freeze waits for the thaw. A withdrawal's transaction is kept
//!   while the withdrawal stays open, and sent again every
//!   [`RESUBMIT_MS`], so one lost on the way, or never asked for while its
//!   requests were full, still goes.
//! - **Deposits until the hub recorded them.** A deposit's attestation
//!   holds the cursor until the hub's ledger recorded it
//!   ([`EscrowView::deposits_recorded`]), and is attested again while it
//!   has not, once the issuer lets go and the validator's share of the
//!   pending table has room: so one a full share refused during a freeze is
//!   lost neither to a restart, which reads it again, nor past the bound on
//!   what the hub's service keeps to attest again.
//!
//! - **The verifier's state across a restart.** Its UNL's lists and
//!   manifests, whether it is UNL-blocked, and its anchor go to its host
//!   with the cursor, after the hub's network and a digest of the
//!   publishers it trusts ([`Watcher::take_kept`]):
//!   once they changed, at most every [`KEEP_VERIFIER_EVERY_MS`], and at
//!   once after a reset, so a restart never meets the chain gone as a new
//!   reset. A restarted watcher takes them back before it connects
//!   ([`Watcher::restore`]), verified again at the restart's time, so a
//!   restart neither lifts a block nor takes an older anchor as current:
//!   the anchor taken back is current to nothing, and no hint is proven
//!   against it, as it may be of a chain that reset while the validator
//!   was down, until a newer one is proven; one off its chain is a reset on
//!   the testnet, and a fork, which stops the verifier, on any other
//!   network. A state of another network, or of other publishers, whose
//!   UNL may fork from the anchor kept and stop the verifier, or one the
//!   verifier does not read, is dropped, and the verifier starts afresh,
//!   its cursor's newest ledger still telling a server behind, or a chain
//!   gone back; so does
//!   one too large to keep, which drops what the host kept before
//!   ([`Kept::Dropped`]) rather than leave an older state to restart from.
//!
//! Its parts: `link`, what is the connection's and the network's, its
//! requests, its verifier, where its server stands, and what its host keeps
//! of the verifier; and `watch`, what is the escrow's. A [`Watcher`] is one
//! of each; a [`LinkWatcher`] (`shared`) is one link and the escrows of
//! every hub the node validates on its network.

use dassie_hub::Rules;
use dassie_hub::ledger::Withdrawal;
use dassie_hub::operation::{Issuer, ValidatorOperation};
use dassie_xrpl::AccountId;
use dassie_xrpl::account::resets;
use dassie_xrpl::api::{self, ApiError, Message, PAGE};
use dassie_xrpl::unl::{Anchor, ValidationSignatures};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::verifier::{ANCHOR_EVERY_MS, REQUEST_TIMEOUT_MS, STALL_MS, Verifier};
use serde_json::Value;

use crate::store::EscrowCursor;
use link::{Asked, Link, Parked};
pub use shared::{LinkOut, LinkWatcher};
use watch::EscrowWatcher;

mod link;
mod shared;
mod watch;

/// How far back from the last ledger attested a catch-up starts: what was
/// attested just before a crash may never have reached a block.
pub const RESCAN_LEDGERS: u32 = 1024;

/// How often the escrow is checked while no quorum has: the fallback. A
/// check also goes as soon as one of the escrow's own transactions is
/// proven to apply, and when a founding asks, once the anchor is newer
/// than the last check's ([`Watcher::check_escrow`]).
pub const ESCROW_CHECK_EVERY_MS: u64 = 30_000;

/// How often a token hub's issuer's hold on the escrow is proven: every
/// few ledgers.
pub const ISSUER_CHECK_EVERY_MS: u64 = 2 * ANCHOR_EVERY_MS;

/// How often a server behind, or on another network, is asked again where
/// it stands.
pub const RETRY_MS: u64 = 10_000;

/// How long after the newest anchor was proven the watcher still judges
/// what holds as of the newest ledger: the escrow's setup, the issuer's
/// hold. An honest server's network closes a ledger about every
/// [`ANCHOR_EVERY_MS`], and a round that fails takes at most
/// [`REQUEST_TIMEOUT_MS`] to give up on, or a stall of [`STALL_MS`]
/// before the next connection: so a validator on an honest server proves
/// a newer anchor within that, two closes allowed for the round itself.
/// Past it, its server stopped closing ledgers, or keeps the newer ones
/// from it, and nothing is judged until a newer anchor is proven.
pub const ANCHOR_FRESH_MS: u64 = REQUEST_TIMEOUT_MS + STALL_MS + 2 * ANCHOR_EVERY_MS;

/// How often at most the verifier's state goes to the host with the cursor
/// while it changes. What changes it is mostly its anchor, about every
/// [`ANCHOR_EVERY_MS`], with lists and manifests that are far larger:
/// written with each anchor, the lists' bytes would be rewritten every few
/// seconds. What a restart loses of a minute is anchors, which the next
/// round proves forward from the one kept, and lists and manifests, which
/// it fetches again; a list that lapsed since still blocks, as the state is
/// verified again at the restart's time.
pub const KEEP_VERIFIER_EVERY_MS: u64 = 60_000;

/// The most signed escrow transactions kept: waiting for a connection, for
/// the issuer to let go of the escrow, or for their withdrawal to settle.
pub const MAX_QUEUED_SUBMITS: usize = 64;

/// How long after it sent an escrow transaction the watcher sends it again
/// while its withdrawal stays open. A transaction the server took is in a
/// validated ledger within two closes, the open ledger's and the next, so
/// one whose withdrawal is still open then may have been lost on the way:
/// dropped by the server or the network, or never asked for while the
/// requests were full. One that applied meanwhile is refused for its
/// sequence, at no cost.
pub const RESUBMIT_MS: u64 = 2 * ANCHOR_EVERY_MS;

/// The most transactions hinted at and not yet proven: two pages of the
/// escrow's history. A page is asked for only while there is room for a
/// whole one; a hint the stream brings without room has the history read
/// again from its ledger, once there is.
pub const MAX_HINTS: usize = 2 * PAGE as usize;

/// What the validator's ledger says that bears on following the escrow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowView {
    /// The rules in force, for the check of the escrow.
    pub rules: Rules,
    /// Whether a quorum checked the escrow.
    pub checked: bool,
    /// The escrow's next sequence the hub gives out.
    pub next_sequence: u32,
    /// The withdrawals with an escrow sequence, not yet settled.
    pub withdrawals: Vec<(u32, Withdrawal)>,
    /// The sequence kept for the old validators' change of signer list.
    pub signer_list: Option<u32>,
    /// When the hub last wrote off a reset, by its block's timestamp.
    pub written_off_at: Option<u64>,
    /// What a quorum last attested of a token hub's issuer's hold.
    pub issuer: Issuer,
    /// How many more deposits this validator can attest now: its share of
    /// the pending table's room, none once the hub ended or while it is in
    /// no set in force.
    pub deposit_room: usize,
    /// The newest deposits, by transaction, whose attestation by this
    /// validator the hub's ledger recorded, or takes no more: credited,
    /// pending with it, or below the hub's floor. A bounded list: one that
    /// left it before the watcher saw it is attested again, and is in it
    /// again.
    pub deposits_recorded: Vec<[u8; 32]>,
}

/// Where a server's validated ledger stands against the newest seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chain {
    /// At or past it.
    Current,
    /// Below it, closed before it: a server behind.
    Behind,
    /// Below it, closed after it: a new chain, which a server behind cannot
    /// report. A reset, on the testnet.
    Restarted,
    /// The same, on a network that never resets.
    WentBack,
}

/// Where ledger `index`, closed at `close_time`, stands against `newest`
/// on `network` (spec/decisions/ledger-resets.md).
#[must_use]
pub fn chain(newest: Option<(u32, u64)>, network: u32, index: u32, close_time: u64) -> Chain {
    match newest {
        Some((newest, newest_close)) if index < newest => {
            if close_time <= newest_close {
                Chain::Behind
            } else if resets(network) {
                Chain::Restarted
            } else {
                Chain::WentBack
            }
        }
        _ => Chain::Current,
    }
}

/// What the watcher asks its host to do. Taken apart at once: boxing would
/// only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Send this on the WebSocket.
    Send(Vec<u8>),
    /// Fetch this validator list publisher's site by an HTTPS GET that
    /// follows no redirect and reads at most
    /// [`MAX_LIST_BYTES`](dassie_xrpl::validator_list::MAX_LIST_BYTES), and
    /// hand the body to [`Watcher::fetched`].
    Fetch(String),
    /// Drop the connection and connect anew: its server lied, refused or
    /// stopped answering, or the network reset.
    Reconnect,
    /// Have the validator attest this, and tell [`Watcher::taken`] once
    /// it took it, with the transaction it is about, if any: until then
    /// the watcher neither remembers the transaction nor moves its cursor
    /// past it.
    Attest(ValidatorOperation, Option<[u8; 32]>),
    /// Persist how far it followed the escrow, and with it, in the same
    /// write, the verifier's state [`Watcher::take_kept`] hands over, if
    /// it does.
    Cursor(EscrowCursor),
    /// Something the owner should read in the log.
    Problem(&'static str),
}

/// What a watcher hands its host to write of its verifier with a cursor
/// ([`Watcher::take_kept`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// Its state, after the hub's network: what [`Watcher::restore`] takes
    /// back.
    State(Vec<u8>),
    /// Nothing: what was kept before is dropped, as its state is too large
    /// to keep, and a restart starts afresh rather than from an older one.
    Dropped,
}

/// Why what a watcher kept of its verifier before a restart is not taken
/// back: the verifier starts afresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unrestored {
    /// It was kept on another network.
    Network(u32),
    /// It was kept under other publishers than the verifier trusts now:
    /// an anchor the old UNL proved may be one the new UNL forks from,
    /// which would stop the verifier on a network that never resets.
    Publishers,
    /// The verifier does not read it: of a version it does not read, cut
    /// short, or not what it writes.
    Unread,
}

/// Follows one hub's escrow: a `Link` of its own, and the escrow's
/// `EscrowWatcher` on it. It hands each input to the two in the order one
/// state machine took it before they were two, and once the input is
/// taken, what the escrow's drains parked: with one escrow, nothing that
/// is not its own.
#[derive(Debug)]
pub struct Watcher {
    link: Link,
    escrow: EscrowWatcher,
}

/// Building and reading it.
impl Watcher {
    /// A watcher of `escrow` on `network`, from where it left off, trusting
    /// the validators the publishers of `unl` list, whose validations
    /// `signatures` checks.
    #[must_use]
    pub fn new(
        escrow: AccountId,
        network: u32,
        cursor: Option<EscrowCursor>,
        unl: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
    ) -> Self {
        Self {
            link: Link::new(network, unl, signatures),
            escrow: EscrowWatcher::new(escrow, cursor),
        }
    }

    /// Takes back at `now_ms` what it handed its host of its verifier
    /// before a restart ([`Self::take_kept`]), before it first connects: the
    /// verifier's state, verified again at `now_ms`, whose validations
    /// `signatures` checks, as [`Self::new`]'s do. The anchor it holds is
    /// current to nothing, and no hint is proven against it: only a newer
    /// one, proven with fresh signatures, moves the cursor.
    ///
    /// # Errors
    ///
    /// Why it is not taken back, kept on another network, under other
    /// publishers, or unread: the verifier then starts afresh, as
    /// [`Self::new`] made it.
    pub fn restore(
        &mut self,
        kept: &[u8],
        signatures: Box<dyn ValidationSignatures + Send>,
        now_ms: u64,
    ) -> Result<(), Unrestored> {
        assert!(
            !self.link.is_connected() && !self.escrow.querying(),
            "taken back before it follows the escrow"
        );
        self.link.restore(kept, signatures, now_ms)?;
        // The threads it reads proven transactions for are no part of its
        // state.
        self.escrow.thread(&mut self.link);
        Ok(())
    }

    /// What to write of the verifier with the cursor of the [`Out::Cursor`]
    /// just taken, if anything: without it, what the host kept stays.
    pub const fn take_kept(&mut self) -> Option<Kept> {
        self.link.take_kept()
    }

    /// Whether it follows the escrow's stream now.
    #[must_use]
    pub fn following(&self) -> bool {
        self.escrow.following()
    }

    /// The network it follows the escrow on, which its host keeps the
    /// verifier's state by.
    #[must_use]
    pub const fn network(&self) -> u32 {
        self.link.network()
    }

    /// How far it followed the escrow.
    #[must_use]
    pub const fn cursor(&self) -> Option<EscrowCursor> {
        self.escrow.cursor()
    }

    /// The newest anchor its verifier proved, if any.
    #[must_use]
    pub fn anchor(&self) -> Option<Anchor> {
        self.escrow.anchor()
    }

    /// Whether the newest anchor is fresh enough to judge the newest
    /// ledger by ([`ANCHOR_FRESH_MS`]).
    #[must_use]
    pub fn fresh(&self) -> bool {
        self.escrow.fresh(self.link.now_ms())
    }

    /// Its verifier, to read what it proved and the lies it caught.
    #[must_use]
    pub const fn verifier(&self) -> &Verifier {
        self.link.verifier()
    }

    /// The newest state of the issuer's hold it proved, as of the ledger it
    /// was proven at, if any.
    #[must_use]
    pub const fn proven_issuer(&self) -> Option<Issuer> {
        self.escrow.proven_issuer()
    }

    /// The next thing to do.
    pub fn poll(&mut self) -> Option<Out> {
        self.link.poll()
    }

    /// Asserts what holds of it: of its link, of its escrow on the link,
    /// and that the escrow is subscribed to whenever the link's server was
    /// checked, as its one escrow subscribes at once.
    pub fn check_invariants(&self) {
        self.link.check_invariants();
        self.escrow.check_invariants(&self.link);
        assert_eq!(
            self.link.checked(),
            self.escrow.subscribed(),
            "its escrow is subscribed to once its server is checked"
        );
    }
}

/// Inputs.
impl Watcher {
    /// The validator took the attestation of `transaction`: it is
    /// remembered, and the cursor moves on once none is waiting before it.
    /// A deposit's waits on until the hub recorded it.
    pub fn taken(&mut self, transaction: Option<[u8; 32]>) {
        self.escrow.taken(&mut self.link, transaction);
    }

    /// The host dropped `operation`, the attestation of `transaction`,
    /// before the validator took it: it waits no more. A deposit's is
    /// attested again later, and holds the cursor until the hub recorded
    /// it; of any other transaction, no cursor this watcher persists passes
    /// its ledger, so a restart reads it again. The escrow's check and the
    /// issuer's hold are attested again once proven again: the validator
    /// signs a stopped hub's payout set by what it heard of the hold.
    pub fn dropped(&mut self, operation: &ValidatorOperation, transaction: Option<[u8; 32]>) {
        self.escrow.dropped(&mut self.link, operation, transaction);
    }

    /// A connection is up at `now_ms`: ask where the server stands, and
    /// have the verifier anchor on it.
    pub fn connected(&mut self, now_ms: u64) {
        self.link.now(now_ms);
        self.escrow.forget_connection(&mut self.link);
        self.link.connected();
        self.escrow.drain(&mut self.link);
        self.hand_on();
    }

    /// The connection is gone. What it decided for the host stays to be
    /// carried out, but for what was to be sent on the connection and a
    /// request to end it.
    pub fn disconnected(&mut self) {
        self.escrow.forget_connection(&mut self.link);
        self.link.disconnected();
    }

    /// Submits a signed escrow transaction once it follows the escrow and
    /// the issuer does not hold it, and again while its withdrawal stays
    /// open.
    pub fn submit(&mut self, blob: Vec<u8>) {
        self.escrow.submit(&mut self.link, blob);
    }

    /// Does what is due at `now`: ask a server that was behind again, have
    /// the verifier anchor while the anchor ages, check the escrow while no
    /// quorum has and the issuer's hold, ask again for proofs the server
    /// did not find, and look again at outcomes held.
    pub fn tick(&mut self, now_ms: u64, view: &EscrowView) {
        self.link.now(now_ms);
        self.see(view);
        self.link.tick(self.escrow.anchored_ms());
        self.escrow.tick(&mut self.link);
        self.escrow.drain(&mut self.link);
        self.escrow.work(&mut self.link);
        self.link.keep(false, self.escrow.persisted());
        self.hand_on();
    }

    /// Takes a message from the server.
    pub fn message(&mut self, now_ms: u64, bytes: &[u8], view: &EscrowView) {
        self.link.now(now_ms);
        self.see(view);
        match api::parse(bytes) {
            Message::Response { id, outcome } => {
                if let Some((asked, _, outcome)) = self.link.response(id, outcome) {
                    self.answer(asked, outcome);
                }
            }
            Message::Transaction(record) if self.escrow.subscribed() => {
                self.escrow.hinted(&record);
            }
            Message::LedgerClosed(closed) if self.escrow.subscribed() => {
                self.escrow.closed(closed.ledger_index);
                self.link.anchor_now();
            }
            Message::Transaction(_) | Message::LedgerClosed(_) | Message::Ignored => {}
        }
        self.escrow.drain(&mut self.link);
        self.escrow.work(&mut self.link);
        self.hand_on();
    }

    /// A founding asks whether a quorum checked the escrow, which its
    /// sponsor set up: the escrow is checked once the anchor is newer than
    /// the last check's, not at the next [`ESCROW_CHECK_EVERY_MS`]. However
    /// often it asks, that is at most once per anchor.
    pub fn check_escrow(&mut self, now_ms: u64, view: &EscrowView) {
        self.link.now(now_ms);
        self.see(view);
        self.escrow.want_check(view);
        self.escrow.work(&mut self.link);
        self.hand_on();
    }

    /// A fetch of a list publisher's `url` answered with `body`, or failed.
    pub fn fetched(&mut self, now_ms: u64, url: &str, body: Option<&[u8]>) {
        self.link.now(now_ms);
        let now_ms = self.link.now_ms();
        self.link.verifier_mut().fetched(url, body, now_ms);
        self.escrow.drain(&mut self.link);
        self.escrow.work(&mut self.link);
        self.hand_on();
    }
}

/// Answers, and what the escrow's drains parked.
impl Watcher {
    fn answer(&mut self, asked: Asked, outcome: Result<Value, ApiError>) {
        let link = &mut self.link;
        match (asked, outcome) {
            (Asked::ServerState, Ok(result)) => self.server_state(&result),
            (Asked::Subscribe, Ok(_)) => self.escrow.read_history(link),
            (Asked::History, Ok(result)) => self.escrow.history(link, &result),
            (Asked::Submit, Ok(result)) => {
                if api::submit_result(&result).is_none() {
                    link.push(Out::Problem(
                        "the XRPL server did not take an escrow transaction",
                    ));
                }
            }
            (Asked::ServerState | Asked::Subscribe | Asked::History, Err(_)) => {
                self.escrow.wait(link);
            }
            (Asked::Submit, Err(_)) => {}
        }
    }

    /// Where the server stands: on the hub's network, and on the chain the
    /// escrow's cursor follows, as far as it says. A server behind is
    /// waited for; a new chain is only a hint for the verifier to anchor,
    /// which proves it or not. Then the escrow is subscribed to.
    fn server_state(&mut self, result: &Value) {
        match self.link.server_state(result, self.escrow.newest()) {
            Some(validated) => self.escrow.catch_up(&mut self.link, validated),
            None => self.escrow.wait(&mut self.link),
        }
    }

    /// What the validator's ledger says now; the verifier threads the
    /// escrow's objects as they are.
    fn see(&mut self, view: &EscrowView) {
        if self.escrow.see(&mut self.link, view) {
            self.escrow.thread(&mut self.link);
        }
    }

    /// Hands on what the escrow's drains parked: a verdict on a query that
    /// was not its own then, which it drops as it did, and an anchor it
    /// took at once, for no other escrow.
    fn hand_on(&mut self) {
        while let Some(parked) = self.link.unpark() {
            match parked {
                Parked::Verdict(id, verdict) => self.escrow.verdict(&mut self.link, id, verdict),
                Parked::Anchored(..) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
