//! Account paths: `<ledger>:<name>`, typed.

use core::fmt;

use crate::account::Limit;
use crate::ledger_id::LedgerId;
use crate::name::Name;

/// A sub-account of the owner's pool, one per spending endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OwnerSub {
    /// A BTP token, by its ID.
    Btp(u64),
    /// A local endpoint, by name.
    Local(Name),
}

/// An account on a ledger. The chart of spec 5 §2.2, with the owner's
/// sub-accounts added (spec 7 §4.5). The original documented
/// `liabilities/…` accounts it never had (ACC-1); a peer's debt to us and
/// ours to it share `assets/interledger/<peer>`, whose balance can have
/// either sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccountName {
    /// Money on the underlying ledger.
    AssetsSettlement,
    /// What a peer owes us (or we it), net of the credit we extend.
    AssetsInterledger(Name),
    /// The credit we extend to a peer.
    ContraTrust(Name),
    /// The owner's pool.
    EquityOwner,
    /// A sub-account of the owner's pool.
    EquityOwnerSub(OwnerSub),
    /// Differences between the books and the underlying ledger.
    EquitySuspense,
    /// Connector fees earned.
    RevenueFees,
    /// The source side of currency exchange.
    RevenueFx,
    /// Fees paid.
    ExpensesFees,
    /// The destination side of currency exchange.
    ExpensesFx,
    /// Money paid on for packets an upstream peer then refused: what a
    /// hop bears when its Fulfill did not count upstream (D17).
    ExpensesLosses,
}

/// A path that does not name an account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidPath;

impl fmt::Display for InvalidPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not an account path")
    }
}

/// An account: its ledger and its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountPath {
    /// The ledger.
    pub ledger: LedgerId,
    /// The account.
    pub name: AccountName,
}

impl AccountPath {
    /// The account `name` on `ledger`.
    #[must_use]
    pub const fn new(ledger: LedgerId, name: AccountName) -> Self {
        Self { ledger, name }
    }

    /// The limit on the account itself. The owner's pool has none of its
    /// own: its group carries one.
    #[must_use]
    pub const fn limit(&self) -> Limit {
        match self.name {
            AccountName::AssetsInterledger(_) => Limit::DebitsMustNotExceedCredits,
            AccountName::ContraTrust(_) => Limit::CreditsMustNotExceedDebits,
            _ => Limit::None,
        }
    }

    /// The root of the account's group, if it is in one. The owner's pool
    /// is the root of its own group, which holds what the node owns on the
    /// ledger beyond what it owes peers: the owner's sub-accounts, and what
    /// reconciliations, fees and losses moved (M7). So whatever the node
    /// gains or loses is the owner's to spend, or not.
    #[must_use]
    pub const fn group_root(&self) -> Option<Self> {
        match self.name {
            AccountName::EquityOwner
            | AccountName::EquityOwnerSub(_)
            | AccountName::EquitySuspense
            | AccountName::RevenueFees
            | AccountName::ExpensesFees
            | AccountName::ExpensesLosses => Some(Self::new(self.ledger, AccountName::EquityOwner)),
            _ => None,
        }
    }

    /// The limit on the total of the group this account is the root of.
    #[must_use]
    pub const fn group_limit(&self) -> Limit {
        match self.name {
            AccountName::EquityOwner => Limit::DebitsMustNotExceedCredits,
            _ => Limit::None,
        }
    }

    /// Parses the canonical form, as [`Display`](fmt::Display) writes it.
    ///
    /// # Errors
    ///
    /// If `text` is not a path.
    pub fn parse(text: &str) -> Result<Self, InvalidPath> {
        let (ledger, name) = text.split_once(':').ok_or(InvalidPath)?;
        let ledger = LedgerId::parse(ledger).ok_or(InvalidPath)?;
        Ok(Self::new(ledger, parse_name(name)?))
    }
}

fn parse_name(text: &str) -> Result<AccountName, InvalidPath> {
    let fixed = match text {
        "assets/settlement" => Some(AccountName::AssetsSettlement),
        "equity/owner" => Some(AccountName::EquityOwner),
        "equity/suspense" => Some(AccountName::EquitySuspense),
        "revenue/fees" => Some(AccountName::RevenueFees),
        "revenue/fx" => Some(AccountName::RevenueFx),
        "expenses/fees" => Some(AccountName::ExpensesFees),
        "expenses/fx" => Some(AccountName::ExpensesFx),
        "expenses/losses" => Some(AccountName::ExpensesLosses),
        _ => None,
    };
    if let Some(name) = fixed {
        return Ok(name);
    }
    let name = |rest: &str| Name::new(rest).map_err(|_| InvalidPath);
    if let Some(rest) = text.strip_prefix("assets/interledger/") {
        return Ok(AccountName::AssetsInterledger(name(rest)?));
    }
    if let Some(rest) = text.strip_prefix("contra/trust/") {
        return Ok(AccountName::ContraTrust(name(rest)?));
    }
    if let Some(rest) = text.strip_prefix("equity/owner/local/") {
        return Ok(AccountName::EquityOwnerSub(OwnerSub::Local(name(rest)?)));
    }
    if let Some(rest) = text.strip_prefix("equity/owner/btp/") {
        // Canonical decimal only: no sign, no leading zeros.
        if rest.is_empty() || (rest.len() > 1 && rest.starts_with('0')) {
            return Err(InvalidPath);
        }
        if !rest.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidPath);
        }
        let id = rest.parse().map_err(|_| InvalidPath)?;
        return Ok(AccountName::EquityOwnerSub(OwnerSub::Btp(id)));
    }
    Err(InvalidPath)
}

impl fmt::Display for AccountName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AssetsSettlement => f.write_str("assets/settlement"),
            Self::AssetsInterledger(node) => write!(f, "assets/interledger/{node}"),
            Self::ContraTrust(node) => write!(f, "contra/trust/{node}"),
            Self::EquityOwner => f.write_str("equity/owner"),
            Self::EquityOwnerSub(OwnerSub::Btp(id)) => write!(f, "equity/owner/btp/{id}"),
            Self::EquityOwnerSub(OwnerSub::Local(name)) => write!(f, "equity/owner/local/{name}"),
            Self::EquitySuspense => f.write_str("equity/suspense"),
            Self::RevenueFees => f.write_str("revenue/fees"),
            Self::RevenueFx => f.write_str("revenue/fx"),
            Self::ExpensesFees => f.write_str("expenses/fees"),
            Self::ExpensesFx => f.write_str("expenses/fx"),
            Self::ExpensesLosses => f.write_str("expenses/losses"),
        }
    }
}

impl fmt::Display for AccountPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.ledger, self.name)
    }
}

#[cfg(test)]
mod tests {
    use std::string::ToString;

    use super::*;

    #[test]
    fn paths_round_trip() {
        let node = Name::new("dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW").unwrap();
        let names = [
            AccountName::AssetsSettlement,
            AccountName::AssetsInterledger(node),
            AccountName::ContraTrust(node),
            AccountName::EquityOwner,
            AccountName::EquityOwnerSub(OwnerSub::Btp(0)),
            AccountName::EquityOwnerSub(OwnerSub::Btp(u64::MAX)),
            AccountName::EquityOwnerSub(OwnerSub::Local(Name::new("stream").unwrap())),
            AccountName::EquitySuspense,
            AccountName::RevenueFees,
            AccountName::RevenueFx,
            AccountName::ExpensesFees,
            AccountName::ExpensesFx,
            AccountName::ExpensesLosses,
        ];
        for ledger in LedgerId::ALL {
            for name in names {
                let path = AccountPath::new(ledger, name);
                assert_eq!(AccountPath::parse(&path.to_string()), Ok(path));
            }
        }
    }

    #[test]
    fn refuses_non_canonical_paths() {
        for text in [
            "",
            "stub+usd",
            "stub+usd:",
            "unreachable",
            "stub+eur:equity/owner",
            "stub+usd:equity/owner/",
            "stub+usd:equity/owner/btp/",
            "stub+usd:equity/owner/btp/01",
            "stub+usd:equity/owner/btp/+1",
            "stub+usd:equity/owner/btp/18446744073709551616",
            "stub+usd:assets/interledger/",
            "stub+usd:assets/interledger/a/b",
            "stub+usd:liabilities/x",
        ] {
            assert_eq!(AccountPath::parse(text), Err(InvalidPath), "{text}");
        }
    }

    #[test]
    fn the_owner_accounts_form_one_group_per_ledger() {
        let root = AccountPath::new(LedgerId::StubUsd, AccountName::EquityOwner);
        let sub = AccountPath::new(
            LedgerId::StubUsd,
            AccountName::EquityOwnerSub(OwnerSub::Btp(3)),
        );
        assert_eq!(root.group_root(), Some(root));
        assert_eq!(sub.group_root(), Some(root));
        assert_eq!(sub.limit(), Limit::None);
        assert_eq!(root.limit(), Limit::None);
        assert_eq!(root.group_limit(), Limit::DebitsMustNotExceedCredits);
        let other = AccountPath::new(LedgerId::XrplTestnetXrp, AccountName::EquityOwner);
        assert_ne!(other.group_root(), root.group_root());
        let settlement = AccountPath::new(LedgerId::StubUsd, AccountName::AssetsSettlement);
        assert_eq!(settlement.group_root(), None);
    }

    #[test]
    fn what_the_node_gains_or_loses_is_the_owners() {
        let root = AccountPath::new(LedgerId::StubUsd, AccountName::EquityOwner);
        for name in [
            AccountName::EquitySuspense,
            AccountName::RevenueFees,
            AccountName::ExpensesFees,
            AccountName::ExpensesLosses,
        ] {
            let path = AccountPath::new(LedgerId::StubUsd, name);
            assert_eq!(path.group_root(), Some(root), "{path}");
            assert_eq!(path.limit(), Limit::None, "{path}");
        }
        // Exchange joins with the rule that the owner's endpoints never
        // convert (M7c); peers' accounts never join.
        for name in [
            AccountName::RevenueFx,
            AccountName::ExpensesFx,
            AccountName::AssetsInterledger(Name::new("n").unwrap()),
        ] {
            assert_eq!(AccountPath::new(LedgerId::StubUsd, name).group_root(), None);
        }
    }
}
