//! External assets: what the node holds outside its books, such as an
//! account on the XRP Ledger, apart from the settlement schemes that settle
//! through it (spec/areas/settlement.md#external-assets).
//!
//! An external asset owns what more than one scheme would otherwise race
//! for: its keys, its connection to its ledger, the transactions it signs
//! and the order they take, and what it holds, as its ledger says. It is
//! sans-IO like a scheme: it says what to send its ledger
//! ([`AssetEffect`]) and the node feeds it what comes back
//! ([`LedgerInput`]). What it learns for the schemes on it, it keeps until
//! the node has handed it to each of them
//! ([`SettlementScheme::on_asset`](crate::SettlementScheme::on_asset)).

use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt;

use dassie_ledger::Name;

use crate::scheme::{LedgerStatus, SchemeError};

/// A kind of external asset. The list is closed, as the ledgers' is: a new
/// kind comes with the schemes that settle through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AssetKind {
    /// An account on the XRP Ledger testnet.
    XrplTestnetAccount,
    /// An account on the XRP Ledger mainnet
    /// (spec/decisions/mainnet-schemes.md).
    XrplAccount,
    /// An account in a settlement hub on the XRP Ledger testnet
    /// (spec/designs/settlement-hubs.md).
    HubTestnetAccount,
}

impl AssetKind {
    /// Every kind, in a fixed order.
    pub const ALL: &'static [Self] = &[
        Self::XrplTestnetAccount,
        Self::XrplAccount,
        Self::HubTestnetAccount,
    ];

    /// Its name, as IDs and the database write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XrplTestnetAccount => "xrpl-testnet-account",
            Self::XrplAccount => "xrpl-account",
            Self::HubTestnetAccount => "hub-testnet-account",
        }
    }
}

/// An external asset's ID: its kind, and its number among the node's of
/// that kind. Written `<kind>/<index>`, such as `xrpl-testnet-account/0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId {
    /// What it is.
    pub kind: AssetKind,
    /// Which of the node's of that kind: the first is 0.
    pub index: u16,
}

impl AssetId {
    /// The ID written as `text`, if it is one.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, index) = text.split_once('/')?;
        let kind = AssetKind::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == kind)?;
        // One way to write each number: no sign, no leading zero.
        if index.is_empty()
            || !index.bytes().all(|byte| byte.is_ascii_digit())
            || (index.len() > 1 && index.starts_with('0'))
        {
            return None;
        }
        Some(Self {
            kind,
            index: index.parse().ok()?,
        })
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind.as_str(), self.index)
    }
}

/// What an external asset holds of one asset, as its ledger last said:
/// never what the books add up (spec/decisions/external-assets.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holding {
    /// The asset's code on its ledger, such as `XRP`.
    pub code: String,
    /// Who issued it, for an issued asset: an address on the ledger.
    pub issuer: Option<String>,
    /// How much, in units of `10^-scale` of the asset.
    pub amount: u128,
    /// The scale `amount` and `reserve` are written at.
    pub scale: u8,
    /// What the ledger holds back of it, in the same units: never
    /// spendable.
    pub reserve: u128,
    /// What its issuer stops, for an issued asset; nothing otherwise.
    pub freeze: Freeze,
}

/// What an issued asset's issuer stops of a holding, as the ledger last
/// said (spec/decisions/issued-assets.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Freeze {
    /// The issuer froze this line: it pays only the issuer, and still
    /// receives.
    pub line: bool,
    /// The issuer deep-froze this line: it neither pays nor receives, but
    /// to and from the issuer.
    pub deep: bool,
    /// The issuer froze its asset for every holder: no holder pays another.
    pub global: bool,
}

/// What the node passes an external asset from its connection to its
/// ledger, from the faucet, and from a document it fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerInput<'a> {
    /// The connection is up.
    Connected,
    /// A message came.
    Message(&'a [u8]),
    /// The connection went down, and is tried again; or the node closed
    /// it, or made it anew.
    Disconnected,
    /// The faucet answered, or did not.
    Funded(bool),
    /// A fetch ([`AssetEffect::Fetch`]) of `url` answered with `body`, or
    /// failed: `None` for no answer, a status other than success, a
    /// redirect, or a body longer than asked for.
    Fetched {
        /// What was fetched, as the asset asked.
        url: &'a str,
        /// What came, whole.
        body: Option<&'a [u8]>,
    },
    /// A message came from one of its endpoints
    /// ([`ExternalAsset::endpoints`]), named by its index. The node takes
    /// it only from that endpoint, so the asset may take who sent it as
    /// said.
    From {
        /// The endpoint's index.
        endpoint: usize,
        /// The message.
        message: &'a [u8],
    },
}

/// Something the node must do on an external asset's ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetEffect {
    /// Send a message over the asset's connection
    /// ([`ExternalAsset::connection`]).
    Send {
        /// The message.
        message: Vec<u8>,
    },
    /// Send a message to one of its endpoints
    /// ([`ExternalAsset::endpoints`]), named by its index. What comes back
    /// comes as [`LedgerInput::From`].
    SendTo {
        /// The endpoint's index.
        endpoint: usize,
        /// The message.
        message: Vec<u8>,
    },
    /// Connect to the ledger anew: the connection stopped answering.
    Reconnect,
    /// Ask a faucet to fund the account on a test ledger, by an HTTPS POST
    /// of `body` to `url`. The answer comes as [`LedgerInput::Funded`].
    Fund {
        /// The faucet.
        url: String,
        /// What to post.
        body: Vec<u8>,
    },
    /// Fetch a document by an HTTPS GET of `url`, such as a validator list
    /// from its publisher's site, following no redirect and reading at
    /// most `max_bytes`. The answer comes as [`LedgerInput::Fetched`], on
    /// the connection it was asked on: one made anew drops it, and the
    /// asset asks again.
    Fetch {
        /// What to fetch.
        url: String,
        /// The longest body taken.
        max_bytes: usize,
    },
}

/// Why a scheme no longer settles through an external asset
/// ([`ExternalAsset::detach`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detach {
    /// The owner removed it: what it settles in is no scheme's until it is
    /// added again.
    Removed,
    /// The books stopped it, until the node restarts: what it settles in is
    /// still on its books, and no other scheme spends it meanwhile.
    Stopped,
}

/// An external asset (spec/areas/settlement.md#external-assets), sans-IO.
///
/// The node keeps one connection per asset, feeds it what comes over it,
/// then hands every scheme on it what it learnt
/// ([`SettlementScheme::on_asset`](crate::SettlementScheme::on_asset)),
/// tells it they have heard ([`told`](Self::told)), and carries out what
/// it asks ([`take_effects`](Self::take_effects)). Schemes on it reach
/// their own kind of asset by downcasting, which is why it is [`Any`].
pub trait ExternalAsset: Send + Any {
    /// Its ID.
    fn id(&self) -> AssetId;

    /// Where the owner sends money to fund it, if anywhere: an address on
    /// its ledger.
    fn address(&self) -> Option<String> {
        None
    }

    /// What it holds, each asset as its ledger last said; nothing until
    /// the ledger has said.
    fn holdings(&self) -> Vec<Holding>;

    /// How it stands with its ledger.
    fn status(&self) -> LedgerStatus;

    /// What it waits for, while it connects or syncs and something keeps
    /// it from going on, such as a validator list it has none of: shown to
    /// the owner beside its status. Nothing by default.
    fn waiting(&self) -> Option<&'static str> {
        None
    }

    /// Where the node connects for its ledger, if it has one: a WebSocket
    /// URL.
    fn connection(&self) -> Option<&str> {
        None
    }

    /// Where the node reaches its ledger when that is several parties it
    /// talks to apart rather than one connection, such as a settlement
    /// hub's validators: their endpoints, in an order that never changes
    /// while the asset runs. The asset names one by its index
    /// ([`AssetEffect::SendTo`], [`LedgerInput::From`]). None by default.
    fn endpoints(&self) -> &[String] {
        &[]
    }

    /// Takes in what came from its ledger, the faucet, or a fetch.
    fn on_ledger(&mut self, now_ms: u64, input: LedgerInput<'_>);

    /// When [`on_timer`](Self::on_timer) is due next, if ever.
    fn deadline(&self) -> Option<u64> {
        None
    }

    /// Runs its timers that are due.
    fn on_timer(&mut self, now_ms: u64) {
        let _ = now_ms;
    }

    /// Whether it can ask a faucet for test money on its ledger.
    fn has_faucet(&self) -> bool {
        false
    }

    /// Asks the faucet of a test ledger for money on the owner's behalf, as
    /// [`AssetEffect::Fund`]. Nothing is booked now: what the faucet sends
    /// is a deposit once the ledger shows it.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unsupported`] where it has no faucet;
    /// [`SchemeError::Unavailable`] while the faucet was asked too
    /// recently, or while there is no connection to take its answer on.
    fn ask_faucet(&mut self) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    /// What the owner may spend now on the ledger of `scheme`, a scheme on
    /// it, by the books: the owner's funds there, less what is pending. The
    /// node says it before each call that may sign, so a holding one scheme
    /// settles in and another spends as gas is spent only within the
    /// owner's funds (spec/decisions/external-assets.md).
    fn owner_funds(&mut self, scheme: Name, funds: u64) {
        let _ = (scheme, funds);
    }

    /// Every scheme on it has been handed what it learnt: it is forgotten.
    fn told(&mut self);

    /// Moves what it asks the node to do into `out`, in order.
    fn take_effects(&mut self, out: &mut Vec<AssetEffect>);

    /// `scheme` no longer settles through it: removed, or stopped. What it
    /// learns of the scheme's own meanwhile, such as the outcome of a
    /// transaction the scheme had it sign, it keeps for the scheme, which
    /// reads it once back. While a scheme is stopped, what it settles in is
    /// still on its books, and no other scheme on the asset spends it as
    /// gas.
    fn detach(&mut self, scheme: Name, why: Detach);

    /// What it must keep across restarts, such as the transactions it
    /// signed. The node writes it in the batch with what it explains, and
    /// hands it back when it makes the asset again.
    fn state(&self) -> Vec<u8>;

    /// A number that changes whenever [`state`](Self::state) does: the
    /// node writes the state with its next batch only then.
    fn state_version(&self) -> u64;

    /// Asserts what holds between calls (spec/decisions/tigerstyle.md).
    ///
    /// # Panics
    ///
    /// If its state is inconsistent.
    fn check_invariants(&self);
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn ids_are_written_one_way() {
        let first = AssetId {
            kind: AssetKind::XrplTestnetAccount,
            index: 0,
        };
        assert_eq!(first.to_string(), "xrpl-testnet-account/0");
        assert_eq!(AssetId::parse("xrpl-testnet-account/0"), Some(first));
        assert_eq!(
            AssetId::parse("xrpl-testnet-account/65535").map(|id| id.index),
            Some(u16::MAX)
        );
        for kind in AssetKind::ALL.iter().copied() {
            let id = AssetId { kind, index: 0 };
            assert_eq!(AssetId::parse(&id.to_string()), Some(id));
        }
        assert_eq!(
            AssetId::parse("xrpl-account/0").map(|id| id.kind),
            Some(AssetKind::XrplAccount)
        );
        for wrong in [
            "",
            "xrpl-testnet-account",
            "xrpl-testnet-account/",
            "xrpl-testnet-account/00",
            "xrpl-testnet-account/01",
            "xrpl-testnet-account/+1",
            "xrpl-testnet-account/65536",
            "xrpl-testnet/0",
            "xrpl-testnet-account/0/0",
        ] {
            assert_eq!(AssetId::parse(wrong), None, "{wrong}");
        }
    }
}
