//! The rate fetcher (spec/decisions/exchange-rates.md): every minute, each
//! source is asked at once, and the rates its answers agree on are kept and
//! handed to the engine. A source of mainnet's XRP/RLUSD pool is followed
//! all along instead ([`super::pool_rate`]), and answers with the rate its
//! reader has then.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::Node;
use super::pool_rate::Follower;
use crate::clock::interval_passed;
use crate::log::target;
use crate::outbound::WebRequest;
use crate::rates::{self, Answer, BASE, MAX_RESPONSE_BYTES, RateSource, SourceKind};
use crate::store::StoredRate;

/// How long a source has to answer.
const SOURCE_TIMEOUT_MS: u64 = 10_000;

impl Node {
    /// Fetches rates while the node runs: at once when the engine starts
    /// or the owner changes the sources, and every
    /// [`rates_every_ms`](super::Policies::rates_every_ms) otherwise: of
    /// real time on the system clock, and of the node's clock on one moved
    /// by hand, whose moves wake it, so a test reaches the next fetch by
    /// moving its clock rather than by waiting in real time.
    pub async fn run_rates(self: Arc<Self>, shutdown: CancellationToken) {
        let mut engines = self.engine.subscribe();
        let mut moves = self.clock.subscribe();
        let mut pool: Option<Follower> = None;
        loop {
            self.follow_pool(&mut pool, &shutdown);
            if let Some(engine) = self.engine() {
                self.fetch_rates(&engine, pool.as_ref()).await;
            }
            let every_ms = self.policies.rates_every_ms;
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = interval_passed(&*self.clock, &mut moves, every_ms) => {}
                () = self.rates_wake.notified() => {}
                changed = engines.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// Asks the rate fetcher to fetch now.
    pub fn wake_rates(&self) {
        self.rates_wake.notify_one();
    }

    /// Follows the pool on the server the owner's first source of it names,
    /// if one does: on the one followed already, or anew.
    fn follow_pool(&self, pool: &mut Option<Follower>, shutdown: &CancellationToken) {
        let named = match self.store.rate_sources() {
            Ok(sources) => (sources.into_iter())
                .find(|source| source.kind == SourceKind::XrplPool)
                .map(|source| source.url().to_owned()),
            Err(error) => {
                tracing::error!(target: target::ENGINE, %error, "reading the rate sources");
                return;
            }
        };
        if pool.as_ref().map(Follower::url) == named.as_deref() {
            return;
        }
        *pool = named.map(|url| {
            let policies = (self.policies.pool_rate, self.policies.pool_rate_unl);
            let client = self.outbound().cloned();
            Follower::start(&url, client, Arc::clone(&self.clock), policies, shutdown)
        });
    }

    /// One cycle: the currencies the node needs, from every source.
    async fn fetch_rates(&self, engine: &crate::engine::Engine, pool: Option<&Follower>) {
        let Ok(mut wanted) = engine.rate_currencies().await else {
            return;
        };
        if let Ok(Some(display)) = self.store.display_currency() {
            wanted.push(display);
        }
        wanted.sort_unstable();
        wanted.dedup();
        if wanted.iter().all(|code| *code == BASE) {
            return;
        }
        let sources = match self.store.rate_sources() {
            Ok(sources) => sources,
            Err(error) => {
                tracing::error!(target: target::ENGINE, %error, "reading the rate sources");
                return;
            }
        };
        let answers: Vec<Answer> =
            futures_util::future::join_all((sources.iter()).map(|source| self.ask(source, pool)))
                .await;
        let now = self.now_ms();
        let fresh: Vec<StoredRate> = rates::combine(&wanted, &answers)
            .into_iter()
            .map(|(code, rate)| (code, rate, now))
            .collect();
        for code in &wanted {
            if !fresh.iter().any(|(fetched, _, _)| fetched == code) {
                tracing::warn!(target: target::ENGINE, currency = code, "no rate agreed on: the last one stays, until it is an hour old");
            }
        }
        if fresh.is_empty() {
            return;
        }
        if let Err(error) = self.store.save_exchange_rates(&fresh) {
            tracing::error!(target: target::ENGINE, %error, "keeping exchange rates");
        }
        if let Err(error) = engine.set_rates(fresh).await {
            tracing::debug!(target: target::ENGINE, %error, "handing the engine exchange rates");
        }
    }

    /// What one source says, if it answers well: the pool's reader, what
    /// it has now.
    async fn ask(&self, source: &RateSource, pool: Option<&Follower>) -> Answer {
        if source.kind == SourceKind::XrplPool {
            let followed = pool.filter(|pool| pool.url() == source.url());
            let rate = followed.and_then(|pool| pool.rate(self.now_ms()));
            if rate.is_none() {
                tracing::debug!(target: target::ENGINE, url = source.url(), "the XRP/RLUSD pool has no rate yet");
            }
            return (source.kind, rate.map(|rate| vec![("XRP", rate)]));
        }
        let said = match self.outbound() {
            Some(client) => {
                let request = WebRequest {
                    method: http::Method::GET,
                    url: source.url().to_owned(),
                    headers: vec![(http::header::ACCEPT, source.kind.accept().to_owned())],
                    body: None,
                    timeout_ms: SOURCE_TIMEOUT_MS,
                    max_response_bytes: MAX_RESPONSE_BYTES,
                };
                match client.web(request).await {
                    Ok(response) if response.status == 200 => {
                        rates::parse(source.kind, &response.body, self.now_ms())
                            .map_err(|e| e.0.to_owned())
                    }
                    Ok(response) => Err(format!("status {}", response.status)),
                    Err(error) => Err(error.to_string()),
                }
            }
            None => Err("no client".to_owned()),
        };
        match said {
            Ok(rates) => (source.kind, Some(rates)),
            Err(error) => {
                tracing::debug!(target: target::ENGINE, url = source.url(), %error, "a rate source did not answer");
                (source.kind, None)
            }
        }
    }
}
