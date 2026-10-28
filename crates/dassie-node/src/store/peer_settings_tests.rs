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
        offer_swaps: false,
        swapping: true,
        swap_targets: dassie_connector::Targets::new(&[
            (dassie_ledger::LedgerId::StubUsd, 400_000),
            (dassie_ledger::LedgerId::XrplTestnetXrp, 600_000),
        ])
        .unwrap(),
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
    // Targets go as the owner takes them away.
    let none = PeerSettings {
        swap_targets: dassie_connector::Targets::NONE,
        ..settings
    };
    store
        .commit_ledger(&LedgerBatch {
            peer_settings: Some(none),
            ..LedgerBatch::default()
        })
        .unwrap();
    assert_eq!(store.peer_settings().unwrap(), none);
}

// A target names an asset, never a hub's ledger, which counts with the
// owner book that funds the hub.
#[test]
fn a_swap_target_names_an_asset() {
    let hub = dassie_connector::Targets::new(&[(dassie_ledger::LedgerId::HubXrplTestnetXrp, 1)]);
    let settings = PeerSettings {
        swap_targets: hub.unwrap(),
        ..PeerSettings::DEFAULT
    };
    assert!(!settings.is_valid());
    assert!(PeerSettings::DEFAULT.is_valid());
}
