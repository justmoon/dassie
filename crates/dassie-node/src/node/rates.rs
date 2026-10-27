//! The rate fetcher (spec/decisions/exchange-rates.md): every minute, each
//! source is asked at once, and the rates its answers agree on are kept and
//! handed to the engine.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::Node;
use crate::clock::clock_moved;
use crate::log::target;
use crate::outbound::WebRequest;
use crate::rates::{self, Answer, BASE, MAX_RESPONSE_BYTES, RateSource};
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
        loop {
            if let Some(engine) = self.engine() {
                self.fetch_rates(&engine).await;
            }
            let next_ms = self.now_ms().saturating_add(self.policies.rates_every_ms);
            loop {
                let wait_ms = next_ms.saturating_sub(self.now_ms());
                if wait_ms == 0 {
                    break;
                }
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    // A clock no one moves by hand is the system clock, which
                    // may step back, and then the rates kept before the step
                    // are stamped in its future and not fresh. So on it the
                    // interval's real end is the deadline, and a step never
                    // puts off the next fetch.
                    () = tokio::time::sleep(Duration::from_millis(wait_ms)) => {
                        if moves.is_none() {
                            break;
                        }
                    }
                    () = clock_moved(&mut moves) => {}
                    () = self.rates_wake.notified() => break,
                    changed = engines.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        break;
                    }
                }
            }
        }
    }

    /// Asks the rate fetcher to fetch now.
    pub fn wake_rates(&self) {
        self.rates_wake.notify_one();
    }

    /// One cycle: the currencies the node needs, from every source.
    async fn fetch_rates(&self, engine: &crate::engine::Engine) {
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
            futures_util::future::join_all(sources.iter().map(|source| self.ask(source))).await;
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

    /// What one source says, if it answers well.
    async fn ask(&self, source: &RateSource) -> Answer {
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
