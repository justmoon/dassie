//! The owner's settings for peers: the fee and spread on what passes from
//! one peer to another (spec/decisions/exchange-rates.md), how far the
//! owner's inventory moves the rate it converts at
//! (spec/decisions/passive-rebalancing.md), whether the node offers swaps
//! to its peers and swaps with them, and the targets a periphery node's own
//! swaps aim for (spec/decisions/peer-swaps.md).

use dassie_connector::Targets;
use dassie_connector::pricing::MAX_PRICED_LEDGERS;
use dassie_ledger::LedgerId;
use rusqlite::Transaction;

use super::{Cached, Store, StoreError};

/// The peer settings the owner chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerSettings {
    /// Our fee on what passes from one peer to another, in parts per
    /// million of what comes in (spec/decisions/exchange-rates.md).
    pub connector_fee_ppm: u32,
    /// Our spread on what such a packet converts, in parts per million.
    pub fx_spread_ppm: u32,
    /// `k`, the curve's reach: how far a skewed inventory moves the rate
    /// such a packet converts at, in parts per million
    /// (spec/decisions/passive-rebalancing.md).
    pub rebalance_ppm: u32,
    /// Whether the node offers swaps to its peers: it takes their
    /// subscriptions to its snapshots (spec/decisions/peer-swaps.md). A
    /// periphery node offers none, whatever this says.
    pub offer_swaps: bool,
    /// Whether the node swaps with its peers: it subscribes to the
    /// snapshots of those it has methods on two ledgers with, and a core
    /// node swaps by itself toward its curve's targets
    /// (spec/decisions/peer-swaps.md).
    pub swapping: bool,
    /// What a periphery node's own swaps aim for, per asset, each named by
    /// the ledger its holding counts toward: none by default, and then it
    /// swaps only when its owner asks. A core node swaps toward its curve's
    /// targets, whatever this says.
    pub swap_targets: Targets,
}

impl PeerSettings {
    /// What a new node starts with: transit is free, converts by the
    /// curve, and the node offers swaps and swaps, as a core node does by
    /// default, with no targets of the owner's
    /// (spec/decisions/peer-swaps.md).
    pub const DEFAULT: Self = Self {
        connector_fee_ppm: 0,
        fx_spread_ppm: 0,
        rebalance_ppm: dassie_connector::pricing::DEFAULT_REBALANCE_PPM,
        offer_swaps: true,
        swapping: true,
        swap_targets: Targets::NONE,
    };

    /// Whether the settings can be stored: the charges and `k` within
    /// their bounds, and each target named by an asset's ledger, the one a
    /// holding counts toward, never a settlement hub's.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.pricing().is_valid() && self.swap_targets.iter().all(|(asset, _)| is_asset(asset))
    }

    /// What transit is charged, and how it is priced.
    #[must_use]
    pub const fn pricing(&self) -> dassie_connector::Pricing {
        dassie_connector::Pricing {
            charges: dassie_connector::Charges {
                fee_ppm: self.connector_fee_ppm,
                spread_ppm: self.fx_spread_ppm,
            },
            rebalance_ppm: self.rebalance_ppm,
        }
    }
}

/// Whether `ledger` names an asset: what a holding counts toward, which a
/// settlement hub's ledger never is, since it counts with the owner book
/// that funds the hub (spec/areas/connector.md#inventory-pricing).
#[must_use]
pub fn is_asset(ledger: LedgerId) -> bool {
    crate::engine::core::owner_book_of(ledger).is_none()
}

impl Store {
    /// The owner's peer settings.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn peer_settings(&self) -> Result<PeerSettings, StoreError> {
        let connection = self.lock();
        let (fee, spread, rebalance, offer_swaps, swapping): (i64, i64, i64, i64, i64) = connection
            .query_row_cached(
                "SELECT connector_fee_ppm, fx_spread_ppm, rebalance_ppm, offer_swaps, swapping
                 FROM peer_settings WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
        let corrupt = || StoreError::Corrupt("peer settings");
        let ppm = |value: i64| u32::try_from(value).map_err(|_| corrupt());
        let flag = |value: i64| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(corrupt()),
        };
        let mut statement = connection
            .prepare_cached("SELECT asset, target_ppm FROM swap_targets ORDER BY asset")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut targets = Vec::new();
        for row in rows {
            let (asset, target) = row?;
            if targets.len() >= MAX_PRICED_LEDGERS {
                return Err(corrupt());
            }
            let asset = LedgerId::parse(&asset).ok_or_else(corrupt)?;
            targets.push((asset, ppm(target)?));
        }
        let settings = PeerSettings {
            connector_fee_ppm: ppm(fee)?,
            fx_spread_ppm: ppm(spread)?,
            rebalance_ppm: ppm(rebalance)?,
            offer_swaps: flag(offer_swaps)?,
            swapping: flag(swapping)?,
            swap_targets: Targets::new(&targets).ok_or_else(corrupt)?,
        };
        if !settings.is_valid() {
            return Err(corrupt());
        }
        Ok(settings)
    }
}

/// Writes a batch's peer settings.
pub(super) fn write_peer_settings(
    transaction: &Transaction<'_>,
    settings: Option<PeerSettings>,
) -> Result<(), StoreError> {
    if let Some(settings) = settings {
        transaction.execute_cached(
            "UPDATE peer_settings SET connector_fee_ppm = ?1, fx_spread_ppm = ?2, rebalance_ppm = ?3,
               offer_swaps = ?4, swapping = ?5 WHERE id = 1",
            [
                settings.connector_fee_ppm,
                settings.fx_spread_ppm,
                settings.rebalance_ppm,
                u32::from(settings.offer_swaps),
                u32::from(settings.swapping),
            ],
        )?;
        transaction.execute_cached("DELETE FROM swap_targets", [])?;
        for (asset, target) in settings.swap_targets.iter() {
            transaction.execute_cached(
                "INSERT INTO swap_targets (asset, target_ppm) VALUES (?1, ?2)",
                rusqlite::params![asset.as_str(), target],
            )?;
        }
    }
    Ok(())
}
