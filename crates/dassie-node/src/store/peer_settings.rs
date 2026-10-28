//! The owner's settings for peers: the fee and spread on what passes from
//! one peer to another (spec/decisions/exchange-rates.md), how far the
//! owner's inventory moves the rate it converts at
//! (spec/decisions/passive-rebalancing.md), and whether the node offers
//! swaps to its peers and swaps with them (spec/decisions/peer-swaps.md).

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
    /// snapshots of those it has methods on two ledgers with
    /// (spec/decisions/peer-swaps.md).
    pub swapping: bool,
}

impl PeerSettings {
    /// What a new node starts with: transit is free, converts by the
    /// curve, and the node offers swaps and swaps, as a core node does by
    /// default (spec/decisions/peer-swaps.md).
    pub const DEFAULT: Self = Self {
        connector_fee_ppm: 0,
        fx_spread_ppm: 0,
        rebalance_ppm: dassie_connector::pricing::DEFAULT_REBALANCE_PPM,
        offer_swaps: true,
        swapping: true,
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
        let (fee, spread, rebalance, offer_swaps, swapping): (i64, i64, i64, i64, i64) =
            self.lock().query_row_cached(
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
        let ppm =
            |value: i64| u32::try_from(value).map_err(|_| StoreError::Corrupt("peer settings"));
        let flag = |value: i64| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StoreError::Corrupt("peer settings")),
        };
        let settings = PeerSettings {
            connector_fee_ppm: ppm(fee)?,
            fx_spread_ppm: ppm(spread)?,
            rebalance_ppm: ppm(rebalance)?,
            offer_swaps: flag(offer_swaps)?,
            swapping: flag(swapping)?,
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
    }
    Ok(())
}
