//! The interface every settlement scheme implements.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use dassie_ledger::{LedgerId, Name};
use rand_core::CryptoRng;

use crate::asset::{AssetId, ExternalAsset};
use crate::commitments::Commitments;

/// Something the node must book or do because of a scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A peer settled with us: Dr `assets/settlement`, Cr
    /// `assets/interledger/<peer>`, posted.
    IncomingSettlement {
        /// The peer.
        peer: Name,
        /// How much, in internal units.
        amount: u64,
        /// The settlement's ID on the underlying ledger, where it has one,
        /// such as a transaction hash: the journal books it once.
        id: Option<Name>,
    },
    /// Our outgoing settlement executed: post its pending transfer.
    Finalize {
        /// The settlement.
        settlement: Name,
    },
    /// Our outgoing settlement failed for good: void its pending transfer.
    /// The original never issued this, so failures held their amount until
    /// a restart (ACC-16).
    Cancel {
        /// The settlement.
        settlement: Name,
    },
    /// The owner added money: Dr `assets/settlement`, Cr `equity/owner`.
    Deposit {
        /// How much.
        amount: u64,
    },
    /// The owner took money out: Dr `equity/owner`, Cr `assets/settlement`.
    Withdrawal {
        /// How much.
        amount: u64,
    },
    /// Send a scheme message to a peer.
    SendMessage {
        /// The peer.
        peer: Name,
        /// The scheme's payload.
        message: Vec<u8>,
    },
    /// The underlying ledger charged a fee for one of our transactions:
    /// Dr `expenses/fees`, Cr `assets/settlement`, recorded whatever the
    /// owner's funds, since the money is gone.
    Fee {
        /// How much, in internal units.
        amount: u64,
        /// The transaction.
        id: Name,
    },
    /// The underlying ledger holds more or less than the books say, for a
    /// reason they have no entry for, such as a change of reserve: the
    /// difference goes through `equity/suspense`, recorded whatever the
    /// owner's funds. Its cause changes only how it is reported.
    Reconcile {
        /// How much, in internal units.
        amount: u64,
        /// Whether the ledger holds more than the books.
        gain: bool,
        /// What the scheme takes the difference to come from.
        cause: ReconcileCause,
    },
    /// A peering over a settlement hub cost us money its hold did not
    /// cover, for one of the reasons the design lists
    /// (spec/designs/settlement-hubs.md#13-how-it-fits-the-node): Dr
    /// `expenses/losses`, Cr the account the money left as its cause says,
    /// recorded whatever the owner's funds, since the money is gone. The
    /// node counts it against the peer, and the simulation measures it.
    PeerLoss {
        /// The peer.
        peer: Name,
        /// How much, in internal units.
        amount: u64,
        /// Why, which says what account the money left.
        cause: LossCause,
        /// What on the underlying ledger it comes from, such as the
        /// operation that took it: the journal books it once.
        id: Name,
    },
}

/// Why a peering over a settlement hub cost us money its hold did not
/// cover (spec/designs/settlement-hubs.md#13-how-it-fits-the-node).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossCause {
    /// The hub's fee for a claim we submitted because the peer's claim was
    /// missing or late: it left `assets/settlement`.
    ClaimFee,
    /// The hub paid the peer, with its claim, for a packet we had not
    /// counted as fulfilled when we learnt of it: we had rejected it
    /// upstream, or forgotten it. It left `assets/interledger/<peer>`.
    PaidLate,
    /// The hub paid the peer, with its claim, for a packet the peer had
    /// rejected, which an honest payee never claims: evidence against the
    /// peering. It left `assets/interledger/<peer>`.
    Cheated,
}

/// What a scheme takes a reconciliation to come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileCause {
    /// The ledger's reserve changed: what it holds back per account or per
    /// object, or how many objects the account owns, such as a trust line
    /// opened. Expected: the books follow what is above the reserve.
    Reserve,
    /// Anything else: a difference the scheme cannot explain, books short
    /// of what a transaction took, or an account written off.
    Unexplained,
}

/// How a scheme stands with its underlying ledger, for the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerStatus {
    /// No underlying ledger to follow, or it is followed and up to date.
    Synced,
    /// Connecting to it.
    Connecting,
    /// Connected, catching up with what happened meanwhile.
    Syncing,
    /// Its account does not exist yet: it waits to be funded.
    Unfunded,
    /// Something is wrong that the scheme cannot mend.
    Failed(&'static str),
}

/// An outgoing settlement, prepared but not yet executed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSettlement {
    /// Unique within the scheme. The node keys the pending transfer with
    /// it, durably.
    pub id: Name,
    /// How much it settles: at most what was asked, less what the
    /// underlying ledger cannot represent. The node books this.
    pub amount: u64,
    /// The proof or claim to send the peer with the settlement message.
    pub message: Vec<u8>,
    /// The peer's new scheme state, if it changed. The node keeps it; the
    /// original dropped it (ACC-15).
    pub peer_state: Option<Vec<u8>>,
}

/// Why a scheme refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemeError {
    /// No scheme has that ID.
    UnknownScheme,
    /// The scheme does not accept that config.
    InvalidConfig,
    /// The scheme does not do that, for example deposits from the node.
    Unsupported,
    /// A peer's peering or settlement data is invalid.
    InvalidData,
    /// The scheme has too many settlements prepared and not executed.
    TooManyPrepared,
    /// A balance would overflow.
    Overflow,
    /// Not now: the scheme cannot reach or does not yet see its ledger,
    /// has no funds to spare, or as much in flight as it takes. Asked again
    /// later, as if nothing happened.
    Unavailable,
}

impl fmt::Display for SchemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownScheme => "unknown settlement scheme",
            Self::InvalidConfig => "invalid settlement scheme config",
            Self::Unsupported => "the settlement scheme does not support this",
            Self::InvalidData => "invalid settlement data",
            Self::TooManyPrepared => "too many settlements in preparation",
            Self::Overflow => "the balance would overflow",
            Self::Unavailable => "the settlement scheme cannot settle now",
        })
    }
}

/// A settlement scheme (spec/areas/settlement.md#the-scheme-interface),
/// sans-IO.
///
/// A scheme that settles through an [`ExternalAsset`] names it
/// ([`asset`](Self::asset)), and the node hands it that asset whenever it
/// may sign, submit or look up through it, and after the asset heard from
/// its ledger ([`on_asset`](Self::on_asset)). It reaches its own kind of
/// asset by downcasting. A scheme without one gets `None`.
pub trait SettlementScheme: Send {
    /// The scheme's ID, as advertised to peers, for example `stub`.
    fn id(&self) -> Name;

    /// The internal ledger the scheme's money is on.
    fn ledger(&self) -> LedgerId;

    /// The external asset it settles through, if any. It never changes.
    fn asset(&self) -> Option<AssetId> {
        None
    }

    /// Who issues what it settles in, for a token: the issuer's address on
    /// the underlying ledger, which what the owner deposits must name.
    /// `None` for the ledger's own asset. It never changes.
    fn issuer(&self) -> Option<String> {
        None
    }

    /// The scheme protocol versions it speaks, negotiated when peering
    /// (ACC-14).
    fn supported_versions(&self) -> &'static [u16];

    /// The balance on the underlying ledger, in internal units: what the
    /// node checkpoints.
    fn balance(&self) -> u64;

    /// How the scheme stands, once its asset is synced: the node shows the
    /// asset's status until then.
    fn status(&self) -> LedgerStatus {
        LedgerStatus::Synced
    }

    /// Whether the scheme has seen its ledger since it last started from
    /// nothing: what its account held then is booked as the owner's, and
    /// what arrives from now on is told apart, a peer's settlement from a
    /// deposit. It is kept with the scheme's state, so it holds across
    /// restarts whether or not the ledger can be reached. The node peers on
    /// the scheme only once it has (spec/decisions/synced-before-peering.md).
    /// A scheme without a ledger to follow has seen it from the start.
    fn has_synced(&self) -> bool {
        true
    }

    /// Whether a peer's settlement can reach us on the underlying ledger
    /// now. While it cannot, as while an issuer has frozen our line deep,
    /// frozen its token globally or charges a transfer fee no peer pays, the
    /// node asks no peer on the scheme to
    /// settle, counts a peer's time short of room toward withdrawing it but
    /// never toward ending the peering, and leaves the scheme out of its
    /// link state (spec/areas/settlement.md#when-peers-cannot-pay-us). The
    /// scheme stays synced meanwhile: it still sees its ledger.
    fn can_receive(&self) -> bool {
        true
    }

    /// The node's settlement check: what the scheme reads of its ledger at
    /// each one, through its asset, such as an issuer's account. What it
    /// learns comes as news ([`on_asset`](Self::on_asset)).
    fn on_check(&mut self, now_ms: u64, asset: &mut dyn ExternalAsset) {
        let _ = (now_ms, asset);
    }

    /// The least amount the underlying ledger can move, in internal units:
    /// settlements are whole multiples of it. Less than it is owed until
    /// there is more, or for good at the end of a peering.
    fn unit(&self) -> u64 {
        1
    }

    /// What the underlying ledger holds back of what the scheme settles
    /// in, in internal units: never spendable, and never booked.
    fn reserve(&self) -> u64 {
        0
    }

    /// What the scheme must keep across restarts beyond its balance, such
    /// as where it is in its asset's transactions. The node persists it with
    /// every batch that changes the books for the scheme, and hands it back
    /// when it makes the scheme again.
    fn state(&self) -> Vec<u8> {
        Vec::new()
    }

    /// A number that changes whenever [`state`](Self::state) does: the
    /// node writes the state with its next batch only then.
    fn state_version(&self) -> u64 {
        0
    }

    /// Takes in what its external asset learnt from its ledger, and tells
    /// the asset what it needs of it. The node calls it once the scheme is
    /// made, and after each time the asset heard from its ledger, its
    /// timers or its faucet, before the asset forgets what it learnt.
    fn on_asset(&mut self, now_ms: u64, asset: &mut dyn ExternalAsset, effects: &mut Vec<Effect>) {
        let _ = (now_ms, asset, effects);
    }

    /// What a node that wants to peer with us needs to know.
    fn peering_info(&self, out: &mut Vec<u8>);

    /// Builds our peering request to `peer`, given its peering info. Keeps
    /// nothing of `peer`: the attempt may still fail.
    ///
    /// # Errors
    ///
    /// If the info is invalid.
    fn create_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError>;

    /// Accepts or refuses `peer`'s request, writing the response data.
    /// Returns the peer's scheme state, and keeps nothing of `peer` until
    /// [`on_peered`](Self::on_peered).
    ///
    /// # Errors
    ///
    /// If the request is refused.
    fn accept_peering_request(
        &mut self,
        peer: Name,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError>;

    /// Completes our request after `peer` accepted it. Returns the peer's
    /// scheme state, and keeps nothing of `peer` until
    /// [`on_peered`](Self::on_peered).
    ///
    /// # Errors
    ///
    /// If the response is invalid.
    fn finalize_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        data: &[u8],
    ) -> Result<Vec<u8>, SchemeError>;

    /// The peering with `peer` is made: its accounts are open, and `state`
    /// is what [`accept_peering_request`](Self::accept_peering_request) or
    /// [`finalize_peering_request`](Self::finalize_peering_request)
    /// returned. What the scheme keeps per peer, it keeps from here, so an
    /// attempt that fails takes none of its room.
    fn on_peered(&mut self, peer: Name, state: &[u8]) {
        let _ = (peer, state);
    }

    /// The settlement channel with `peer`, which no longer peers with us,
    /// closed, with nothing of it pending and the scheme seeing its ledger
    /// (spec/decisions/closing-channels.md): what the scheme kept of it for
    /// its settlements may go. A later peering tells it again
    /// ([`on_peered`](Self::on_peered)). Its accounts stay.
    fn on_closed(&mut self, peer: Name) {
        let _ = peer;
    }

    /// Prepares a settlement of `amount` to `peer`, signing through
    /// `asset` where it settles through one. Submits nothing.
    ///
    /// # Errors
    ///
    /// If the scheme cannot settle now.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        asset: Option<&mut dyn ExternalAsset>,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError>;

    /// Forgets a settlement prepared and never executed: the node could
    /// not book it. It is never sent.
    fn discard_settlement(&mut self, asset: Option<&mut dyn ExternalAsset>, settlement: Name) {
        let _ = (asset, settlement);
    }

    /// The message for the peer for a settlement of ours, made again from
    /// its ID, after a restart lost the one [`PreparedSettlement`] gave.
    fn settlement_message(&self, settlement: Name) -> Vec<u8> {
        let _ = settlement;
        Vec::new()
    }

    /// Executes a prepared settlement. Reports the outcome as
    /// [`Effect::Finalize`] or [`Effect::Cancel`], now or later. The node
    /// calls it only once the settlement's pending transfer is on disk.
    fn execute_settlement(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        settlement: Name,
        effects: &mut Vec<Effect>,
    );

    /// Settles the outcome of settlements whose pending transfers survived
    /// a restart, given by ID and amount, at activation (ACC-16): each gets
    /// [`Effect::Finalize`] or [`Effect::Cancel`], now or later.
    fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    );

    /// Handles a peer's settlement message.
    ///
    /// # Errors
    ///
    /// If the data is invalid.
    fn handle_settlement(
        &mut self,
        peer: Name,
        amount: u64,
        data: &[u8],
        peer_state: &[u8],
        effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError>;

    /// Handles a scheme message from a peer.
    fn handle_message(&mut self, peer: Name, message: &[u8], effects: &mut Vec<Effect>);

    /// Adds money on the owner's behalf, where the scheme can.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unsupported`] where the owner deposits on the
    /// underlying ledger itself.
    fn deposit(&mut self, amount: u64, effects: &mut Vec<Effect>) -> Result<(), SchemeError>;

    /// Its side that signs and takes promises per packet, if its peerings
    /// carry them, as a settlement hub's do
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). The node
    /// then sends a commitment with each Prepare to a peer on it, checks
    /// one with each Prepare from it, and carries a claim in each reply to
    /// a Fulfill.
    fn commitments(&mut self) -> Option<&mut dyn Commitments> {
        None
    }

    /// Asserts what holds between calls: its bounds, and how its parts
    /// relate (spec/decisions/tigerstyle.md). Tests, simulations and the
    /// node's checks run it.
    ///
    /// # Panics
    ///
    /// If the scheme's state is inconsistent.
    fn check_invariants(&self);
}

/// A scheme the node can make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemeInfo {
    /// Its ID.
    pub id: &'static str,
    /// Its name for people.
    pub name: &'static str,
    /// The ledger it serves.
    pub ledger: LedgerId,
}
