#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::store::LedgerBatch;

#[test]
fn the_peer_settings_start_at_the_default_and_change() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.peer_settings().unwrap(), PeerSettings::DEFAULT);
    let settings = PeerSettings {
        connector_fee_ppm: 100_000,
        fx_spread_ppm: 1,
        rebalance_ppm: 0,
    };
    store
        .commit_ledger(&LedgerBatch {
            peer_settings: Some(settings),
            ..LedgerBatch::default()
        })
        .unwrap();
    assert_eq!(store.peer_settings().unwrap(), settings);
    // The table refuses what the code would.
    let refused = store.commit_ledger(&LedgerBatch {
        peer_settings: Some(PeerSettings {
            connector_fee_ppm: 100_001,
            ..PeerSettings::DEFAULT
        }),
        ..LedgerBatch::default()
    });
    assert!(refused.is_err());
    let refused = store.commit_ledger(&LedgerBatch {
        peer_settings: Some(PeerSettings {
            rebalance_ppm: 100_001,
            ..PeerSettings::DEFAULT
        }),
        ..LedgerBatch::default()
    });
    assert!(refused.is_err());
    assert_eq!(store.peer_settings().unwrap(), settings);
}
