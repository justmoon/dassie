//! Swaps between members of an XRP hub and a dollar hub, as their owners
//! see them (spec/decisions/peer-swaps.md, spec/areas/rate-feed.md): what
//! a member holds of each asset in its hub and its owner book, its rates,
//! its swap settings and status, and the snapshots of its peers it holds.
//! For the tests of peer swaps and of the market maker on paused time.

use dassie_feed::Quote;
use dassie_identity::IdentityKey;
use dassie_node::rpc::proto::{self, request::Method, response};

use super::TestNode;
use super::hubs::{HubAsset, HubNet, Member, list_ledgers, until};
use super::rates::RatesServer;

/// A whole XRP or dollar in the books' units, which count at scale 9.
pub const UNIT: u64 = 1_000_000_000;

/// What the rate source says: two XRP to the dollar, the dev rate.
pub const XRP_PER_USD: &str = "2";

/// What `member`'s owner holds of `asset`, by its books: in the asset's
/// hub and in the owner book that funds it, between which the node moves
/// funds by itself (spec/areas/hubs.md#in-the-node). A move on its way
/// reads short; read once settling is over, it is whole.
pub async fn held(net: &HubNet, member: &Member, asset: HubAsset) -> u64 {
    let list = list_ledgers(&member.node).await;
    let owner = |id: &str| {
        list.ledgers
            .iter()
            .find(|ledger| ledger.id == id)
            .map_or(0, |ledger| {
                u64::try_from(ledger.owner_balance.max(0)).unwrap()
            })
    };
    owner(&net.hub(asset).ledger) + owner(&asset.owner_ledger(net.network))
}

/// Has `node` read its rates from `rates`, and waits until it has fresh
/// ones for XRP and dollars, which its snapshot prices its hubs' ledgers
/// at.
pub async fn rated(node: &TestNode, rates: &RatesServer) {
    let set = node
        .ipc()
        .await
        .call(Method::SetRateSources(proto::SetRateSourcesRequest {
            sources: vec![proto::RateSource {
                kind: "coinbase".into(),
                url: rates.url(),
            }],
        }))
        .await;
    assert!(set.is_ok(), "{set:?}");
    until("fresh rates for XRP and dollars", async || {
        let rates = list_ledgers(node).await.rates;
        ["XRP", "USD"].iter().all(|code| {
            rates
                .iter()
                .any(|rate| rate.asset_code == *code && !rate.stale)
        })
    })
    .await;
}

/// The node's swap settings.
pub async fn peer_settings(node: &TestNode) -> proto::PeerSettings {
    match node
        .ipc()
        .await
        .call(Method::GetPeerSettings(proto::GetPeerSettingsRequest {}))
        .await
    {
        Ok(response::Method::GetPeerSettings(reply)) => reply.settings.unwrap(),
        other => panic!("{other:?}"),
    }
}

/// Sets the node's spread, its other settings as they are.
pub async fn set_spread(node: &TestNode, fx_spread_ppm: u32) {
    let settings = proto::PeerSettings {
        fx_spread_ppm,
        ..peer_settings(node).await
    };
    let set = node
        .ipc()
        .await
        .call(Method::SetPeerSettings(proto::SetPeerSettingsRequest {
            settings: Some(settings),
        }))
        .await;
    assert!(set.is_ok(), "{set:?}");
}

/// What the node's swap service did and does now.
pub async fn swap_status(node: &TestNode) -> proto::GetSwapStatusResponse {
    match node
        .ipc()
        .await
        .call(Method::GetSwapStatus(proto::GetSwapStatusRequest {}))
        .await
    {
        Ok(response::Method::GetSwapStatus(status)) => status,
        other => panic!("{other:?}"),
    }
}

/// The snapshot of `of` that `node` holds, if it prices both hubs'
/// methods (`ListPeerSnapshots`).
pub async fn peer_snapshot(
    net: &HubNet,
    node: &TestNode,
    of: &IdentityKey,
) -> Option<proto::PeerSnapshot> {
    let list = match node
        .ipc()
        .await
        .call(Method::ListPeerSnapshots(
            proto::ListPeerSnapshotsRequest {},
        ))
        .await
    {
        Ok(response::Method::ListPeerSnapshots(list)) => list,
        other => panic!("{other:?}"),
    };
    let id = of.node_id().to_string();
    let snapshot = list
        .snapshots
        .into_iter()
        .find(|listed| listed.node == id)?;
    let both = net.hubs.iter().all(|hub| {
        snapshot
            .methods
            .iter()
            .any(|method| method.scheme == hub.scheme)
    });
    both.then_some(snapshot)
}

/// A quote of `snapshot`'s method `scheme`: its bid or its ask.
fn quote(snapshot: &proto::PeerSnapshot, scheme: &str, bid: bool) -> Option<Quote> {
    let method = snapshot.methods.iter().find(|m| m.scheme == scheme)?;
    let quote = Quote::from_bits(if bid { method.bid } else { method.ask })?;
    (!quote.is_zero()).then_some(quote)
}

/// What node A gains, in parts per million, by a vanishingly small swap
/// with node B that pays on `pay` and is paid back on `back`: B's bid on
/// `pay` over its ask on `back` (B's snapshot `theirs`), valued at A's own
/// quotes, its bid on `back` over its bid on `pay` (A's snapshot `ours`),
/// less one. A node swaps while this clears its costs
/// (spec/areas/connector.md#the-swap-service). `None` while a snapshot
/// does not price a side. Computed here from the quotes, apart from the
/// node's reader.
pub fn gain_ppm(
    ours: &proto::PeerSnapshot,
    theirs: &proto::PeerSnapshot,
    pay: &str,
    back: &str,
) -> Option<i128> {
    let up = [quote(theirs, pay, true)?, quote(ours, back, true)?];
    let down = [quote(theirs, back, false)?, quote(ours, pay, true)?];
    let mantissas =
        |quotes: [Quote; 2]| u128::from(quotes[0].mantissa()) * u128::from(quotes[1].mantissa());
    let shift = up.iter().map(|q| q.exponent()).sum::<i32>()
        - down.iter().map(|q| q.exponent()).sum::<i32>();
    let (mut numerator, mut denominator) = (mantissas(up) * 1_000_000, mantissas(down));
    if shift >= 0 {
        numerator <<= shift;
    } else {
        denominator <<= -shift;
    }
    Some(i128::try_from(numerator / denominator).unwrap() - 1_000_000)
}
