//! A member's copies of its hubs' payout bundles
//! (spec/areas/hubs.md#payout-bundles): every so often, the node asks each
//! validator of each hub it is a member of for the payout set it signed,
//! which it has only once the hub ended or stalled, reads it a page at a
//! time, checks it (`dassie_hub::member::Payouts`) and keeps the highest
//! of each validator's in the database, apart for each hub. With a
//! quorum's bundles of one set the owner can submit it, its own payout
//! among it, if the validators are gone ([`payout`]).

use std::sync::Arc;
use std::time::Duration;

use dassie_hub::escrow::PayoutBundle;
use dassie_hub::member::{Payouts, Taken};
use dassie_hub::service::wire::{Message, PayoutPage};
use dassie_hub::{HubId, Rules};
use dassie_oer::{Decode, Encode};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use tokio_util::sync::CancellationToken;

use super::driver::hex;
use super::{HubClient, HubExchangeError};
use crate::clock::clock_moved;
use crate::log::target;
use crate::node::Node;
use crate::store::StoreError;

/// How often each validator is asked for its payout set, by the node's
/// clock: one small request, answered with nothing while the hub runs.
pub const PAYOUT_POLL_MS: u64 = 60_000;

/// The most pages read from one validator in one round: a set of the most
/// accounts a hub's default terms allow takes one.
const MAX_PAGES_PER_ROUND: usize = 64;

/// The longest one request takes.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the keeper looks at the node's clock, unless it is moved by
/// hand.
const TICK: Duration = Duration::from_secs(1);

/// Asks the validators of each of the node's hubs for their payout sets
/// every [`PAYOUT_POLL_MS`] while its hub scheme runs, until `shutdown`.
/// What each hub's validators sign is checked and kept apart from the
/// others', by the hub's ID.
pub async fn run(node: Arc<Node>, shutdown: CancellationToken) {
    // One per hub the node is in: at most `dassie_peer::MAX_OWN_HUBS`, as
    // the hub schemes are.
    let mut kept: Vec<Payouts> = Vec::new();
    let mut next_ms = 0_u64;
    let mut moves = node.clock().subscribe();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(TICK) => {}
            () = clock_moved(&mut moves) => {}
        }
        let now = node.now_ms();
        if now < next_ms {
            continue;
        }
        next_ms = now.saturating_add(PAYOUT_POLL_MS);
        kept = following(&node, hub_rules(&node).await, kept);
        let client = HubClient::of(&node);
        // The hubs are asked at once too, so that one whose validators are
        // gone holds up no other hub's round.
        futures_util::future::join_all(
            kept.iter_mut()
                .map(|payouts| round(&node, &client, payouts, &shutdown)),
        )
        .await;
    }
}

/// The rules of each hub the node is a member of, while its scheme runs.
async fn hub_rules(node: &Node) -> Vec<Rules> {
    match node.engine() {
        Some(engine) => engine.hub_rules().await.unwrap_or_default(),
        None => Vec::new(),
    }
}

/// The payout bundles to follow, one set per hub of `rules`: those already
/// followed kept as they are, a hub joined since restored from the
/// database, and a hub left forgotten, its bundles kept in the database.
fn following(node: &Node, rules: Vec<Rules>, mut kept: Vec<Payouts>) -> Vec<Payouts> {
    assert!(
        rules.len() <= dassie_peer::MAX_OWN_HUBS,
        "a hub scheme per asset"
    );
    rules
        .into_iter()
        .map(|rules| {
            let id = rules.id();
            match kept.iter().position(|payouts| payouts.rules().id() == id) {
                Some(index) => kept.swap_remove(index),
                None => restored(node, rules),
            }
        })
        .collect()
}

/// What the database keeps of the hub's payout bundles, checked again.
fn restored(node: &Node, rules: Rules) -> Payouts {
    let rows = node
        .store_handle()
        .hub_payouts(rules.id().0.as_bytes())
        .unwrap_or_default();
    restored_from(rules, rows)
}

/// The hub's payout bundles in `rows`, as the database keeps them, checked
/// again.
fn restored_from(rules: Rules, rows: Vec<(u8, u64, Vec<u8>)>) -> Payouts {
    let mut payouts = Payouts::new(rules);
    for (index, _, bytes) in rows {
        let restored = PayoutBundle::decode_exact(&bytes)
            .ok()
            .and_then(|bundle| payouts.restore(usize::from(index), bundle).ok());
        if restored.is_none() {
            tracing::warn!(target: target::HUB, validator = index, "a kept payout bundle does not check: asking for it again");
        }
    }
    payouts
}

/// Asks each validator for its payout set, a page at a time, and keeps
/// what completes. The validators are asked at once, and each answer is
/// taken as it comes, so that one that is gone, as validators are when a
/// hub stalls, holds up no other for the time its connection takes to give
/// up.
async fn round(node: &Node, client: &HubClient, payouts: &mut Payouts, stop: &CancellationToken) {
    let rules = payouts.rules().clone();
    let hub = rules.id();
    let ask = |index: usize, request: Vec<u8>| {
        let acceptance = rules.validators.get(index);
        async move {
            let Some(acceptance) = acceptance else {
                return (index, Err(HubExchangeError::Unreachable));
            };
            let node_id = acceptance.node.node_id();
            let exchanged = client
                .exchange(
                    node_id,
                    &acceptance.endpoint,
                    &hub,
                    &request,
                    EXCHANGE_TIMEOUT,
                )
                .await;
            (index, exchanged)
        }
    };
    let mut asks = FuturesUnordered::new();
    for index in 0..rules.validators.len() {
        asks.push(ask(
            index,
            Message::Request(payouts.request(index)).to_vec(),
        ));
    }
    // Pages read of each validator's set: the first came with the ask above.
    let mut read = vec![1_usize; rules.validators.len()];
    loop {
        let answered = tokio::select! {
            () = stop.cancelled() => return,
            answered = asks.next() => answered,
        };
        let Some((index, exchanged)) = answered else {
            return;
        };
        let page = exchanged.ok().and_then(|messages| {
            messages
                .iter()
                .find_map(|message| match Message::decode_exact(message) {
                    Ok(Message::Payout(page)) => Some(page),
                    _ => None,
                })
        });
        let Some(page) = page else {
            continue;
        };
        if !take(node, &hub, index, payouts, page) {
            continue;
        }
        let Some(pages) = read.get_mut(index) else {
            continue;
        };
        if *pages < MAX_PAGES_PER_ROUND {
            *pages += 1;
            asks.push(ask(
                index,
                Message::Request(payouts.request(index)).to_vec(),
            ));
        }
    }
}

/// Takes a page of validator `index`'s payout set: whether there is more
/// of it to read.
fn take(node: &Node, hub: &HubId, index: usize, payouts: &mut Payouts, page: PayoutPage) -> bool {
    match payouts.take(index, page) {
        Ok(Taken::More(_)) => true,
        Ok(Taken::Kept(height)) => {
            keep(node, hub, index, payouts);
            tracing::info!(target: target::HUB, hub = %hex(hub.0.as_bytes()), validator = index, height, "kept a validator's payout bundle");
            false
        }
        Ok(Taken::Nothing) => false,
        Err(error) => {
            tracing::warn!(target: target::HUB, validator = index, ?error, "a validator's payout set does not check");
            false
        }
    }
}

/// Writes validator `index`'s bundle.
fn keep(node: &Node, hub: &HubId, index: usize, payouts: &Payouts) {
    let Some((_, bundle)) = payouts.kept().find(|(kept, _)| *kept == index) else {
        return;
    };
    let validator = u8::try_from(index).unwrap_or(u8::MAX);
    let written = node.store_handle().put_hub_payout(
        hub.0.as_bytes(),
        validator,
        bundle.height,
        node.now_ms(),
        &bundle.to_vec(),
    );
    if let Err(error) = written {
        tracing::error!(target: target::HUB, %error, "cannot keep a payout bundle");
    }
}

/// What the node holds of its hub's payout: each validator's bundle it
/// kept, and the set ready to submit once a quorum's sign the same.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutStatus {
    /// The hub.
    pub hub: HubId,
    /// Each validator's bundle kept: its place, and the height it pays.
    pub bundles: Vec<(usize, u64, usize)>,
    /// The quorum of the set.
    pub quorum: usize,
    /// Once a quorum's agree: the height, and the transactions to submit
    /// in order.
    pub ready: Option<(u64, Vec<Vec<u8>>)>,
}

/// Why [`payout`] has no payout to show.
#[derive(Debug)]
pub enum PayoutError {
    /// No hub scheme runs.
    NoHub,
    /// The node is in several hubs, and the call named none.
    Unnamed,
    /// The node is in no hub of that ID.
    NotFound,
    /// The database failed.
    Store(StoreError),
}

/// What the node holds of the payout of the hub `hub` names by its ID, or
/// of its only hub when it names none.
///
/// # Errors
///
/// No hub scheme runs, the node is in several hubs and `hub` names none,
/// or in none of that ID; or the database failed.
pub async fn payout(node: &Node, hub: Option<[u8; 32]>) -> Result<PayoutStatus, PayoutError> {
    let rules = chosen(hub_rules(node).await, hub)?;
    let hub = rules.id();
    let quorum = usize::try_from(dassie_hub::escrow::signer_quorum(&rules)).unwrap_or(usize::MAX);
    let rows = node
        .store_handle()
        .hub_payouts(hub.0.as_bytes())
        .map_err(PayoutError::Store)?;
    let payouts = restored_from(rules, rows);
    Ok(PayoutStatus {
        hub,
        bundles: payouts
            .kept()
            .map(|(index, bundle)| (index, bundle.height, bundle.entries.len()))
            .collect(),
        quorum,
        ready: payouts.assembled(),
    })
}

/// The rules of the hub `hub` names among `running`, the rules of each hub
/// the node is in: the only one when it names none.
fn chosen(running: Vec<Rules>, hub: Option<[u8; 32]>) -> Result<Rules, PayoutError> {
    match (hub, running.len()) {
        (_, 0) => Err(PayoutError::NoHub),
        (None, 1) => running.into_iter().next().ok_or(PayoutError::NoHub),
        (None, _) => Err(PayoutError::Unnamed),
        (Some(id), _) => running
            .into_iter()
            .find(|rules| *rules.id().0.as_bytes() == id)
            .ok_or(PayoutError::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use dassie_identity::NodeKey;

    use super::*;
    use crate::hub::config::tests::rules;

    /// A hub of one validator, sponsored by the key of `sponsor`'s bytes:
    /// each sponsor's hub has an ID of its own.
    fn hub(sponsor: u8) -> Rules {
        let validator = (NodeKey::from_bytes(&[1; 32]), "v.example:443".into());
        rules(&NodeKey::from_bytes(&[sponsor; 32]), &[validator])
    }

    // The owner names the hub whose payout to show by its ID, or none while
    // the node is in one hub only (proto/dassie/owner/v1/hub_founding.proto).
    #[test]
    fn a_payout_is_of_the_hub_named_or_the_only_one() {
        let (first, second) = (hub(100), hub(101));
        let id = |rules: &Rules| *rules.id().0.as_bytes();
        assert_ne!(id(&first), id(&second));
        assert!(matches!(chosen(Vec::new(), None), Err(PayoutError::NoHub)));
        assert!(matches!(
            chosen(Vec::new(), Some(id(&first))),
            Err(PayoutError::NoHub)
        ));
        let only = chosen(vec![first.clone()], None).unwrap();
        assert_eq!(id(&only), id(&first));
        let both = || vec![first.clone(), second.clone()];
        assert!(matches!(chosen(both(), None), Err(PayoutError::Unnamed)));
        assert_eq!(id(&chosen(both(), Some(id(&second))).unwrap()), id(&second));
        assert!(matches!(
            chosen(both(), Some([0; 32])),
            Err(PayoutError::NotFound)
        ));
        assert!(matches!(
            chosen(vec![first.clone()], Some(id(&second))),
            Err(PayoutError::NotFound)
        ));
    }
}
