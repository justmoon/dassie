//! Exchange rates (D7): where the node fetches them, the last it got, and
//! the currency the owner sees totals in.

use rusqlite::params;

use super::{Store, StoreError, from_sql_time, to_sql_time};
use crate::rates::{MAX_SOURCES, RateSource, SourceKind, currency};

/// A rate as kept: the currency, units of it per dollar times 10^15, and
/// when it was learned.
pub type StoredRate = (&'static str, u128, u64);

impl Store {
    /// The rate sources, in the owner's order.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn rate_sources(&self) -> Result<Vec<RateSource>, StoreError> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT kind, url FROM rate_sources ORDER BY position")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(kind, url)| {
                let kind = SourceKind::parse(&kind).ok_or(StoreError::Corrupt("rate source"))?;
                let source = RateSource { kind, url };
                match source.problem() {
                    Some(_) => Err(StoreError::Corrupt("rate source")),
                    None => Ok(source),
                }
            })
            .collect()
    }

    /// Replaces the rate sources.
    ///
    /// # Errors
    ///
    /// The database failed, or refused them: there are more than
    /// [`MAX_SOURCES`], or one is invalid.
    pub fn set_rate_sources(&self, sources: &[RateSource]) -> Result<(), StoreError> {
        if sources.len() > MAX_SOURCES || sources.iter().any(|s| s.problem().is_some()) {
            return Err(StoreError::Corrupt("rate sources"));
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM rate_sources", [])?;
        for (position, source) in sources.iter().enumerate() {
            transaction.execute(
                "INSERT INTO rate_sources (position, kind, url) VALUES (?1, ?2, ?3)",
                params![
                    i64::try_from(position).unwrap_or(i64::MAX),
                    source.kind.as_str(),
                    source.url,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The last rates fetched. Rates of currencies the node no longer keeps
    /// are left out.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn exchange_rates(&self) -> Result<Vec<StoredRate>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT asset, rate, as_of_ms FROM exchange_rates ORDER BY asset")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut rates = Vec::with_capacity(rows.len());
        for (asset, rate, as_of) in rows {
            let rate = rate
                .parse::<u128>()
                .ok()
                .filter(|rate| *rate > 0)
                .ok_or(StoreError::Corrupt("exchange rate"))?;
            if let Some(code) = currency(&asset) {
                rates.push((code, rate, from_sql_time(as_of)));
            }
        }
        Ok(rates)
    }

    /// Keeps rates just fetched, replacing what was kept for their
    /// currencies.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn save_exchange_rates(&self, rates: &[StoredRate]) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        for (code, rate, as_of_ms) in rates {
            transaction.execute(
                "INSERT INTO exchange_rates (asset, rate, as_of_ms) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (asset) DO UPDATE SET rate = excluded.rate, \
                 as_of_ms = excluded.as_of_ms",
                params![code, rate.to_string(), to_sql_time(*as_of_ms)],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The currency the owner sees totals in, if one was chosen.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn display_currency(&self) -> Result<Option<&'static str>, StoreError> {
        let stored: Option<String> = self.lock().query_row(
            "SELECT display_currency FROM payment_settings WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(stored.as_deref().and_then(currency))
    }

    /// Chooses the currency the owner sees totals in, or none.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn set_display_currency(&self, code: Option<&'static str>) -> Result<(), StoreError> {
        self.lock().execute(
            "UPDATE payment_settings SET display_currency = ?1 WHERE id = 1",
            [code],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use rusqlite::Connection;

    use super::super::schema::{self, APPLICATION_ID};
    use super::*;

    #[test]
    fn sources_start_with_all_three_and_change() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.rate_sources().unwrap(), RateSource::defaults());
        let dev = vec![RateSource {
            kind: SourceKind::Coinbase,
            url: Some("https://localhost:10100/rates.json".into()),
        }];
        store.set_rate_sources(&dev).unwrap();
        assert_eq!(store.rate_sources().unwrap(), dev);
        let too_many = vec![RateSource::of(SourceKind::Kraken); MAX_SOURCES + 1];
        assert!(store.set_rate_sources(&too_many).is_err());
        let invalid = vec![RateSource {
            kind: SourceKind::Kraken,
            url: Some("https://localhost".into()),
        }];
        assert!(store.set_rate_sources(&invalid).is_err());
        assert_eq!(store.rate_sources().unwrap(), dev);
        store.set_rate_sources(&[]).unwrap();
        assert_eq!(store.rate_sources().unwrap(), []);
    }

    #[test]
    fn rates_are_kept_exactly() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.exchange_rates().unwrap(), []);
        let huge = u128::MAX / 3;
        store
            .save_exchange_rates(&[("XRP", 2, 10), ("JPY", huge, 20)])
            .unwrap();
        store.save_exchange_rates(&[("XRP", 3, 30)]).unwrap();
        assert_eq!(
            store.exchange_rates().unwrap(),
            [("JPY", huge, 20), ("XRP", 3, 30)]
        );
    }

    #[test]
    fn the_display_currency_is_kept() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.display_currency().unwrap(), None);
        store.set_display_currency(Some("EUR")).unwrap();
        assert_eq!(store.display_currency().unwrap(), Some("EUR"));
        store.set_display_currency(None).unwrap();
        assert_eq!(store.display_currency().unwrap(), None);
    }

    // Migration 12 takes a Fulfill owed without a third transfer, which
    // migration 15 lets go, and starts with no fee and the three sources.
    #[test]
    fn migration_12_starts_with_no_fee_and_the_sources() {
        let temp = tempfile::tempdir().unwrap();
        let mut connection =
            Connection::open(temp.path().join(super::super::DATABASE_FILE)).unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        schema::migrate_until(&mut connection, 11).unwrap();
        let peer = dassie_identity::NodeKey::from_bytes(&[9; 32]).node_id();
        connection
            .execute_batch(&format!(
                "INSERT INTO ledger_accounts VALUES (0, 'stub+usd:equity/owner', 0, 0); \
                 INSERT INTO ledger_accounts VALUES (1, 'stub+usd:equity/suspense', 0, 0); \
                 INSERT INTO owed_fulfills (packet_epoch, packet_sequence, peer, request_id, \
                   fulfill, compensation, debit_1, credit_1, amount_1, expires_at_ms, \
                   created_at_ms) VALUES (1, 2, '{peer}', 3, x'01', 'loss', 0, 1, 5, 9, 8);"
            ))
            .unwrap();
        drop(connection);
        let store = Store::open(temp.path()).unwrap();
        let settings = store.peer_settings().unwrap();
        assert_eq!((settings.connector_fee_ppm, settings.fx_spread_ppm), (0, 0));
        assert_eq!(store.rate_sources().unwrap(), RateSource::defaults());
    }
}
