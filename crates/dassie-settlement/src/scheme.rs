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
    /// a restart.
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
    /// The owner's payment `id` out of the scheme's account
    /// ([`SettlementScheme::pay_owner`]) is no longer on its way: the node
    /// posts the pending transfer it held the owner's funds with (Dr
    /// `equity/owner`, Cr `assets/settlement`) if it applied, and voids it
    /// if it moved nothing, or if the scheme cannot tell, in which case its
    /// ledger's balance said what it did.
    OwnerPaid {
        /// The payment.
        id: Name,
        /// Whether it applied.
        applied: bool,
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
    /// The hub's fee for a dispute we submitted because the peer claimed a
    /// packet it had released with its Reject: evidence against the
    /// peering, as [`Cheated`](Self::Cheated) is, though the dispute kept
    /// the packet from being paid (spec/decisions/signed-rejects.md). It
    /// left `assets/settlement`.
    DisputeFee,
}

impl LossCause {
    /// Whether it is evidence that the peer cheated: the node ends the
    /// peering.
    #[must_use]
    pub const fn cheated(self) -> bool {
        matches!(self, Self::Cheated | Self::DisputeFee)
    }

    /// Whether it is a fee we paid the hub, which left `assets/settlement`,
    /// rather than a packet paid, which left `assets/interledger/<peer>`.
    #[must_use]
    pub const fn is_fee(self) -> bool {
        matches!(self, Self::ClaimFee | Self::DisputeFee)
    }
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
}

/// The most bytes of the memo an [`OwnerPayment`] carries: of its type,
/// and of its data.
pub const MAX_OWNER_MEMO_BYTES: usize = 64;

/// The memo an [`OwnerPayment`] carries, on a ledger that has memos: its
/// type, which says who reads it and how, and its data. Whoever asks for
/// the payment names the type: the reader on the other side, such as a
/// settlement hub's validators reading the key of the account a deposit
/// opens, is theirs, not the scheme's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerMemo {
    /// Its type, at most [`MAX_OWNER_MEMO_BYTES`].
    pub memo_type: Vec<u8>,
    /// Its data, at most [`MAX_OWNER_MEMO_BYTES`].
    pub data: Vec<u8>,
}

impl OwnerMemo {
    /// Whether its type and data are within [`MAX_OWNER_MEMO_BYTES`].
    #[must_use]
    pub const fn fits(&self) -> bool {
        self.memo_type.len() <= MAX_OWNER_MEMO_BYTES && self.data.len() <= MAX_OWNER_MEMO_BYTES
    }
}

/// A payment of the owner's funds out of a scheme's account, to an address
/// its ledger pays: how the node moves the owner's funds between two of
/// its ledgers, such as into a settlement hub and out of it
/// (spec/areas/hubs.md#in-the-node). The scheme books it as the owner's
/// withdrawal once its ledger shows it applied, with its fee; the ledger
/// it arrives on books it as a deposit when it sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerPayment {
    /// Where, as the ledger writes an address.
    pub destination: String,
    /// The destination tag, on a ledger that has them.
    pub tag: Option<u32>,
    /// One memo, of the type its reader expects.
    pub memo: Option<OwnerMemo>,
    /// How much, in internal units: whole units of the ledger.
    pub amount: u64,
}

/// What became of an owner's payment out of a scheme's account
/// ([`SettlementScheme::pay_owner`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OwnerPaymentState {
    /// The scheme does not know: not its last payment, or one whose outcome
    /// it never read, such as one its ledger settled while the scheme was
    /// stopped. Only what it reached on the other side can tell.
    #[default]
    Unknown,
    /// It is on its way: no other is made meanwhile.
    InFlight,
    /// Its ledger applied it.
    Applied,
    /// Its ledger recorded it as failed: it moved nothing but its fee. The
    /// same payment again fails alike, at a fee each time, unless what made
    /// it fail changed.
    Failed(PaymentFailure),
    /// It never reached its ledger, which can never apply it now: it moved
    /// nothing, not even a fee, and nothing stands against paying again.
    Cancelled,
}

/// Why an owner's payment its ledger recorded failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaymentFailure {
    /// Its destination does not exist, and it was too little to create it.
    TooLittle,
    /// Anything else.
    Other,
}

/// A currency's exchange rate against the dollar, as the node has it: how
/// many of its units one dollar buys, times 10^`precision`, and when the
/// node learnt it (spec/decisions/exchange-rates.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DollarRate {
    /// Units of the currency per dollar, times 10^`precision`.
    pub per_dollar: u128,
    /// The decimals of `per_dollar`.
    pub precision: u32,
    /// When the node learnt it.
    pub as_of_ms: u64,
}

/// What a dollar token's peg guard says, for the owner
/// (spec/areas/xrpl.md#the-peg-guard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PegView {
    /// Why the scheme is paused, if it is.
    pub paused: Option<PegPause>,
    /// The token's price in millionths of a dollar at the last reading
    /// that counted, and the validated ledger it is as of.
    pub last: Option<(u64, u32)>,
}

/// Why a dollar token's peg guard pauses its scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PegPause {
    /// Its pool prices it off its peg, and has for long enough.
    OffPeg,
    /// No reading has counted for too long: the pool unread or too thin,
    /// or the rate stale. The guard fails closed.
    Stale,
}

impl PegPause {
    /// The reason, for people.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::OffPeg => "the token is off its peg",
            Self::Stale => "the token's price could not be read",
        }
    }
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

    /// Whether the node peers on the scheme. One that does not is an owner
    /// book: it keeps what the node holds on its ledger, takes the owner's
    /// deposits and withdrawals and pays the owner's payments, such as a
    /// settlement hub's funding, and peers with nobody. The node leaves it
    /// out of its link state, picks no candidate on it, and refuses
    /// requests on it as for a scheme it does not run
    /// (spec/areas/settlement.md#owner-books). It never changes while the
    /// scheme runs.
    ///
    /// The peer half of the interface (the peering steps, preparing and
    /// executing a settlement) answers by default as an owner book does:
    /// no peering info, and every peering step and settlement refused. A
    /// scheme that peers overrides each of them.
    fn peers(&self) -> bool {
        true
    }

    /// Whether a peer's settlement can reach us on the underlying ledger
    /// now. While it cannot, as once a settlement hub is shutting down,
    /// stalled or ended, the node asks no peer on the scheme to settle,
    /// counts a peer's time short of room toward withdrawing it but never
    /// toward ending the peering, leaves the scheme out of its link state,
    /// and drops its methods once it could not receive for as long as a
    /// silent peer is kept (spec/areas/settlement.md#when-peers-cannot-pay-us).
    /// Only a scheme that peers is asked.
    fn can_receive(&self) -> bool {
        true
    }

    /// Whether the connector may convert to or from the scheme's ledger
    /// now. While it may not, as while a dollar token is off its peg, the
    /// node prices no pair with the ledger, as for a ledger whose rate is
    /// stale (spec/areas/connector.md#inventory-pricing); transit on the
    /// ledger alone still passes.
    fn converts(&self) -> bool {
        true
    }

    /// What its peg guard says, for a dollar token that has one.
    fn peg(&self) -> Option<PegView> {
        None
    }

    /// The currencies whose rates against the dollar the scheme reads,
    /// beside its own ledger's, such as XRP for a dollar token priced by a
    /// pool of XRP: the node fetches them, and hands each over before each
    /// settlement check ([`on_rate`](Self::on_rate)).
    fn rate_currencies(&self) -> &'static [&'static str] {
        &[]
    }

    /// The rate of `code`, one of [`rate_currencies`](Self::rate_currencies),
    /// as the node has it now: `None` if it has none.
    fn on_rate(&mut self, code: &str, rate: Option<DollarRate>) {
        let _ = (code, rate);
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

    /// What a node that wants to peer with us needs to know. None by
    /// default, as an owner book gives.
    fn peering_info(&self, out: &mut Vec<u8>) {
        out.clear();
    }

    /// Builds our peering request to `peer`, given its peering info. Keeps
    /// nothing of `peer`: the attempt may still fail.
    ///
    /// # Errors
    ///
    /// If the info is invalid; [`SchemeError::Unsupported`] by default, as
    /// an owner book peers with no one.
    fn create_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        let _ = (peer, peering_info, out);
        Err(SchemeError::Unsupported)
    }

    /// Accepts or refuses `peer`'s request. Returns the peer's scheme
    /// state, and keeps nothing of `peer` until
    /// [`on_peered`](Self::on_peered). The acceptance carries nothing: what
    /// the requester needs of us, it has from our peering info.
    ///
    /// # Errors
    ///
    /// If the request is refused; [`SchemeError::Unsupported`] by default,
    /// as an owner book peers with no one.
    fn accept_peering_request(&mut self, peer: Name, data: &[u8]) -> Result<Vec<u8>, SchemeError> {
        let _ = (peer, data);
        Err(SchemeError::Unsupported)
    }

    /// Completes our request after `peer` accepted it, from the peering
    /// info it gave us. Returns the peer's scheme state, and keeps nothing
    /// of `peer` until [`on_peered`](Self::on_peered).
    ///
    /// # Errors
    ///
    /// If the peering info is invalid; [`SchemeError::Unsupported`] by
    /// default, as an owner book peers with no one.
    fn finalize_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        let _ = (peer, peering_info);
        Err(SchemeError::Unsupported)
    }

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

    /// Whether the scheme still needs the settlement channel with `peer`,
    /// which no longer peers with us: something between the two is not
    /// settled on its ledger yet, as a hold with funds or a claim not yet
    /// taken over a settlement hub. The channel stays open meanwhile
    /// (spec/decisions/closing-channels.md).
    fn keeps_channel(&self, peer: Name) -> bool {
        let _ = peer;
        false
    }

    /// Prepares a settlement of `amount` to `peer`, signing through
    /// `asset` where it settles through one. Submits nothing.
    ///
    /// # Errors
    ///
    /// If the scheme cannot settle now; [`SchemeError::Unsupported`] by
    /// default, as an owner book settles with no one.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        asset: Option<&mut dyn ExternalAsset>,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        let _ = (rng, asset, peer, amount, peer_state);
        Err(SchemeError::Unsupported)
    }

    /// The most a settlement to `peer` could move now, in the ledger's
    /// units, as [`prepare_settlement`](Self::prepare_settlement) would
    /// clamp it: so the node asks for none it cannot fund, and knows when
    /// one it could not fund can be (spec/decisions/settle-when-funds-arrive.md).
    /// `None` if the scheme cannot tell, as by default.
    fn spendable(&self, asset: Option<&dyn ExternalAsset>, peer: Name) -> Option<u64> {
        let _ = (asset, peer);
        None
    }

    /// Forgets a settlement prepared and never executed: the node could
    /// not book it. It is never sent.
    fn discard_settlement(&mut self, asset: Option<&mut dyn ExternalAsset>, settlement: Name) {
        let _ = (asset, settlement);
    }

    /// Executes a prepared settlement. Reports the outcome as
    /// [`Effect::Finalize`] or [`Effect::Cancel`], now or later. The node
    /// calls it only once the settlement's pending transfer is on disk. By
    /// default, as an owner book, which prepares none, it cancels it: one
    /// the node asks for anyway can never apply.
    fn execute_settlement(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        settlement: Name,
        effects: &mut Vec<Effect>,
    ) {
        let _ = asset;
        effects.push(Effect::Cancel { settlement });
    }

    /// Settles the outcome of settlements whose pending transfers survived
    /// a restart, given by ID and amount, at activation: each gets
    /// [`Effect::Finalize`] or [`Effect::Cancel`], now or later.
    fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    );

    /// Whether a `settlement` message on this scheme books what it states.
    /// Only the stub's does, which only a build with debug assertions runs
    /// (spec/decisions/stub-in-debug-builds.md). A scheme whose ledger says what arrived, as a
    /// hub's, sends a bare notice instead, which only moves the settlement
    /// channel on, with no data, and takes a peer's whatever it states or
    /// carries: the node neither checks nor books the amount a peer's
    /// states, so the amount the node states in its own only says what is
    /// on its way. The
    /// node posts such a settlement as the scheme finalizes it, and only
    /// its notice waits for the channel; one whose message books goes
    /// with its message, in one batch.
    fn books_messages(&self) -> bool {
        true
    }

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

    /// Adds money on the owner's behalf, where the scheme can.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unsupported`] where the owner deposits on the
    /// underlying ledger itself.
    fn deposit(&mut self, amount: u64, effects: &mut Vec<Effect>) -> Result<(), SchemeError>;

    /// Pays `payment` out of the owner's funds, through the scheme's asset:
    /// one at a time. Returns the payment's ID, which
    /// [`owner_payment`](Self::owner_payment) answers for, also after a
    /// restart. The node holds the payment against the owner's funds as a
    /// pending transfer under that ID, booked in the batch that keeps the
    /// payment, so the books refuse it if the owner's funds do not cover it.
    /// The scheme reports once the payment is no longer on its way
    /// ([`Effect::OwnerPaid`]), which posts or voids the hold, and books its
    /// fee either way.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unsupported`] by default; [`SchemeError::InvalidData`]
    /// for a destination the ledger cannot pay, a memo too long or an
    /// amount below the ledger's unit; [`SchemeError::Unavailable`] while
    /// one is on its way, or the asset cannot sign it now.
    fn pay_owner(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        payment: &OwnerPayment,
    ) -> Result<Name, SchemeError> {
        let _ = (asset, payment);
        Err(SchemeError::Unsupported)
    }

    /// The most an owner's payment made now could take of the asset with
    /// its fees, in the ledger's units, as [`pay_owner`](Self::pay_owner)
    /// checks it: what the asset can spend once what is ahead of the
    /// payment has taken its share, so the node sizes none the scheme
    /// would refuse (spec/areas/hubs.md#in-the-node). `None` if the scheme
    /// cannot tell, as by default.
    fn owner_spendable(&self, asset: Option<&dyn ExternalAsset>) -> Option<u64> {
        let _ = asset;
        None
    }

    /// What became of the owner's payment `id`: the scheme keeps the one
    /// on its way and the outcome of the last, with its state.
    fn owner_payment(&self, id: Name) -> OwnerPaymentState {
        let _ = id;
        OwnerPaymentState::Unknown
    }

    /// Its side that signs and takes promises per packet, if its peerings
    /// carry them, as a settlement hub's do
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). The node
    /// then sends a commitment with each Prepare to a peer on it, checks
    /// one with each Prepare from it, and carries a claim in each reply to
    /// a Fulfill.
    fn commitments(&mut self) -> Option<&mut dyn Commitments> {
        None
    }

    /// Over a settlement hub: `peer`'s account number in the hub, as the
    /// scheme knows it, which may be one the hub opened for the peer's key
    /// after the peering was made, as a peer paid into the hub has
    /// (spec/designs/settlement-hubs.md#5-joining). None for a peer without
    /// one, and by default.
    fn peer_hub_account(&self, peer: Name) -> Option<u32> {
        let _ = peer;
        None
    }

    /// Over a settlement hub: whether a packet over it is not settled yet,
    /// by the promises it carried: one of ours in flight or waiting for the
    /// hub to decide it, or one of a peer's in flight through us or
    /// fulfilled with no claim of the peer's covering it, which the scheme
    /// claims on the hub itself once the peer's claim is late
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). False by
    /// default.
    fn packets_unsettled(&self) -> bool {
        false
    }

    /// Over a settlement hub: `peer`, which peers with us on the scheme,
    /// names `account` in the hub `hub` in its link state, which it did not
    /// before. Only the hub says which account a key has: a scheme that
    /// has none for the peer asks the hub at once rather than at its next
    /// lookup (spec/areas/hubs.md#the-members-side). Nothing by default.
    fn on_peer_advertised(&mut self, peer: Name, hub: [u8; 32], account: u32) {
        let _ = (peer, hub, account);
    }

    /// Over a settlement hub: what deposits had credited the node's own
    /// account in the hub as of the last state its books took, in the hub
    /// asset's smallest unit, the hub's `deposited` counter as the books
    /// booked it. It is kept with the books, and grows only as they book a
    /// deposit, so a deposit whose counter it has not reached is not on
    /// the scheme's ledger. None before the books took a state of the
    /// account, and by default.
    fn deposits_booked(&self) -> Option<u64> {
        None
    }

    /// Whether peers on the scheme trust each other without limit, as the
    /// stub with unlimited trust's do, for tests whose subject is not
    /// settlement (spec/areas/settlement.md#the-stub-scheme). The node then
    /// extends every peer on it
    /// [`UNLIMITED_CREDIT`](crate::policy::UNLIMITED_CREDIT), never asks
    /// one to settle on its own account, and settles nothing when asked, as
    /// at a peering's end. It never changes.
    fn unlimited_trust(&self) -> bool {
        false
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
