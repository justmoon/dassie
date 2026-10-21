//! The interface every settlement scheme implements.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use dassie_ledger::{LedgerId, Name};
use rand_core::CryptoRng;

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
    /// The underlying ledger holds more or less than the books say, for
    /// no reason the scheme can name, such as a change of reserve: the
    /// difference goes through `equity/suspense`, recorded whatever the
    /// owner's funds.
    Reconcile {
        /// How much, in internal units.
        amount: u64,
        /// Whether the ledger holds more than the books.
        gain: bool,
    },
    /// Send a message to the underlying ledger, over the scheme's
    /// connection ([`SettlementScheme::connection`]).
    LedgerSend {
        /// The message.
        message: Vec<u8>,
    },
    /// Connect to the underlying ledger anew: the connection stopped
    /// answering.
    Reconnect,
    /// Ask a faucet to fund the scheme's account on a test ledger, by an
    /// HTTPS POST of `body` to `url`. The answer comes as
    /// [`LedgerInput::Funded`].
    Fund {
        /// The faucet.
        url: String,
        /// What to post.
        body: Vec<u8>,
    },
}

/// What the node passes a scheme from its connection to the underlying
/// ledger, and from the faucet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerInput<'a> {
    /// The connection is up.
    Connected,
    /// A message came.
    Message(&'a [u8]),
    /// The connection went down; it is tried again.
    Disconnected,
    /// The faucet answered, or did not.
    Funded(bool),
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
/// Peering hooks and settlement are used from M4, when nodes peer; M3 runs
/// the stub's deposits.
pub trait SettlementScheme: Send {
    /// The scheme's ID, as advertised to peers, for example `stub`.
    fn id(&self) -> Name;

    /// The internal ledger the scheme's money is on.
    fn ledger(&self) -> LedgerId;

    /// The scheme protocol versions it speaks, negotiated when peering
    /// (ACC-14).
    fn supported_versions(&self) -> &'static [u16];

    /// The balance on the underlying ledger, in internal units: what the
    /// node checkpoints.
    fn balance(&self) -> u64;

    /// How the scheme stands with its underlying ledger.
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

    /// Where the node connects for the scheme's underlying ledger, if it
    /// has one: a WebSocket URL. Messages go both ways as
    /// [`Effect::LedgerSend`] and [`LedgerInput::Message`].
    fn connection(&self) -> Option<&str> {
        None
    }

    /// Where the owner sends money to fund the scheme, if anywhere: an
    /// address on the underlying ledger.
    fn deposit_address(&self) -> Option<String> {
        None
    }

    /// The least amount the underlying ledger can move, in internal units:
    /// settlements are whole multiples of it. Less than it is owed until
    /// there is more, or for good at the end of a peering.
    fn unit(&self) -> u64 {
        1
    }

    /// What the underlying ledger holds back from the scheme's balance, in
    /// internal units: never spendable, and never booked.
    fn reserve(&self) -> u64 {
        0
    }

    /// What the scheme must keep across restarts beyond its balance, such
    /// as transactions in flight. The node persists it with every batch
    /// that changes the books for the scheme, and hands it back when it
    /// makes the scheme again.
    fn state(&self) -> Vec<u8> {
        Vec::new()
    }

    /// A number that changes whenever [`state`](Self::state) does: the
    /// node writes the state with its next batch only then.
    fn state_version(&self) -> u64 {
        0
    }

    /// Takes in what came from the underlying ledger, or the faucet.
    fn on_ledger(&mut self, now_ms: u64, input: LedgerInput<'_>, effects: &mut Vec<Effect>) {
        let _ = (now_ms, input, effects);
    }

    /// When [`on_timer`](Self::on_timer) is due next, if ever.
    fn deadline(&self) -> Option<u64> {
        None
    }

    /// Runs the scheme's timers that are due.
    fn on_timer(&mut self, now_ms: u64, effects: &mut Vec<Effect>) {
        let _ = (now_ms, effects);
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

    /// Prepares a settlement of `amount` to `peer`. Submits nothing.
    ///
    /// # Errors
    ///
    /// If the scheme cannot settle now.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError>;

    /// Forgets a settlement prepared and never executed: the node could
    /// not book it. It is never sent.
    fn discard_settlement(&mut self, settlement: Name) {
        let _ = settlement;
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
    fn execute_settlement(&mut self, settlement: Name, effects: &mut Vec<Effect>);

    /// Settles the outcome of settlements whose pending transfers survived
    /// a restart, given by ID and amount, at activation (ACC-16): each gets
    /// [`Effect::Finalize`] or [`Effect::Cancel`], now or later.
    fn reconcile(&mut self, pending: &[(Name, u64)], effects: &mut Vec<Effect>);

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

    /// Whether the scheme can ask a faucet for test money on its
    /// underlying ledger.
    fn has_faucet(&self) -> bool {
        false
    }

    /// Asks the faucet of a test ledger for money on the owner's behalf, as
    /// [`Effect::Fund`]. Nothing is booked now: what the faucet sends is a
    /// deposit once the ledger shows it.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unsupported`] where the scheme has no faucet;
    /// [`SchemeError::Unavailable`] while the faucet was asked too recently.
    fn ask_faucet(&mut self, effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        let _ = effects;
        Err(SchemeError::Unsupported)
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
