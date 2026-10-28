//! The owner's removal of a stored scheme the node left out at start
//! (spec/areas/hubs.md#in-the-node): a hub scheme goes as any other such
//! scheme, its move between the hub and its owner book on its way or not,
//! since the owner cannot give that move up while the scheme does not run;
//! the database keeps the move's row, for the scheme added again.

use std::sync::Arc;

use dassie_ledger::Name;

use super::{NOW, Store, core_on};
use crate::store::{HubFundingRow, HubMoveRow, LedgerBatch};

/// A store of a node that ran the XRP hub's scheme, stored with a config
/// this version refuses, and keeps `row` of its moves.
fn stored_hub(row: &HubFundingRow) -> (Arc<Store>, Name) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let hub = Name::new(dassie_hub::member::SCHEME_ID).unwrap();
    store
        .commit_ledger(&LedgerBatch {
            at_ms: NOW,
            schemes_added: vec![(hub, "not a hub".into())],
            hub_funding: [(hub, row.clone())].into(),
            ..LedgerBatch::default()
        })
        .unwrap();
    (store, hub)
}

/// The schemes `store` keeps.
fn stored(store: &Store) -> Vec<Name> {
    store
        .settlement_schemes()
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

// A move of the hub on its way, as the database keeps it, does not keep
// the scheme left out at start from being removed: the owner could not
// give the move up while the scheme does not run. Its stored config goes,
// and the row stays, so the move goes on once the owner adds the scheme
// again.
#[test]
fn a_scheme_left_out_with_a_hub_move_on_its_way_is_removed_and_its_row_kept() {
    let row = HubFundingRow {
        paused_until_ms: 0,
        moving: Some(HubMoveRow {
            deposit: true,
            drops: 1_000_000,
            target: 1_000_000,
            payment: "deposit-1".into(),
            by_owner: true,
            sent_ms: NOW - 5_000,
            applied_ms: None,
        }),
        given_up: None,
    };
    let (store, hub) = stored_hub(&row);
    let mut core = core_on(Arc::clone(&store), 4);
    assert!(core.ledger_views().is_empty(), "the scheme ran");
    core.remove_scheme(hub.as_str()).unwrap();
    core.finish_step(&mut Vec::new());
    assert!(stored(&store).is_empty());
    assert_eq!(store.hub_funding(hub).unwrap().row, row);
}

// Without a move on its way, it goes the same: its stored config goes,
// and the row, which the database keeps, stays.
#[test]
fn a_scheme_left_out_with_no_hub_move_is_removed() {
    let row = HubFundingRow {
        paused_until_ms: NOW,
        moving: None,
        given_up: Some((true, 1_000_000)),
    };
    let (store, hub) = stored_hub(&row);
    let mut core = core_on(Arc::clone(&store), 4);
    core.remove_scheme(hub.as_str()).unwrap();
    core.finish_step(&mut Vec::new());
    assert!(stored(&store).is_empty());
    assert_eq!(store.hub_funding(hub).unwrap().row, row);
    assert!(core.remove_scheme(hub.as_str()).is_err());
}
