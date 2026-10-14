//! Live streams (spec 7 §4.6): the wallet's views of state that changes.
//!
//! A stream does not replay the engine's events. It follows the engine's
//! change counts ([`Versions`]): when the count it follows moves, it fetches
//! the state afresh and sends it whole, or what changed since it last sent.
//! So a stream that falls behind never lags; it just sees fewer, later
//! states. The count is marked seen before the fetch, so a change during
//! the fetch wakes the stream again.
//!
//! A stream sends at most one item per [`MIN_ITEM_INTERVAL`], fetches again
//! every [`REFRESH`] whatever the counts say, and sends an item at least
//! every [`KEEPALIVE`]. It ends with UNAVAILABLE when the engine does.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use prost::Message;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::RpcError;
use super::handlers::{Items, Reply};
use super::proto::stream_item;
use crate::engine::{EngineError, Versions};

/// The shortest time between two items of a stream.
pub const MIN_ITEM_INTERVAL: Duration = Duration::from_millis(250);
/// The longest time without an item.
pub const KEEPALIVE: Duration = Duration::from_secs(60);
/// How often a stream fetches again, whatever the counts say.
pub const REFRESH: Duration = Duration::from_secs(5);
/// The largest item, in bytes: a larger snapshot is split.
pub const MAX_ITEM_BYTES: usize = 256 * 1024;

/// The change counts a stream follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    /// The ledgers, balances, tokens and journal.
    Ledger,
    /// The node table, peers, routes and registrations.
    Network,
    /// Both.
    Both,
    /// Open Payments resources.
    Payments,
}

/// The counts a stream has seen.
pub type Seen = (u64, u64, u64);

impl Follow {
    const fn count(self, versions: Versions) -> Seen {
        match self {
            Self::Ledger => (versions.ledger, 0, 0),
            Self::Network => (0, versions.network, 0),
            Self::Both => (versions.ledger, versions.network, 0),
            Self::Payments => (0, 0, versions.payments),
        }
    }
}

/// Where a stream gets its wake-ups.
pub struct Source {
    /// The engine's change counts.
    pub versions: watch::Receiver<Versions>,
    /// Cancelled when the engine stops.
    pub ended: CancellationToken,
}

impl Source {
    /// Marks the counts seen, before a fetch.
    pub fn seen(&mut self, follow: Follow) -> Seen {
        follow.count(*self.versions.borrow_and_update())
    }

    /// Waits until the counts `follow` names moved past `seen`, or a
    /// refresh is due.
    pub async fn wait(&mut self, follow: Follow, seen: Seen) -> Result<(), RpcError> {
        let ended = || RpcError::unavailable(EngineError::Unavailable);
        tokio::select! {
            () = self.ended.cancelled() => return Err(ended()),
            () = tokio::time::sleep(MIN_ITEM_INTERVAL) => {}
        }
        let refresh = tokio::time::sleep(REFRESH);
        tokio::pin!(refresh);
        loop {
            if follow.count(*self.versions.borrow()) != seen {
                return Ok(());
            }
            tokio::select! {
                () = self.ended.cancelled() => return Err(ended()),
                () = &mut refresh => return Ok(()),
                changed = self.versions.changed() => {
                    if changed.is_err() {
                        return Err(ended());
                    }
                }
            }
        }
    }
}

/// Streams a value, whole, whenever it changes.
pub async fn values<T, F, Fut>(
    mut source: Source,
    items: &Items,
    follow: Follow,
    mut fetch: F,
    item: impl Fn(T) -> stream_item::Method,
) -> Result<Reply, RpcError>
where
    T: PartialEq + Clone,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RpcError>>,
{
    let mut last: Option<T> = None;
    let mut sent_at = Instant::now();
    loop {
        let seen = source.seen(follow);
        let value = fetch().await?;
        if last.as_ref() != Some(&value) || sent_at.elapsed() >= KEEPALIVE {
            items.send_wait(item(value.clone())).await?;
            sent_at = Instant::now();
            last = Some(value);
        }
        source.wait(follow, seen).await?;
    }
}

/// Streams a table: all of it first, then the entries that changed and the
/// keys that left. `item` builds an item from `reset`, upserts and
/// removals.
pub async fn table<K, V, F, Fut>(
    mut source: Source,
    items: &Items,
    follow: Follow,
    mut fetch: F,
    item: impl Fn(bool, Vec<V>, Vec<K>) -> stream_item::Method,
) -> Result<Reply, RpcError>
where
    K: Ord + Clone,
    V: PartialEq + Clone + Message,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<BTreeMap<K, V>, RpcError>>,
{
    let mut last: Option<BTreeMap<K, V>> = None;
    let mut sent_at = Instant::now();
    loop {
        let seen = source.seen(follow);
        let current = fetch().await?;
        let (reset, upserted, removed) = match &last {
            None => (true, current.values().cloned().collect(), Vec::new()),
            Some(last) => diff(last, &current),
        };
        let keepalive = sent_at.elapsed() >= KEEPALIVE;
        if reset || !upserted.is_empty() || !removed.is_empty() || keepalive {
            send_chunked(items, reset, upserted, removed, &item).await?;
            sent_at = Instant::now();
        }
        last = Some(current);
        source.wait(follow, seen).await?;
    }
}

/// The entries of `current` that are new or changed since `last`, and the
/// keys of `last` that are gone.
fn diff<K: Ord + Clone, V: PartialEq + Clone>(
    last: &BTreeMap<K, V>,
    current: &BTreeMap<K, V>,
) -> (bool, Vec<V>, Vec<K>) {
    let upserted = current
        .iter()
        .filter(|(key, value)| last.get(key) != Some(value))
        .map(|(_, value)| value.clone())
        .collect();
    let removed = last
        .keys()
        .filter(|key| !current.contains_key(key))
        .cloned()
        .collect();
    (false, upserted, removed)
}

/// Sends upserts in items of at most [`MAX_ITEM_BYTES`]; only the first
/// carries `reset`, and the removals.
async fn send_chunked<K, V: Message>(
    items: &Items,
    reset: bool,
    upserted: Vec<V>,
    removed: Vec<K>,
    item: &impl Fn(bool, Vec<V>, Vec<K>) -> stream_item::Method,
) -> Result<(), RpcError> {
    let mut chunks: Vec<Vec<V>> = vec![Vec::new()];
    let mut bytes = 0;
    for value in upserted {
        let size = value.encoded_len();
        if bytes + size > MAX_ITEM_BYTES && chunks.last().is_some_and(|c| !c.is_empty()) {
            chunks.push(Vec::new());
            bytes = 0;
        }
        bytes += size;
        if let Some(chunk) = chunks.last_mut() {
            chunk.push(value);
        }
    }
    let mut removed = Some(removed);
    for (index, chunk) in chunks.into_iter().enumerate() {
        let first = index == 0;
        let gone = if first {
            removed.take().unwrap_or_default()
        } else {
            Vec::new()
        };
        items.send_wait(item(reset && first, chunk, gone)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::sync::mpsc;

    use super::*;
    use crate::rpc::proto::{self, server_message};

    fn harness() -> (Items, mpsc::Receiver<super::super::connection::Queued>) {
        let (sender, receiver) = mpsc::channel(64);
        let items = Items {
            call_id: 1,
            generation: 1,
            sender,
        };
        (items, receiver)
    }

    fn source() -> (watch::Sender<Versions>, CancellationToken, Source) {
        let (versions, receiver) = watch::channel(Versions::default());
        let ended = CancellationToken::new();
        let source = Source {
            versions: receiver,
            ended: ended.clone(),
        };
        (versions, ended, source)
    }

    fn logs(item: &proto::ServerMessage) -> &proto::LogEvent {
        let Some(server_message::Kind::Item(proto::StreamItem {
            method: Some(stream_item::Method::SubscribeLogs(event)),
        })) = &item.kind
        else {
            panic!("not a log item: {item:?}");
        };
        event
    }

    fn line(index: u64) -> proto::LogLine {
        proto::LogLine {
            index,
            ..proto::LogLine::default()
        }
    }

    // A value is sent when it changes, at most every 250 ms, and again
    // after a minute without a change; the stream ends with the engine.
    #[tokio::test(start_paused = true)]
    async fn values_follow_changes_and_keep_alive() {
        let (items, mut out) = harness();
        let (versions, ended, source) = source();
        let value = Arc::new(AtomicU64::new(1));
        let fetched = Arc::clone(&value);
        let stream = tokio::spawn(async move {
            values(
                source,
                &items,
                Follow::Ledger,
                || {
                    let fetched = Arc::clone(&fetched);
                    async move { Ok(fetched.load(Ordering::SeqCst)) }
                },
                |n| {
                    stream_item::Method::SubscribeLogs(proto::LogEvent {
                        lines: vec![line(n)],
                        ..proto::LogEvent::default()
                    })
                },
            )
            .await
        });
        let first = out.recv().await.unwrap().1;
        assert_eq!(logs(&first).lines[0].index, 1);
        // Two changes within one interval are one item.
        value.store(2, Ordering::SeqCst);
        versions.send_modify(|v| v.ledger += 1);
        value.store(3, Ordering::SeqCst);
        versions.send_modify(|v| v.ledger += 1);
        let started = Instant::now();
        let next = out.recv().await.unwrap().1;
        assert_eq!(logs(&next).lines[0].index, 3);
        assert!(started.elapsed() >= MIN_ITEM_INTERVAL);
        // A network change wakes nothing; a minute brings the keepalive.
        versions.send_modify(|v| v.network += 1);
        let started = Instant::now();
        let again = out.recv().await.unwrap().1;
        assert_eq!(logs(&again).lines[0].index, 3);
        assert!(started.elapsed() >= KEEPALIVE - REFRESH);
        ended.cancel();
        let error = stream.await.unwrap().err().unwrap();
        assert_eq!(error.code, proto::ErrorCode::Unavailable);
    }

    // A table comes whole first, split below the item limit, then as
    // upserts and removals.
    #[tokio::test(start_paused = true)]
    async fn tables_send_a_snapshot_then_changes() {
        let (items, mut out) = harness();
        let (versions, _ended, source) = source();
        let big = "x".repeat(MAX_ITEM_BYTES / 3);
        let table: Arc<std::sync::Mutex<BTreeMap<u64, proto::LogLine>>> =
            Arc::new(std::sync::Mutex::new(
                (1..=4)
                    .map(|i| {
                        (
                            i,
                            proto::LogLine {
                                index: i,
                                message: big.clone(),
                                ..proto::LogLine::default()
                            },
                        )
                    })
                    .collect(),
            ));
        let fetched = Arc::clone(&table);
        tokio::spawn(async move {
            table_stream(source, &items, fetched).await;
        });
        let first = out.recv().await.unwrap().1;
        let second = out.recv().await.unwrap().1;
        assert!(logs(&first).reset && !logs(&second).reset);
        assert_eq!(logs(&first).lines.len() + logs(&second).lines.len(), 4);
        {
            let mut table = table.lock().unwrap();
            table.remove(&2);
            table.get_mut(&3).unwrap().message = "changed".into();
        }
        versions.send_modify(|v| v.network += 1);
        let change = out.recv().await.unwrap().1;
        let change = logs(&change);
        assert!(!change.reset);
        assert_eq!(change.lines.len(), 1);
        assert_eq!(change.lines[0].index, 3);
        assert_eq!(change.skipped, 2, "the removal");
    }

    async fn table_stream(
        source: Source,
        items: &Items,
        table: Arc<std::sync::Mutex<BTreeMap<u64, proto::LogLine>>>,
    ) {
        let _ = super::table(
            source,
            items,
            Follow::Network,
            || {
                let table = Arc::clone(&table);
                async move { Ok(table.lock().unwrap().clone()) }
            },
            |reset, lines, removed: Vec<u64>| {
                stream_item::Method::SubscribeLogs(proto::LogEvent {
                    reset,
                    lines,
                    skipped: removed.first().copied().unwrap_or(0),
                })
            },
        )
        .await;
    }
}
