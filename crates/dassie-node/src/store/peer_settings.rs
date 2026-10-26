//! The owner's settings for peers: the fee and spread on what passes from
//! one peer to another (spec/decisions/exchange-rates.md), and how far the
//! owner's inventory moves the rate it converts at
//! (spec/decisions/passive-rebalancing.md).

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
}

impl PeerSettings {
    /// What a new node starts with: transit is free, and converts by the
    /// curve.
    pub const DEFAULT: Self = Self {
        connector_fee_ppm: 0,
        fx_spread_ppm: 0,
        rebalance_ppm: dassie_connector::pricing::DEFAULT_REBALANCE_PPM,
    };

    /// Whether the settings can be stored.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.pricing().is_valid()
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

impl Store {
    /// The owner's peer settings.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn peer_settings(&self) -> Result<PeerSettings, StoreError> {
        let (fee, spread, rebalance): (i64, i64, i64) = self.lock().query_row_cached(
            "SELECT connector_fee_ppm, fx_spread_ppm, rebalance_ppm FROM peer_settings WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let ppm =
            |value: i64| u32::try_from(value).map_err(|_| StoreError::Corrupt("peer settings"));
        let settings = PeerSettings {
            connector_fee_ppm: ppm(fee)?,
            fx_spread_ppm: ppm(spread)?,
            rebalance_ppm: ppm(rebalance)?,
        };
        if !settings.is_valid() {
            return Err(StoreError::Corrupt("peer settings"));
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
            "UPDATE peer_settings SET connector_fee_ppm = ?1, fx_spread_ppm = ?2, rebalance_ppm = ?3 WHERE id = 1",
            [
                settings.connector_fee_ppm,
                settings.fx_spread_ppm,
                settings.rebalance_ppm,
            ],
        )?;
    }
    Ok(())
}
