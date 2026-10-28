//! What the tests and the vector generator share: the fixed key and the
//! snapshots they sign.

#![allow(dead_code, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use dassie_identity::NodeKey;

/// The seed of the key the vectors are signed with: the bytes 0 to 31.
pub const SEED: [u8; 32] = {
    let mut seed = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        seed[index] = index as u8;
        index += 1;
    }
    seed
};

/// The key the vectors are signed with.
pub fn key() -> NodeKey {
    NodeKey::from_bytes(&SEED)
}

/// Another key, for a node that is not the one subscribed to.
pub fn other_key() -> NodeKey {
    NodeKey::from_bytes(&[0xa5; 32])
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Bytes from hex.
pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap_or_default())
        .collect()
}

/// The drops of the test snapshots' grid, in basis points.
pub const GRID_BP: [u16; dassie_feed::LEVELS] = [10, 25, 50, 100, 200, 400, 800, 1600];

/// A snapshot at fixed times, with the test grid, on these
/// methods, sorted by tag.
pub fn snapshot_of(mut methods: Vec<dassie_feed::MethodQuotes>) -> dassie_feed::Snapshot {
    methods.sort_by_key(|method| method.tag);
    dassie_feed::Snapshot {
        sequence_ms: 1_790_000_000_000,
        valid_until_ms: 1_790_000_090_000,
        grid_bp: GRID_BP,
        methods,
    }
}

/// A heartbeat of a node that quotes nothing.
pub fn empty_snapshot() -> dassie_feed::Snapshot {
    snapshot_of(vec![])
}

/// The quotes of a method whose ladders hold `depth` at the first drop and
/// double at each later one, both sides open.
pub fn open_method(
    scheme_id: &str,
    bid: dassie_feed::Quote,
    ask: dassie_feed::Quote,
    depth: u64,
) -> dassie_feed::MethodQuotes {
    let ladder = dassie_feed::Ladder::floor(core::array::from_fn(|level| depth << level));
    dassie_feed::MethodQuotes {
        tag: dassie_feed::method_tag(scheme_id),
        bid,
        ask,
        bid_ladder: ladder,
        ask_ladder: ladder,
    }
}

/// A node on the testnet's DSUSD hub and its XRP hub, in a virtual unit of
/// a millionth of a dollar: a DSUSD millionth bids 0.999 and asks 1.001, a
/// drop of XRP bids 2.49 and asks 2.51.
pub fn two_method_snapshot() -> dassie_feed::Snapshot {
    use dassie_feed::Quote;
    snapshot_of(vec![
        open_method(
            "hub-xrpl-testnet-dsusd",
            Quote::floor(999, 1_000),
            Quote::ceil(1_001, 1_000),
            40_000_000,
        ),
        open_method(
            "hub-xrpl-testnet-xrp",
            Quote::floor(249, 100),
            Quote::ceil(251, 100),
            15_000_000,
        ),
    ])
}

/// As [`two_method_snapshot`], but the node gives out no DSUSD: its ask
/// there is closed, a dollar token off its peg.
pub fn closed_side_snapshot() -> dassie_feed::Snapshot {
    let mut snapshot = two_method_snapshot();
    let dsusd = dassie_feed::method_tag("hub-xrpl-testnet-dsusd");
    for method in &mut snapshot.methods {
        if method.tag == dsusd {
            method.ask = dassie_feed::Quote::ZERO;
            method.ask_ladder = dassie_feed::Ladder::ZERO;
        }
    }
    snapshot
}

/// The largest snapshot: every method, every quote and ladder at its
/// largest, every field at its bound.
pub fn largest_snapshot() -> dassie_feed::Snapshot {
    let largest_quote = dassie_feed::Quote::from_bits(u32::MAX).unwrap();
    let methods = (0..dassie_feed::MAX_SNAPSHOT_METHODS)
        .map(|index| {
            let mut method = open_method(
                &format!("scheme-{index}"),
                largest_quote,
                largest_quote,
                u64::MAX,
            );
            method.bid_ladder = dassie_feed::Ladder::floor([u64::MAX; dassie_feed::LEVELS]);
            method.ask_ladder = method.bid_ladder;
            method
        })
        .collect();
    let mut snapshot = snapshot_of(methods);
    snapshot.sequence_ms = u64::MAX - 1;
    snapshot.valid_until_ms = u64::MAX;
    snapshot.grid_bp = [9_992, 9_993, 9_994, 9_995, 9_996, 9_997, 9_998, 9_999];
    snapshot
}
