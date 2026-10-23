//! Moving the owner's funds between the node's XRPL account and its
//! settlement hub (spec/areas/hubs.md#in-the-node): whether the node does it
//! by itself, until when an owner's move pauses that, and the move on its
//! way, kept so a restart never pays it twice.

use rusqlite::{OptionalExtension, Transaction, params};

use super::{Store, StoreError};

/// The move on its way, as the database keeps it. Amounts are in drops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubMoveRow {
    /// Whether it is a deposit into the hub, rather than a withdrawal.
    pub deposit: bool,
    /// How much.
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

/// What the database keeps of the moves, beyond whether they are automatic.
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

type Columns = (
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
);

impl Store {
    /// Whether the node moves funds by itself, until when it waits for the
    /// owner's last move, and the move on its way.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn hub_funding(&self) -> Result<(bool, HubFundingRow), StoreError> {
        let columns: Option<Columns> = self
            .lock()
            .query_row(
                "SELECT automatic, paused_until_ms, move_kind, move_drops, move_target, \
                 move_payment, move_by_owner, move_sent_ms, move_applied_ms, \
                 given_up_kind, given_up_drops FROM hub_funding WHERE id = 1",
                [],
                |row| {
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
                    ))
                },
            )
            .optional()?;
        let corrupt = || StoreError::Corrupt("hub funding");
        let (automatic, paused, kind, drops, target, payment, by_owner, sent, applied, given, gave) =
            columns.ok_or_else(corrupt)?;
        let unsigned = |value: i64| u64::try_from(value).map_err(|_| corrupt());
        let moving = match (kind, drops, target, payment, by_owner, sent) {
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
            automatic != 0,
            HubFundingRow {
                paused_until_ms: unsigned(paused)?,
                moving,
                given_up,
            },
        ))
    }
}

/// Writes what a batch changed of the moves.
pub(super) fn write_hub_funding(
    transaction: &Transaction<'_>,
    automatic: Option<bool>,
    funding: Option<&HubFundingRow>,
) -> Result<(), StoreError> {
    if let Some(automatic) = automatic {
        transaction.execute(
            "UPDATE hub_funding SET automatic = ?1 WHERE id = 1",
            [i64::from(automatic)],
        )?;
    }
    let Some(funding) = funding else {
        return Ok(());
    };
    let signed = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    let moving = funding.moving.as_ref();
    transaction.execute(
        "UPDATE hub_funding SET paused_until_ms = ?1, move_kind = ?2, move_drops = ?3, \
         move_target = ?4, move_payment = ?5, move_by_owner = ?6, move_sent_ms = ?7, \
         move_applied_ms = ?8, given_up_kind = ?9, given_up_drops = ?10 WHERE id = 1",
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
        ],
    )?;
    Ok(())
}
