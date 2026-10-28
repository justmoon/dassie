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
    /// The node's account in its hub of XRP on the XRP Ledger testnet
    /// (spec/designs/settlement-hubs.md).
    HubXrplTestnetXrp,
    /// The node's account in its hub of DSUSD on the XRP Ledger testnet.
    HubXrplTestnetDsusd,
    /// The node's account in its hub of XRP on the XRP Ledger mainnet:
    /// real money (spec/decisions/hub-scheme-names.md).
    HubXrplXrp,
    /// The node's account in its hub of RLUSD on the XRP Ledger mainnet:
    /// real money.
    HubXrplRlusd,
}

impl AssetKind {
    /// Every kind, in a fixed order.
    pub const ALL: &'static [Self] = &[
        Self::XrplTestnetAccount,
        Self::XrplAccount,
        Self::HubXrplTestnetXrp,
        Self::HubXrplTestnetDsusd,
        Self::HubXrplXrp,
        Self::HubXrplRlusd,
    ];

    /// Its name, as IDs and the database write it. A hub account's is the
    /// ID of the scheme over its hub (spec/decisions/hub-scheme-names.md).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XrplTestnetAccount => "xrpl-testnet-account",
            Self::XrplAccount => "xrpl-account",
            Self::HubXrplTestnetXrp => "hub-xrpl-testnet-xrp",
            Self::HubXrplTestnetDsusd => "hub-xrpl-testnet-dsusd",
            Self::HubXrplXrp => "hub-xrpl-xrp",
            Self::HubXrplRlusd => "hub-xrpl-rlusd",
        }
    }

    /// Whether the node may hold several of this kind, each with a number
    /// of its own: an XRPL account, numbered by the index of its seed path.
    /// Of any other kind it holds one at most, as it is in one hub per
    /// asset (spec/decisions/hubs-only.md), so the kind alone names it.
    #[must_use]
    pub const fn numbered(self) -> bool {
        match self {
            Self::XrplTestnetAccount | Self::XrplAccount => true,
            Self::HubXrplTestnetXrp
            | Self::HubXrplTestnetDsusd
            | Self::HubXrplXrp
            | Self::HubXrplRlusd => false,
        }
    }
}

/// An external asset's ID: its kind, and for a numbered kind its number
/// among the node's of that kind. Written `<kind>/<index>` for a numbered
/// kind, such as `xrpl-testnet-account/0`, and `<kind>` alone for any
/// other, such as `hub-xrpl-testnet-xrp`. No kind's name holds a `/`, so
/// each text names one ID at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId {
    /// What it is.
    kind: AssetKind,
    /// Which of the node's of that kind, for a numbered kind: the first is
    /// 0; `None` for any other. Only the first of each numbered kind is
    /// built; the number is kept, in every stored ID and the owner's config
    /// of the asset, so that a further account of a kind, which a new seed
    /// path would derive, takes an ID of its own without renaming the
    /// first's (spec/areas/settlement.md#external-assets).
    index: Option<u16>,
}

impl AssetId {
    /// The node's asset numbered `index` of `kind`.
    ///
    /// # Panics
    ///
    /// If `kind` is not numbered.
    #[must_use]
    pub const fn numbered(kind: AssetKind, index: u16) -> Self {
        assert!(
            kind.numbered(),
            "a kind the node holds one of has no number"
        );
        Self {
            kind,
            index: Some(index),
        }
    }

    /// The node's one asset of `kind`.
    ///
    /// # Panics
    ///
    /// If `kind` is numbered.
    #[must_use]
    pub const fn sole(kind: AssetKind) -> Self {
        assert!(!kind.numbered(), "a numbered kind needs a number");
        Self { kind, index: None }
    }

    /// What it is.
    #[must_use]
    pub const fn kind(self) -> AssetKind {
        self.kind
    }

    /// Which of the node's of its kind, for a numbered kind; `None` for
    /// any other.
    #[must_use]
    pub const fn index(self) -> Option<u16> {
        self.index
    }

    /// The ID written as `text`, if it is one.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, index) = match text.split_once('/') {
            Some((kind, index)) => (kind, Some(index)),
            None => (text, None),
        };
        let kind = AssetKind::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == kind)?;
        let Some(index) = index else {
            return (!kind.numbered()).then(|| Self::sole(kind));
        };
        // One way to write each number: no sign, no leading zero.
        if !kind.numbered()
            || index.is_empty()
            || !index.bytes().all(|byte| byte.is_ascii_digit())
            || (index.len() > 1 && index.starts_with('0'))
        {
            return None;
        }
        Some(Self::numbered(kind, index.parse().ok()?))
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.index {
            Some(index) => write!(f, "{}/{index}", self.kind.as_str()),
            None => f.write_str(self.kind.as_str()),
        }
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

/// What an external asset could not prove of its ledger, counted since it
/// was made, never kept: the node warns of each new one and shows the
/// counts to the owner ([`ExternalAsset::unproven`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unproven {
    /// Gaps in the history it read that its server no longer had: what
    /// fell in one, its schemes matched through suspense.
    pub history_gaps: u64,
    /// Settlements of its own past their last ledger it could prove
    /// neither applied nor absent: each stays pending, for the owner.
    pub settlements: u64,
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

    /// What it could not prove of its ledger, counted since it was made:
    /// nothing by default.
    fn unproven(&self) -> Unproven {
        Unproven::default()
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

    /// A settlement through it is expected or about to be made until
    /// `until_ms`: money the node asked for may arrive on its ledger, such
    /// as a withdrawal from a settlement hub, or the node is about to pay
    /// out of it. An asset that reads its ledger on demand reads it on
    /// every ledger meanwhile, so that it sees what arrives as soon as its
    /// ledger shows it, and signs from a fresh view of it. Returns whether
    /// it could sign now: one that cannot yet gets ready, and is asked again
    /// at a later check, which is no failure. Nothing to do, and ready, by
    /// default.
    fn settling(&mut self, until_ms: u64) -> bool {
        let _ = until_ms;
        true
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
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    // spec/areas/settlement.md#external-assets: an XRPL account's ID is its
    // kind and its number, and each is written one way.
    #[test]
    fn numbered_ids_are_written_one_way() {
        let first = AssetId::numbered(AssetKind::XrplTestnetAccount, 0);
        assert_eq!(first.to_string(), "xrpl-testnet-account/0");
        assert_eq!(AssetId::parse("xrpl-testnet-account/0"), Some(first));
        assert_eq!(
            AssetId::parse("xrpl-testnet-account/65535").and_then(AssetId::index),
            Some(u16::MAX)
        );
        assert_eq!(
            AssetId::parse("xrpl-account/0").map(AssetId::kind),
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

    // spec/decisions/hub-scheme-names.md: a hub account's ID is the ID of
    // the scheme over its hub, with no number; the IDs from before, of a
    // kind and a number, are no IDs.
    #[test]
    fn a_hub_accounts_id_is_its_schemes() {
        for (kind, text) in [
            (AssetKind::HubXrplTestnetXrp, "hub-xrpl-testnet-xrp"),
            (AssetKind::HubXrplTestnetDsusd, "hub-xrpl-testnet-dsusd"),
            (AssetKind::HubXrplXrp, "hub-xrpl-xrp"),
            (AssetKind::HubXrplRlusd, "hub-xrpl-rlusd"),
        ] {
            let id = AssetId::sole(kind);
            assert_eq!(id.to_string(), text);
            assert_eq!(AssetId::parse(text), Some(id));
            assert_eq!(id.index(), None);
            for wrong in [format!("{text}/0"), format!("{text}/1"), format!("{text}/")] {
                assert_eq!(AssetId::parse(&wrong), None, "{wrong}");
            }
        }
        for old in [
            "hub-testnet-account/0",
            "hub-testnet-account/1",
            "hub-account/0",
            "hub-account/1",
            "hub-testnet-account",
            "hub-account",
        ] {
            assert_eq!(AssetId::parse(old), None, "{old}");
        }
    }

    // Every kind's IDs read back as written, no two kinds' meet, and the
    // longest fits the store's bound on an asset's ID (64 bytes).
    #[test]
    fn every_id_reads_back_as_written() {
        for (at, kind) in AssetKind::ALL.iter().copied().enumerate() {
            assert!(!kind.as_str().contains('/'), "{kind:?}");
            assert!(
                AssetKind::ALL[at + 1..]
                    .iter()
                    .all(|other| other.as_str() != kind.as_str()),
                "{kind:?}"
            );
            let ids = if kind.numbered() {
                vec![
                    AssetId::numbered(kind, 0),
                    AssetId::numbered(kind, u16::MAX),
                ]
            } else {
                vec![AssetId::sole(kind)]
            };
            for id in ids {
                let text = id.to_string();
                assert_eq!(AssetId::parse(&text), Some(id));
                assert_eq!(id.kind(), kind);
                assert_eq!(id.index().is_some(), kind.numbered());
                assert!(text.len() <= 64, "{text}");
            }
        }
    }

    #[test]
    #[should_panic(expected = "has no number")]
    fn a_sole_kind_takes_no_number() {
        let _ = AssetId::numbered(AssetKind::HubXrplTestnetXrp, 0);
    }

    #[test]
    #[should_panic(expected = "needs a number")]
    fn a_numbered_kind_needs_one() {
        let _ = AssetId::sole(AssetKind::XrplTestnetAccount);
    }
}
