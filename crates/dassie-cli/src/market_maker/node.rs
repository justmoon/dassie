//! The market maker's one node, over its IPC socket: what it holds of each
//! asset, the snapshots of its peers it reads, and a swap with one of them
//! (spec/areas/rate-feed.md#market-maker).
//!
//! The node is a member of the settlement hubs of both assets, and holds
//! each in two places: its account in the asset's hub, on the hub's
//! ledger, which its swaps pay from and are paid back on, and the owner
//! book of the asset on its XRP Ledger account, which funds the hub and
//! which its rebalances spend from. The node moves funds between the two
//! by itself, between the hub's floor and ceiling
//! (spec/areas/hubs.md#in-the-node), since its owner keeps what it receives
//! on the own account, which the hub's funding sweeps beyond the ceiling
//! into the book (`dassie setup --keep-funds account`,
//! spec/areas/payments.md#where-received-money-is-kept), so what it holds
//! is both: a move between them counts where the ledger it leaves still
//! has it, and nowhere while it is on neither.
//!
//! A trade is the owner's swap (`SwapWithPeer`): the node prices it by the
//! target's snapshot, as its swap service prices its own, and sends each
//! packet only while what the snapshot promises it clears the rate the
//! market maker names, so nothing comes back below it
//! (spec/areas/connector.md#the-swap-service). Every call is bounded in
//! time, and a connection that fails is made again at the next call.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dassie_feed::{LEVELS, Ladder, MethodQuotes, Quote, Snapshot, TAG_BYTES};
use dassie_identity::NodeId;
use dassie_node::rpc::proto::{self, request::Method, response};

use super::config::Config;
use super::strategy::{Holding, Holdings, Side};
use crate::client::Client;

/// How long one call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a swap may take: the node sends for a minute at most, then
/// waits for what it has in flight, each packet as long as a STREAM packet
/// (`SwapWithPeer`).
const SWAP_TIMEOUT: Duration = Duration::from_secs(180);
/// The scale both assets' books count at.
pub const LEDGER_SCALE: u32 = 9;

/// The node.
pub struct Node {
    /// Its two ledgers of XRP: the hub's, and the owner book.
    xrp: [String; 2],
    /// Its two ledgers of the token.
    token: [String; 2],
    /// The settlement scheme over each asset's hub, as its hub's ledger
    /// last named it; empty until read.
    hub_schemes: [String; 2],
    socket: PathBuf,
    client: Option<Client>,
}

/// What the node says of one asset: `ListLedgers` of its hub's ledger and
/// its owner book.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerState {
    /// The asset's code, as both ledgers name it.
    pub asset_code: String,
    /// What the node holds of it.
    pub holding: Holding,
    /// The owner book's XRPL account's address.
    pub deposit_address: String,
    /// Who issues what the owner book holds, for a token; empty for XRP.
    pub issuer: String,
}

/// What the node's ledgers of one asset say, as [`holding`] reads them:
/// the hub's ledger and the owner book, each from `ListLedgers`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Views {
    /// The owner's funds on the hub's ledger.
    pub hub_owner: i64,
    /// The owner's funds in the owner book.
    pub book_owner: i64,
    /// Of those, what is held for payments on their way out of the book,
    /// such as a deposit into the hub.
    pub book_pending: u64,
    /// What the book's account holds, as the books have it, less what the
    /// ledger holds back.
    pub book_on_ledger: u64,
}

/// A target's snapshot, as the node holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSnapshot {
    /// The target.
    pub node: NodeId,
    /// Its snapshot.
    pub snapshot: Arc<Snapshot>,
}

/// What a swap the node sent for the market maker did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapDone {
    /// What the target took of what the node paid.
    pub paid: u64,
    /// What came back, of the other asset.
    pub received: u64,
    /// Its packets fulfilled and rejected.
    pub fulfilled: u64,
    pub rejected: u64,
    /// Why it ended, as the node names it.
    pub ended: proto::SwapEnd,
}

impl Node {
    /// The node `config` names; it connects at the first call.
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let both = |side| {
            let ledgers = config.ledgers(side);
            [ledgers.ledger.clone(), ledgers.owner_book.clone()]
        };
        Self {
            xrp: both(Side::Xrp),
            token: both(Side::Token),
            hub_schemes: [String::new(), String::new()],
            socket: config.socket.clone(),
            client: None,
        }
    }

    /// The socket.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Calls `method`, within [`CALL_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// The node cannot be reached, does not answer in time, or refuses.
    pub async fn call(&mut self, method: Method) -> Result<response::Method, String> {
        self.call_within(CALL_TIMEOUT, method).await
    }

    async fn call_within(
        &mut self,
        timeout: Duration,
        method: Method,
    ) -> Result<response::Method, String> {
        let outcome = tokio::time::timeout(timeout, async {
            let client = self.connected().await?;
            client.call(method).await.map_err(|e| (e.code, e.message))
        })
        .await
        .unwrap_or_else(|_| Err((None, "the node did not answer in time".into())));
        self.settle(outcome)
    }

    async fn connected(&mut self) -> Result<&mut Client, (Option<proto::ErrorCode>, String)> {
        if self.client.is_none() {
            let client = Client::connect_to(&self.socket)
                .await
                .map_err(|e| (None, e.message))?;
            self.client = Some(client);
        }
        self.client
            .as_mut()
            .ok_or_else(|| (None, "not connected".into()))
    }

    /// The outcome of a call: a connection that failed, or a call that ran
    /// out of time on it, rather than a node that refused, is dropped.
    fn settle<T>(
        &mut self,
        outcome: Result<T, (Option<proto::ErrorCode>, String)>,
    ) -> Result<T, String> {
        outcome.map_err(|(code, message)| {
            if code.is_none() {
                self.client = None;
            }
            format!("the node: {message}")
        })
    }

    /// Whether the node reads its peers' snapshots: its owner lets it swap
    /// (spec/areas/network.md#snapshot-subscriptions).
    ///
    /// # Errors
    ///
    /// The call failed.
    pub async fn swapping(&mut self) -> Result<bool, String> {
        let response::Method::GetPeerSettings(reply) = self
            .call(Method::GetPeerSettings(proto::GetPeerSettingsRequest {}))
            .await?
        else {
            return Err("GetPeerSettings answered out of turn".into());
        };
        Ok(reply.settings.is_some_and(|settings| settings.swapping))
    }

    /// What the node holds of each asset, in its hub and its owner book.
    ///
    /// # Errors
    ///
    /// A call failed, the node does not run a ledger, or two of one asset
    /// count another asset or at another scale.
    pub async fn ledger_states(&mut self) -> Result<[LedgerState; 2], String> {
        let response::Method::ListLedgers(list) = self
            .call(Method::ListLedgers(proto::ListLedgersRequest {}))
            .await?
        else {
            return Err("ListLedgers answered out of turn".into());
        };
        let xrp = ledger_state(&list, &self.xrp)?;
        let token = ledger_state(&list, &self.token)?;
        self.hub_schemes = [xrp.1, token.1];
        Ok([xrp.0, token.0])
    }

    /// What the node holds of each asset.
    ///
    /// # Errors
    ///
    /// As [`Self::ledger_states`].
    pub async fn holdings(&mut self) -> Result<Holdings, String> {
        let [xrp, token] = self.ledger_states().await?;
        Ok(Holdings {
            xrp: xrp.holding,
            token: token.holding,
        })
    }

    /// What the node's own move between the hub of `side`'s asset and its
    /// owner book has on its way, in its ledgers' units, as `GetHubStatus`
    /// says: 0 with none. While it is on its way, the holding may read
    /// short of it ([`holding`]). The hub is the one whose scheme
    /// [`Self::ledger_states`] last read; none read yet is none on its way.
    ///
    /// # Errors
    ///
    /// The call failed.
    pub async fn moving(&mut self, side: Side) -> Result<u64, String> {
        let response::Method::GetHubStatus(status) = self
            .call(Method::GetHubStatus(proto::GetHubStatusRequest {}))
            .await?
        else {
            return Err("GetHubStatus answered out of turn".into());
        };
        let scheme = match side {
            Side::Xrp => &self.hub_schemes[0],
            Side::Token => &self.hub_schemes[1],
        };
        Ok(status
            .hubs
            .iter()
            .filter(|hub| !scheme.is_empty() && hub.scheme == *scheme)
            .filter_map(|hub| hub.moving.as_ref())
            .fold(0, |sum, moving| sum.saturating_add(moving.amount)))
    }

    /// The snapshots of its peers the node holds, as `ListPeerSnapshots`
    /// gives them, by node ID: those that do not read as snapshots are left
    /// out.
    ///
    /// # Errors
    ///
    /// The call failed.
    pub async fn peer_snapshots(&mut self) -> Result<Vec<PeerSnapshot>, String> {
        let response::Method::ListPeerSnapshots(list) = self
            .call(Method::ListPeerSnapshots(
                proto::ListPeerSnapshotsRequest {},
            ))
            .await?
        else {
            return Err("ListPeerSnapshots answered out of turn".into());
        };
        let mut snapshots: Vec<PeerSnapshot> = list
            .snapshots
            .iter()
            .filter_map(|listed| {
                let node = NodeId::parse(&listed.node).ok()?;
                let snapshot = snapshot(listed)?;
                Some(PeerSnapshot {
                    node,
                    snapshot: Arc::new(snapshot),
                })
            })
            .collect();
        snapshots.sort_by_key(|listed| listed.node);
        snapshots.dedup_by_key(|listed| listed.node);
        Ok(snapshots)
    }

    /// Swaps with `target` for the owner, from `pay_on` to `back_on`, up
    /// to `amount`, for no less than `rate` per unit, and waits until it
    /// is done.
    ///
    /// # Errors
    ///
    /// The node refused it, or did not answer in time: it still finishes
    /// what it has in flight.
    pub async fn swap(
        &mut self,
        target: NodeId,
        pay_on: &str,
        back_on: &str,
        amount: u64,
        rate: (u64, u64),
    ) -> Result<SwapDone, String> {
        let (rate_numerator, rate_denominator) = rate;
        assert!(rate_numerator > 0 && rate_denominator > 0, "a rate");
        let request = proto::SwapWithPeerRequest {
            node: target.to_string(),
            pay_on: pay_on.to_owned(),
            back_on: back_on.to_owned(),
            amount,
            rate_numerator,
            rate_denominator,
        };
        let response::Method::SwapWithPeer(done) = self
            .call_within(SWAP_TIMEOUT, Method::SwapWithPeer(request))
            .await?
        else {
            return Err("SwapWithPeer answered out of turn".into());
        };
        Ok(SwapDone {
            paid: done.paid,
            received: done.received,
            fulfilled: done.fulfilled,
            rejected: done.rejected,
            ended: done.ended(),
        })
    }
}

/// The state of the asset whose hub's ledger and owner book are
/// `[hub, book]` in `list`, and the scheme over the hub.
fn ledger_state(
    list: &proto::ListLedgersResponse,
    [hub, book]: &[String; 2],
) -> Result<(LedgerState, String), String> {
    let find = |id: &str| {
        list.ledgers
            .iter()
            .find(|ledger| ledger.id == id)
            .ok_or_else(|| format!("the node runs no {id}"))
    };
    let (hub, book) = (find(hub)?, find(book)?);
    for ledger in [hub, book] {
        if ledger.asset_scale != LEDGER_SCALE {
            return Err(format!(
                "{} counts at scale {}",
                ledger.id, ledger.asset_scale
            ));
        }
    }
    // A hub's ledger reports no issuer today; one that does must name the
    // book's.
    let issuers_differ = !hub.issuer.is_empty() && hub.issuer != book.issuer;
    if hub.asset_code != book.asset_code || issuers_differ {
        return Err(format!("{} and {} hold different assets", hub.id, book.id));
    }
    let views = Views {
        hub_owner: hub.owner_balance,
        book_owner: book.owner_balance,
        book_pending: book.owner_pending,
        book_on_ledger: book.on_ledger_balance,
    };
    let state = LedgerState {
        asset_code: book.asset_code.clone(),
        holding: holding(&views),
        deposit_address: book.deposit_address.clone(),
        issuer: book.issuer.clone(),
    };
    Ok((state, hub.settlement_scheme.clone()))
}

/// The snapshot `listed` says, as `dassie-feed` reads one: `None` unless
/// every quote and ladder is one a snapshot carries, so that what is
/// priced from it is what the node read
/// (spec/areas/rate-feed.md#numbers).
#[must_use]
pub fn snapshot(listed: &proto::PeerSnapshot) -> Option<Snapshot> {
    let grid_bp: Vec<u16> = listed
        .grid_bp
        .iter()
        .map(|&drop| u16::try_from(drop).ok())
        .collect::<Option<_>>()?;
    let grid_bp: [u16; LEVELS] = grid_bp.try_into().ok()?;
    let ladder = |amounts: &[u64]| {
        let amounts: [u64; LEVELS] = amounts.try_into().ok()?;
        let ladder = Ladder::floor(amounts);
        (ladder.amounts() == amounts).then_some(ladder)
    };
    let methods = listed
        .methods
        .iter()
        .map(|method| {
            let tag: [u8; TAG_BYTES] = method.tag.as_slice().try_into().ok()?;
            Some(MethodQuotes {
                tag,
                bid: Quote::from_bits(method.bid)?,
                ask: Quote::from_bits(method.ask)?,
                bid_ladder: ladder(&method.bid_ladder)?,
                ask_ladder: ladder(&method.ask_ladder)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let snapshot = Snapshot {
        sequence_ms: listed.sequence,
        valid_until_ms: u64::try_from(listed.valid_until_unix_ms).ok()?,
        grid_bp,
        methods,
    };
    snapshot.check().ok()?;
    Some(snapshot)
}

/// What the node holds of an asset, from what its ledgers say: its funds
/// in the hub and in the owner book, none below zero; of that, what the
/// owner book's account may spend now: what it holds, at most the owner's
/// funds there less what payments on their way out of it hold. A move
/// between hub and book counts on the ledger it leaves until that ledger
/// books it out, and on the other once that one books it in: a deposit is
/// held in the book until the XRP Ledger applies it and credited by the
/// hub some seconds later, and a withdrawal stays on the hub's ledger
/// until the hub executes it and reaches the book when the account sees
/// it arrive. Between, the holding reads short of the move, never above:
/// the two ledgers book on their own schedules, and counting a move in
/// flight from either side could count it twice.
#[must_use]
pub fn holding(views: &Views) -> Holding {
    let positive = |balance: i64| u64::try_from(balance.max(0)).unwrap_or(0);
    let book = positive(views.book_owner);
    let total = positive(views.hub_owner).saturating_add(book);
    let on_ledger = views
        .book_on_ledger
        .min(book.saturating_sub(views.book_pending));
    assert!(
        on_ledger <= total,
        "the account's share is part of the whole"
    );
    Holding { total, on_ledger }
}

#[cfg(test)]
mod tests {
    use dassie_feed::method_tag;

    use super::*;

    const ONE: i64 = 1_000_000_000;

    /// An asset with `hub` in its hub and `book` in its owner book, all of
    /// the book in its account, and nothing on its way.
    const fn views(hub: i64, book: i64) -> Views {
        Views {
            hub_owner: hub,
            book_owner: book,
            book_pending: 0,
            book_on_ledger: book.unsigned_abs(),
        }
    }

    const fn units(whole: i64) -> u64 {
        (whole * ONE).unsigned_abs()
    }

    #[test]
    fn an_asset_is_held_in_its_hub_and_its_book() {
        let held = holding(&views(5 * ONE, 95 * ONE));
        assert_eq!(held.total, units(100));
        // A rebalance spends only from the book's account.
        assert_eq!(held.on_ledger, units(95));
    }

    #[test]
    fn a_deposit_on_its_way_never_counts_twice() {
        // Held in the book until the XRP Ledger applied it: still the
        // owner's there, but not the account's to spend.
        let mut leaving = views(5 * ONE, 95 * ONE);
        leaving.book_pending = units(10);
        assert_eq!(holding(&leaving).total, units(100));
        assert_eq!(holding(&leaving).on_ledger, units(85));
        // Applied: out of the book, not credited by the hub yet, so on
        // neither ledger, and the holding reads short of it.
        let applied = views(5 * ONE, 85 * ONE);
        assert_eq!(holding(&applied).total, units(90));
        assert_eq!(holding(&applied).on_ledger, units(85));
        // Credited: in the hub, once.
        let credited = views(15 * ONE, 85 * ONE);
        assert_eq!(holding(&credited).total, units(100));
        assert_eq!(holding(&credited).on_ledger, units(85));
    }

    #[test]
    fn the_account_spends_at_most_the_owners_funds() {
        // The account holds more than the books give the owner, or less.
        let mut more = views(0, 10 * ONE);
        more.book_on_ledger = units(12);
        assert_eq!(holding(&more).on_ledger, units(10));
        let mut less = views(0, 10 * ONE);
        less.book_on_ledger = units(4);
        assert_eq!(holding(&less).on_ledger, units(4));
    }

    #[test]
    fn nothing_counts_below_zero_or_overflows() {
        let held = holding(&views(-ONE, -2 * ONE));
        assert_eq!(held.total, 0);
        assert_eq!(held.on_ledger, 0);
        let most = holding(&views(i64::MAX, i64::MAX));
        assert_eq!(most.total, i64::MAX.unsigned_abs() * 2);
    }

    /// A snapshot of two methods, as the node lists it.
    fn listed() -> (Snapshot, proto::PeerSnapshot) {
        let ladder = Ladder::floor([10, 20, 40, 80, 160, 320, 640, 1_280].map(|n| n * 1_000_000));
        let method = |id: &str, bid: u128| MethodQuotes {
            tag: method_tag(id),
            bid: Quote::floor(bid, 1_000),
            ask: Quote::ceil(bid + 1, 1_000),
            bid_ladder: ladder,
            ask_ladder: ladder,
        };
        let mut methods = vec![method("a-hub", 500), method("b-hub", 1_000)];
        methods.sort_by_key(|method| method.tag);
        let snapshot = Snapshot {
            sequence_ms: 1_000,
            valid_until_ms: 91_000,
            grid_bp: [1, 3, 10, 30, 100, 300, 1_000, 3_000],
            methods,
        };
        let proto = proto::PeerSnapshot {
            node: String::new(),
            sequence: snapshot.sequence_ms,
            valid_until_unix_ms: 91_000,
            grid_bp: snapshot.grid_bp.iter().map(|&bp| u32::from(bp)).collect(),
            methods: snapshot
                .methods
                .iter()
                .map(|method| proto::SnapshotMethod {
                    tag: method.tag.to_vec(),
                    scheme: String::new(),
                    bid: method.bid.to_bits(),
                    ask: method.ask.to_bits(),
                    bid_ladder: method.bid_ladder.amounts().to_vec(),
                    ask_ladder: method.ask_ladder.amounts().to_vec(),
                })
                .collect(),
        };
        (snapshot, proto)
    }

    // spec/areas/rate-feed.md#numbers: a snapshot the node lists reads as
    // the one it took, and one that is not a snapshot's is left out.
    #[test]
    fn a_listed_snapshot_reads_as_the_node_read_it() {
        let (taken, listed) = listed();
        assert_eq!(snapshot(&listed), Some(taken));
        let broken: [fn(&mut proto::PeerSnapshot); 8] = [
            // A grid of seven drops, or one not ascending.
            |s| {
                s.grid_bp.pop();
            },
            |s| s.grid_bp.swap(0, 1),
            // A tag of five bytes.
            |s| s.methods[0].tag.push(0),
            // A quote that is not normalized.
            |s| s.methods[0].bid = 1,
            // A ladder no snapshot carries: an amount between steps, or
            // falling.
            |s| s.methods[0].bid_ladder[1] += 1,
            |s| s.methods[0].ask_ladder.swap(0, 7),
            // Valid only until before it was made.
            |s| s.valid_until_unix_ms = 999,
            // Methods out of the order of their tags.
            |s| s.methods.swap(0, 1),
        ];
        for (index, break_it) in broken.iter().enumerate() {
            let mut listed = listed.clone();
            break_it(&mut listed);
            assert_eq!(snapshot(&listed), None, "{index}");
        }
    }
}
