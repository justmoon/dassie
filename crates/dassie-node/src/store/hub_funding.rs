//! Moving the owner's funds between the owner book of a hub's asset and
//! the node's account in the hub, for each hub the node is in
//! (spec/areas/hubs.md#in-the-node): whether the node does it by itself,
//! the target the owner set for it, until when an owner's move pauses
//! that, and the move on its way, kept so a restart never pays it twice. Each hub's row is keyed by the scheme
//! over it; a hub without one reads as the defaults. Only the core's
//! funding of a scheme over a hub writes a row, and the code names those
//! schemes, so the table holds a handful.

use std::collections::BTreeMap;

use dassie_ledger::Name;
use rusqlite::{OptionalExtension, Transaction, params};

use super::{Cached, Store, StoreError};

/// The move on its way, as the database keeps it. Amounts are in the hub
/// asset's smallest unit, as the hub counts it: drops of XRP, or
/// millionths of a token such as DSUSD. The columns that hold them are
/// named for drops (`move_drops`, `given_up_drops`), and hold a token
/// hub's millionths alike: a row is keyed by the scheme over its hub,
/// which names the asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubMoveRow {
    /// Whether it is a deposit into the hub, rather than a withdrawal.
    pub deposit: bool,
    /// How much, in the hub asset's smallest unit.
    pub drops: u64,
    /// The value of the hub account's counter that shows it done.
    pub target: u64,
    /// Its payment's ID with the scheme of the side it leaves.
    pub payment: String,
    /// Whether the owner asked for it.
    pub by_owner: bool,
    /// When it was made.
    pub sent_ms: u64,
    /// When the XRP Ledger applied a deposit.
    pub applied_ms: Option<u64>,
}

/// What the database keeps of one hub's moves, beyond the owner's
/// choices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubFundingRow {
    /// Until when an owner's move pauses the node's own.
    pub paused_until_ms: u64,
    /// The move on its way.
    pub moving: Option<HubMoveRow>,
    /// The move the owner gave up last, while it holds the node's own
    /// moves back: whether a deposit, and how much.
    pub given_up: Option<(bool, u64)>,
}

/// One hub's row: the owner's choices, whether the node moves funds by
/// itself and the target it keeps, and the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubFunding {
    /// Whether the node moves funds by itself.
    pub automatic: bool,
    /// The target the owner set for what the node keeps in the hub, in the
    /// hub ledger's units; `None` for the default
    /// (spec/decisions/generous-hub-target.md).
    pub target: Option<u64>,
    /// The rest.
    pub row: HubFundingRow,
}

impl Default for HubFunding {
    /// A hub without a row: the node's own moves on, the default target,
    /// no pause, nothing on its way.
    fn default() -> Self {
        Self {
            automatic: true,
            target: None,
            row: HubFundingRow::default(),
        }
    }
}

type Columns = (
    String,
    i64,
    i64,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

const SELECT: &str = "SELECT scheme, automatic, paused_until_ms, move_kind, move_drops, \
     move_target, move_payment, move_by_owner, move_sent_ms, move_applied_ms, \
     given_up_kind, given_up_drops, target FROM hub_funding";

fn columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<Columns> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ))
}

/// A hub's row, checked, with its scheme.
fn funding(columns: Columns) -> Result<(Name, HubFunding), StoreError> {
    let corrupt = || StoreError::Corrupt("hub funding");
    let (
        scheme,
        automatic,
        paused,
        kind,
        drops,
        move_target,
        payment,
        by_owner,
        sent,
        applied,
        given,
        gave,
        hub_target,
    ) = columns;
    let scheme = Name::new(&scheme).map_err(|_| corrupt())?;
    let unsigned = |value: i64| u64::try_from(value).map_err(|_| corrupt());
    let moving = match (kind, drops, move_target, payment, by_owner, sent) {
        (None, ..) => None,
        (Some(kind), Some(drops), Some(target), Some(payment), Some(by_owner), Some(sent)) => {
            Some(HubMoveRow {
                deposit: kind == "deposit",
                drops: unsigned(drops)?,
                target: unsigned(target)?,
                payment,
                by_owner: by_owner != 0,
                sent_ms: unsigned(sent)?,
                applied_ms: applied.map(unsigned).transpose()?,
            })
        }
        _ => return Err(corrupt()),
    };
    let given_up = match (given, gave) {
        (None, None) => None,
        (Some(kind), Some(drops)) => Some((kind == "deposit", unsigned(drops)?)),
        _ => return Err(corrupt()),
    };
    Ok((
        scheme,
        HubFunding {
            automatic: automatic != 0,
            target: hub_target.map(unsigned).transpose()?,
            row: HubFundingRow {
                paused_until_ms: unsigned(paused)?,
                moving,
                given_up,
            },
        },
    ))
}

impl Store {
    /// The moves over the hub of `scheme`: whether the node moves funds by
    /// itself, the target the owner set, until when it waits for the
    /// owner's last move, and the move on its way. A hub with no row reads
    /// as the defaults: the node's own moves on, the default target, no
    /// pause, nothing on its way.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn hub_funding(&self, scheme: Name) -> Result<HubFunding, StoreError> {
        let columns: Option<Columns> = self
            .lock()
            .query_row_cached(
                &format!("{SELECT} WHERE scheme = ?1"),
                [scheme.as_str()],
                columns,
            )
            .optional()?;
        Ok(match columns {
            Some(columns) => funding(columns)?.1,
            None => HubFunding::default(),
        })
    }

    /// Every hub's row, by the scheme over it, for a test to see that each
    /// hub's is kept on its own.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    #[cfg(test)]
    pub fn hub_fundings(&self) -> Result<BTreeMap<Name, HubFunding>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare_cached(&format!("{SELECT} ORDER BY scheme"))?;
        let mut fundings = BTreeMap::new();
        for row in statement.query_map([], columns)? {
            let (scheme, funding) = funding(row?)?;
            fundings.insert(scheme, funding);
        }
        Ok(fundings)
    }
}

/// Writes what a batch changed of each hub's moves: a hub without a row
/// gets one first. A target of `None` is the default again.
pub(super) fn write_hub_funding(
    transaction: &Transaction<'_>,
    automatic: &BTreeMap<Name, bool>,
    targets: &BTreeMap<Name, Option<u64>>,
    fundings: &BTreeMap<Name, HubFundingRow>,
) -> Result<(), StoreError> {
    for scheme in automatic
        .keys()
        .chain(targets.keys())
        .chain(fundings.keys())
    {
        transaction.execute_cached(
            "INSERT INTO hub_funding (scheme, automatic, paused_until_ms) VALUES (?1, 1, 0) \
             ON CONFLICT (scheme) DO NOTHING",
            [scheme.as_str()],
        )?;
    }
    for (scheme, automatic) in automatic {
        transaction.execute_cached(
            "UPDATE hub_funding SET automatic = ?1 WHERE scheme = ?2",
            params![i64::from(*automatic), scheme.as_str()],
        )?;
    }
    let signed = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    for (scheme, target) in targets {
        transaction.execute_cached(
            "UPDATE hub_funding SET target = ?1 WHERE scheme = ?2",
            params![target.map(signed), scheme.as_str()],
        )?;
    }
    for (scheme, funding) in fundings {
        let moving = funding.moving.as_ref();
        transaction.execute_cached(
            "UPDATE hub_funding SET paused_until_ms = ?1, move_kind = ?2, move_drops = ?3, \
             move_target = ?4, move_payment = ?5, move_by_owner = ?6, move_sent_ms = ?7, \
             move_applied_ms = ?8, given_up_kind = ?9, given_up_drops = ?10 WHERE scheme = ?11",
            params![
                signed(funding.paused_until_ms),
                moving.map(|m| if m.deposit { "deposit" } else { "withdraw" }),
                moving.map(|m| signed(m.drops)),
                moving.map(|m| signed(m.target)),
                moving.map(|m| m.payment.as_str()),
                moving.map(|m| i64::from(m.by_owner)),
                moving.map(|m| signed(m.sent_ms)),
                moving.and_then(|m| m.applied_ms.map(signed)),
                funding
                    .given_up
                    .map(|(deposit, _)| if deposit { "deposit" } else { "withdraw" }),
                funding.given_up.map(|(_, drops)| signed(drops)),
                scheme.as_str(),
            ],
        )?;
    }
    Ok(())
}
