//! What a transfer is for.

use crate::name::Name;

/// Identifies a packet across restarts: the node's boot epoch, persisted
/// and incremented at every start, and a counter within it. A packet's
/// transfers carry its trace ID, which is how the journal and the packet
/// trace line up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraceId {
    /// The boot the packet arrived in.
    pub epoch: u64,
    /// The packet's number within the boot.
    pub sequence: u64,
}

/// Why a transfer exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    /// An ILP packet. Pending while the packet is in flight; journaled only
    /// once posted.
    Packet(TraceId),
    /// A settlement with a peer. Journaled while pending too, because the
    /// money may already be on its way when the node restarts (ACC-16).
    Settlement {
        /// The settlement scheme.
        scheme: Name,
        /// The scheme's ID for it, unique within the scheme.
        id: Name,
    },
    /// The credit extended to a peer when its accounts are opened (ACC-5).
    Trust,
    /// Money the owner put on the underlying ledger.
    Deposit,
    /// Money the owner took off the underlying ledger.
    Withdrawal,
    /// A difference between a settlement module's balance and its last
    /// checkpoint, found at activation (ACC-6).
    Reconciliation,
    /// The compensation for a packet whose Fulfill the upstream peer
    /// refused after it was posted here: it reversed the packet, or booked
    /// the loss. Nodes booked these before each kept its own Fulfills
    /// (spec/decisions/own-fulfills.md); it stays for the journals that have
    /// it.
    Refusal(TraceId),
    /// A fee the underlying ledger charged for one of our transactions.
    Fee {
        /// The settlement scheme.
        scheme: Name,
        /// The transaction.
        id: Name,
    },
    /// Money a peering over a settlement hub cost us beyond its hold
    /// (spec/areas/hubs.md#the-members-side).
    Loss {
        /// The settlement scheme.
        scheme: Name,
        /// What on the underlying ledger it comes from.
        id: Name,
    },
    /// The owner's payment out of a scheme's account, such as a deposit
    /// into a settlement hub (spec/areas/settlement.md#the-scheme-interface):
    /// pending while it is on its way, so the owner's funds are held for
    /// it, posted when it applied, voided when it moved nothing. Journaled
    /// while pending too, as a settlement is: it may be on its way when the
    /// node restarts.
    OwnerPayment {
        /// The settlement scheme that pays it.
        scheme: Name,
        /// The scheme's ID for it.
        id: Name,
    },
}

impl Reference {
    /// Whether the transfer reports something that already happened
    /// elsewhere. The ledger records those even past a limit: refusing them
    /// would make the books disagree with reality, not prevent anything.
    #[must_use]
    pub const fn is_report(&self) -> bool {
        matches!(
            self,
            Self::Deposit
                | Self::Withdrawal
                | Self::Reconciliation
                | Self::Refusal(_)
                | Self::Fee { .. }
                | Self::Loss { .. }
        )
    }

    /// Whether a pending transfer with this reference must survive a
    /// restart.
    #[must_use]
    pub const fn is_durable_while_pending(&self) -> bool {
        matches!(self, Self::Settlement { .. } | Self::OwnerPayment { .. })
    }
}
