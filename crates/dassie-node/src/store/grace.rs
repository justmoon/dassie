//! One grace limit per peer (D18): the owner's share, why each node was
//! banned, and what a peer's refusals cost us, read from the journal.

use dassie_identity::NodeId;
use dassie_ledger::{AccountId, LedgerId};
use rusqlite::{Row, Transaction, params};

use super::{Store, StoreError, from_sql_time, to_sql_time};

/// Most ban records kept. A record outlives its ban; past this, the
/// records of the bans lifted longest ago go first.
pub const MAX_BAN_RECORDS: usize = 2048;

/// The peer settings the owner chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerSettings {
    /// The share of the credit we extend to a peer that it may cost us in
    /// the window before it is banned, in parts per million.
    pub grace_share_ppm: u32,
}

impl PeerSettings {
    /// What a new node starts with: three quarters of the credit. One crash
    /// of an honest upstream with packets in flight can cost more than half
    /// of it (the simulation finds 0.6 of 1.1 units at worst); a peer can
    /// owe us no more than all of it, so a share below that bans one that
    /// never settles.
    pub const DEFAULT: Self = Self {
        grace_share_ppm: 750_000,
    };

    /// The largest share: ten times the credit.
    pub const MAX_SHARE_PPM: u32 = 10_000_000;

    /// Whether the settings can be stored.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.grace_share_ppm > 0 && self.grace_share_ppm <= Self::MAX_SHARE_PPM
    }
}

/// Why a node was banned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanReason {
    /// The owner banned it.
    Owner,
    /// It cost us more than its grace limit (D18).
    GraceLimit,
}

impl BanReason {
    /// Its stored form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::GraceLimit => "grace-limit",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "owner" => Some(Self::Owner),
            "grace-limit" => Some(Self::GraceLimit),
            _ => None,
        }
    }
}

/// What a peer cost us on one ledger, in its units, against its limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerCost {
    /// The ledger.
    pub ledger: LedgerId,
    /// The losses its refused Fulfills booked in the window.
    pub refused: u64,
    /// What it owes us and should have settled.
    pub overdue: u64,
    /// Its grace limit.
    pub grace: u64,
}

impl PeerCost {
    /// Everything it cost us.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.refused.saturating_add(self.overdue)
    }

    /// Whether that is past its limit.
    #[must_use]
    pub const fn over(&self) -> bool {
        self.total() > self.grace
    }
}

/// Why and when a node was banned, and when the ban was lifted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BanRecord {
    /// The node.
    pub node: NodeId,
    /// Why.
    pub reason: BanReason,
    /// When.
    pub banned_at_ms: u64,
    /// What it cost us, for a ban over its grace limit.
    pub cost: Option<PeerCost>,
    /// When the ban was lifted, if it was.
    pub unbanned_at_ms: Option<u64>,
}

/// A change to the ban records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanChange {
    /// A node was banned, again or for the first time.
    Banned(BanRecord),
    /// Its ban was lifted.
    Unbanned {
        /// The node.
        node: NodeId,
        /// When.
        at_ms: u64,
    },
    /// Its record is dropped, to keep within [`MAX_BAN_RECORDS`].
    Forgotten(NodeId),
}

impl Store {
    /// The owner's peer settings.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn peer_settings(&self) -> Result<PeerSettings, StoreError> {
        let share: i64 = self.lock().query_row(
            "SELECT grace_share_ppm FROM peer_settings WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        let settings = PeerSettings {
            grace_share_ppm: u32::try_from(share)
                .map_err(|_| StoreError::Corrupt("grace share"))?,
        };
        if !settings.is_valid() {
            return Err(StoreError::Corrupt("grace share"));
        }
        Ok(settings)
    }

    /// Every ban record, banned or lifted, by node.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn ban_records(&self) -> Result<Vec<BanRecord>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT node, reason, banned_at_ms, cost_ledger, cost_refused, cost_overdue, \
             cost_grace, unbanned_at_ms FROM peer_ban_records ORDER BY node LIMIT ?1",
        )?;
        let limit = i64::try_from(MAX_BAN_RECORDS).unwrap_or(i64::MAX);
        let rows = statement
            .query_map([limit], read_ban)?
            .map(|row| row?)
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(rows)
    }

    /// What a peer's refused Fulfills cost us since `since_ms` (D17, D18):
    /// the losses booked against its account, `peer`, into `losses`.
    /// Refusals our own receiver took back cost nothing and are left out.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn losses_since(
        &self,
        peer: AccountId,
        losses: AccountId,
        since_ms: u64,
    ) -> Result<u64, StoreError> {
        let total: i64 = self.lock().query_row(
            "SELECT coalesce(sum(amount), 0) FROM ledger_transfers \
             WHERE reference = 'refusal' AND credit = ?1 AND debit = ?2 \
               AND created_at_ms >= ?3",
            params![peer.0, losses.0, to_sql_time(since_ms)],
            |row| row.get(0),
        )?;
        u64::try_from(total).map_err(|_| StoreError::Corrupt("refused total"))
    }
}

fn read_ban(row: &Row<'_>) -> rusqlite::Result<Result<BanRecord, StoreError>> {
    let node: String = row.get(0)?;
    let reason: String = row.get(1)?;
    let banned_at: i64 = row.get(2)?;
    let ledger: Option<String> = row.get(3)?;
    let amounts: [Option<i64>; 3] = [row.get(4)?, row.get(5)?, row.get(6)?];
    let unbanned_at: Option<i64> = row.get(7)?;
    Ok((|| {
        let cost = match (ledger, amounts) {
            (Some(ledger), [Some(refused), Some(overdue), Some(grace)]) => {
                let amount =
                    |value: i64| u64::try_from(value).map_err(|_| StoreError::Corrupt("ban cost"));
                Some(PeerCost {
                    ledger: LedgerId::parse(&ledger).ok_or(StoreError::Corrupt("ban ledger"))?,
                    refused: amount(refused)?,
                    overdue: amount(overdue)?,
                    grace: amount(grace)?,
                })
            }
            (None, [None, None, None]) => None,
            _ => return Err(StoreError::Corrupt("ban cost")),
        };
        Ok(BanRecord {
            node: NodeId::parse(&node).map_err(|_| StoreError::Corrupt("node ID"))?,
            reason: BanReason::parse(&reason).ok_or(StoreError::Corrupt("ban reason"))?,
            banned_at_ms: from_sql_time(banned_at),
            cost,
            unbanned_at_ms: unbanned_at.map(from_sql_time),
        })
    })())
}

/// Writes a batch's peer settings and ban records.
pub(super) fn write_grace(
    transaction: &Transaction<'_>,
    settings: Option<PeerSettings>,
    bans: &[BanChange],
) -> Result<(), StoreError> {
    if let Some(settings) = settings {
        transaction.execute(
            "UPDATE peer_settings SET grace_share_ppm = ?1 WHERE id = 1",
            [settings.grace_share_ppm],
        )?;
    }
    for change in bans {
        match change {
            BanChange::Banned(record) => {
                let cost = record.cost;
                let amount =
                    |f: fn(&PeerCost) -> u64| cost.as_ref().map(|cost| to_sql_time(f(cost)));
                transaction.execute(
                    "INSERT INTO peer_ban_records (node, reason, banned_at_ms, cost_ledger, \
                       cost_refused, cost_overdue, cost_grace, unbanned_at_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL) \
                     ON CONFLICT (node) DO UPDATE SET reason = ?2, banned_at_ms = ?3, \
                       cost_ledger = ?4, cost_refused = ?5, cost_overdue = ?6, \
                       cost_grace = ?7, unbanned_at_ms = NULL",
                    params![
                        record.node.as_str(),
                        record.reason.as_str(),
                        to_sql_time(record.banned_at_ms),
                        cost.map(|cost| cost.ledger.as_str()),
                        amount(|cost| cost.refused),
                        amount(|cost| cost.overdue),
                        amount(|cost| cost.grace),
                    ],
                )?;
            }
            BanChange::Unbanned { node, at_ms } => {
                transaction.execute(
                    "UPDATE peer_ban_records SET unbanned_at_ms = max(?2, banned_at_ms) \
                     WHERE node = ?1",
                    params![node.as_str(), to_sql_time(*at_ms)],
                )?;
            }
            BanChange::Forgotten(node) => {
                transaction.execute(
                    "DELETE FROM peer_ban_records WHERE node = ?1",
                    [node.as_str()],
                )?;
            }
        }
    }
    Ok(())
}
