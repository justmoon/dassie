//! The fake XRP Ledger of `dassie-xrpl`, served as rippled serves the real
//! one: its WebSocket API on `ws://`, its faucet on HTTPS, and a ledger
//! closed every so often. For tests and dev environments only (feature
//! `testing`): CI and `node --run dev -- --xrpl fake` settle against it.
//!
//! Next to the faucet, under `/tokens/`, the issuer of its test RLUSD takes
//! requests: it mints to an address, freezes, deep-freezes and globally
//! freezes, sets a transfer fee and claws back, each in the next ledger
//! (spec/areas/xrpl.md#the-fake-ledger).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use dassie_xrpl::AccountId;
use dassie_xrpl::codec::TokenValue;
use dassie_xrpl::fake::{FakeLedger, Token};
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::clock::{Clock, SystemClock};
use crate::store::StoredCertificate;

/// How the fake ledger is served.
#[derive(Clone, Debug)]
pub struct Options {
    /// Where its WebSocket API listens.
    pub listen: SocketAddr,
    /// Where its faucet listens, over HTTPS with this certificate.
    pub faucet: Option<(SocketAddr, StoredCertificate)>,
    /// How often a ledger closes.
    pub close_every: Duration,
}

struct Shared {
    ledger: Mutex<FakeLedger>,
    connections: Mutex<BTreeMap<u64, mpsc::Sender<Vec<u8>>>>,
    next: AtomicU64,
}

/// A running fake ledger.
pub struct Server {
    shared: Arc<Shared>,
    cancel: CancellationToken,
    /// Its WebSocket URL.
    pub url: String,
    /// Its faucet's URL, if it has one.
    pub faucet_url: Option<String>,
    /// Where its issuer takes requests, with the faucet: `…/tokens`.
    pub tokens_url: Option<String>,
}

impl Server {
    /// Starts serving.
    ///
    /// # Errors
    ///
    /// A listener cannot bind, or the certificate is invalid.
    pub async fn start(options: Options) -> Result<Self, String> {
        // Its ledgers close on the wall clock, the first a second after
        // any ledger of a server before it: a new server is a new chain,
        // and nodes that knew an earlier one see a testnet reset.
        let mut ledger = FakeLedger::new();
        ledger.set_close_time(close_time_now().saturating_add(1));
        let shared = Arc::new(Shared {
            ledger: Mutex::new(ledger),
            connections: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
        });
        let cancel = CancellationToken::new();
        let listener = TcpListener::bind(options.listen)
            .await
            .map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        tokio::spawn(serve_ws(listener, Arc::clone(&shared), cancel.clone()));
        tokio::spawn(close_ledgers(
            options.close_every,
            Arc::clone(&shared),
            cancel.clone(),
        ));
        let (faucet_url, tokens_url) = match options.faucet {
            Some((listen, certificate)) => {
                let listener = TcpListener::bind(listen).await.map_err(|e| e.to_string())?;
                let address = listener.local_addr().map_err(|e| e.to_string())?;
                let loaded = crate::tls::load(&certificate).map_err(|e| e.to_string())?;
                let slot = Arc::new(crate::tls::CertificateSlot::new(loaded));
                let config = crate::tls::server_config(slot).map_err(|e| e.to_string())?;
                let acceptor = tokio_rustls::TlsAcceptor::from(config);
                tokio::spawn(serve_faucet(
                    listener,
                    acceptor,
                    Arc::clone(&shared),
                    cancel.clone(),
                ));
                (
                    Some(format!("https://localhost:{}/accounts", address.port())),
                    Some(format!("https://localhost:{}/tokens", address.port())),
                )
            }
            None => (None, None),
        };
        Ok(Self {
            shared,
            cancel,
            url: format!("ws://{address}"),
            faucet_url,
            tokens_url,
        })
    }

    /// The ledger, for a test to look at or change.
    pub fn ledger(&self) -> MutexGuard<'_, FakeLedger> {
        self.shared
            .ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Stops serving, and drops every connection.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The XRP Ledger's epoch, 2000-01-01, in seconds since the Unix epoch.
const RIPPLE_EPOCH_UNIX_S: u64 = 946_684_800;

/// Now, as the XRP Ledger counts close times: seconds since its epoch. The
/// fake ledger's servers close their ledgers then.
#[must_use]
pub fn close_time_now() -> u64 {
    (SystemClock.now_ms() / 1_000).saturating_sub(RIPPLE_EPOCH_UNIX_S)
}

async fn close_ledgers(every: Duration, shared: Arc<Shared>, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(every);
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            _ = interval.tick() => {}
        }
        let out = shared
            .ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close_at(close_time_now());
        let connections = shared
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (connection, message) in out {
            if let Some(sender) = connections.get(&connection) {
                let _ = sender.try_send(message);
            }
        }
    }
}

async fn serve_ws(listener: TcpListener, shared: Arc<Shared>, cancel: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else { continue };
        tokio::spawn(connection(stream, Arc::clone(&shared), cancel.clone()));
    }
}

async fn connection(stream: TcpStream, shared: Arc<Shared>, cancel: CancellationToken) {
    let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let id = shared.next.fetch_add(1, Ordering::Relaxed);
    let (sender, mut stream_out) = mpsc::channel(1024);
    shared
        .connections
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, sender);
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            out = stream_out.recv() => {
                let Some(out) = out else { break };
                if socket.send(Message::text(String::from_utf8_lossy(&out).into_owned())).await.is_err() {
                    break;
                }
            }
            incoming = socket.next() => {
                let bytes = match incoming {
                    Some(Ok(Message::Text(text))) => text.as_bytes().to_vec(),
                    Some(Ok(Message::Binary(bytes))) => bytes.to_vec(),
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                    _ => break,
                };
                let reply = shared
                    .ledger
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .request(id, &bytes);
                if socket.send(Message::text(String::from_utf8_lossy(&reply).into_owned())).await.is_err() {
                    break;
                }
            }
        }
    }
    shared
        .connections
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&id);
    shared
        .ledger
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .disconnect(id);
}

async fn serve_faucet(
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else { continue };
        let (acceptor, shared) = (acceptor.clone(), Arc::clone(&shared));
        let serve = async move {
            let Ok(tls) = acceptor.accept(stream).await else {
                return;
            };
            let service = service_fn(move |request| {
                let shared = Arc::clone(&shared);
                async move { Ok::<_, Infallible>(serve_request(&shared, request).await) }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(tls), service)
                .await;
        };
        // A connection kept alive goes with the server, so a client's next
        // request reaches the faucet of a server started in its place.
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = serve => {}
            }
        });
    }
}

/// The faucet and the issuer, over HTTPS: a JSON body, a JSON answer.
async fn serve_request(
    shared: &Shared,
    request: Request<hyper::body::Incoming>,
) -> Response<Full<Bytes>> {
    let path = request.uri().path().to_owned();
    let (status, body) = match Limited::new(request.into_body(), 4096).collect().await {
        Ok(body) => answer(
            &mut shared.ledger.lock().unwrap_or_else(PoisonError::into_inner),
            &path,
            &body.to_bytes(),
        ),
        Err(_) => (StatusCode::BAD_REQUEST, "{}".to_string()),
    };
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    response
}

/// What a request to `path` with `body` gets, done to `ledger`: an
/// issuer's request under `/tokens/`, else the faucet's.
fn answer(ledger: &mut FakeLedger, path: &str, body: &[u8]) -> (StatusCode, String) {
    let body = serde_json::from_slice::<serde_json::Value>(body).unwrap_or_default();
    let Some(action) = path.strip_prefix("/tokens/") else {
        // The faucet: `{"destination": "r…"}` gets 1000 test XRP in the
        // next ledger.
        let destination = body
            .get("destination")
            .and_then(serde_json::Value::as_str)
            .and_then(AccountId::parse);
        let Some(destination) = destination else {
            return (
                StatusCode::BAD_REQUEST,
                r#"{"error":"no destination"}"#.into(),
            );
        };
        ledger.fund(destination);
        return (
            StatusCode::OK,
            format!(r#"{{"account":{{"address":"{destination}"}}}}"#),
        );
    };
    match issuer_request(ledger, action, &body) {
        None => (StatusCode::NOT_FOUND, r#"{"error":"unknown"}"#.into()),
        Some(Ok(())) => (StatusCode::OK, r#"{"queued":true}"#.into()),
        Some(Err(error)) => (
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": error }).to_string(),
        ),
    }
}

/// The issuer of the fake's test RLUSD does `action`, in the next ledger;
/// `None` if there is no such action.
///
/// - `mint`, `{"destination": "r…", "value": "12.5"}`: pays it RLUSD.
/// - `freeze` and `deep-freeze`, `{"account": "r…", "on": true}`: freezes
///   its line or deep-freezes it; `false` lifts the freeze (and any deep
///   freeze), or the deep freeze alone.
/// - `global-freeze`, `{"on": true}`: freezes every line, or lifts that.
/// - `transfer-rate`, `{"rate": 1002000000}`: a fee on transfers between
///   holders, in billionths; 0 for none.
/// - `clawback`, `{"account": "r…", "value": "2"}`: takes it back.
fn issuer_request(
    ledger: &mut FakeLedger,
    action: &str,
    body: &serde_json::Value,
) -> Option<Result<(), &'static str>> {
    let token = Token::RLUSD;
    let account = |key: &str| {
        body.get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(AccountId::parse)
            .ok_or("no account")
    };
    let value = || {
        body.get("value")
            .and_then(serde_json::Value::as_str)
            .and_then(TokenValue::parse)
            .ok_or("no value")
    };
    let on = || {
        body.get("on")
            .and_then(serde_json::Value::as_bool)
            .ok_or("no on")
    };
    let rate = || {
        body.get("rate")
            .and_then(serde_json::Value::as_u64)
            .and_then(|rate| u32::try_from(rate).ok())
            .ok_or("no rate")
    };
    Some(match action {
        "mint" => account("destination").and_then(|to| ledger.issue(to, token.amount(value()?))),
        "freeze" => account("account").and_then(|holder| ledger.freeze(holder, token, on()?)),
        "deep-freeze" => {
            account("account").and_then(|holder| ledger.deep_freeze(holder, token, on()?))
        }
        "global-freeze" => on().and_then(|on| ledger.global_freeze(token.issuer, on)),
        "transfer-rate" => rate().and_then(|rate| ledger.set_transfer_rate(token.issuer, rate)),
        "clawback" => {
            account("account").and_then(|holder| ledger.claw_back(holder, token.amount(value()?)))
        }
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use dassie_xrpl::Wallet;
    use dassie_xrpl::api::account_flags;
    use dassie_xrpl::codec::{TF_SET_NO_RIPPLE, TrustSet};
    use dassie_xrpl::fake::{FEE_BASE, RLUSD_ISSUER};
    use serde_json::json;

    use super::*;

    fn value(text: &str) -> TokenValue {
        TokenValue::parse(text).unwrap()
    }

    /// A holder with a line to the issuer, as the scheme makes it.
    fn holder(ledger: &mut FakeLedger) -> AccountId {
        let wallet = Wallet::from_seed([1; 16]);
        ledger.deposit(wallet.account(), 100_000_000);
        ledger.close();
        let signed = TrustSet {
            account: wallet.account(),
            limit: Token::RLUSD.amount(TokenValue::MAX),
            fee: FEE_BASE,
            // A new account's first sequence is the ledger it was made in.
            sequence: ledger.ledger(),
            last_ledger_sequence: ledger.ledger() + 5,
            flags: TF_SET_NO_RIPPLE,
            network_id: None,
            memos: Vec::new(),
            signing_pub_key: *wallet.public_key(),
            signature: None,
        }
        .sign(&wallet);
        let submit =
            json!({ "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&signed.blob) });
        let reply = ledger.request(1, &serde_json::to_vec(&submit).unwrap());
        assert!(String::from_utf8_lossy(&reply).contains("tesSUCCESS"));
        ledger.close();
        wallet.account()
    }

    /// Asks, and closes the ledger that applies it.
    fn ask(ledger: &mut FakeLedger, path: &str, body: &serde_json::Value) -> StatusCode {
        let (status, _) = answer(ledger, path, &serde_json::to_vec(body).unwrap());
        ledger.close();
        status
    }

    #[test]
    fn the_issuer_mints_freezes_and_claws_back_on_request() {
        let mut ledger = FakeLedger::new();
        let holder = holder(&mut ledger);
        let address = holder.to_address();
        let held = |ledger: &FakeLedger| ledger.token_balance(holder, Token::RLUSD);
        let theirs = |ledger: &FakeLedger| {
            let line = ledger
                .line(holder, RLUSD_ISSUER, Token::RLUSD.currency)
                .unwrap();
            line.view(holder).unwrap().theirs
        };
        let mint = json!({ "destination": address, "value": "25.5" });
        assert_eq!(ask(&mut ledger, "/tokens/mint", &mint), StatusCode::OK);
        assert_eq!(held(&ledger), Some(value("25.5")));
        let on = json!({ "account": address, "on": true });
        assert_eq!(ask(&mut ledger, "/tokens/freeze", &on), StatusCode::OK);
        assert!(theirs(&ledger).freeze && !theirs(&ledger).deep_freeze);
        assert_eq!(ask(&mut ledger, "/tokens/deep-freeze", &on), StatusCode::OK);
        assert!(theirs(&ledger).deep_freeze);
        let off = json!({ "account": address, "on": false });
        assert_eq!(ask(&mut ledger, "/tokens/freeze", &off), StatusCode::OK);
        assert!(!theirs(&ledger).freeze && !theirs(&ledger).deep_freeze);
        let global = json!({ "on": true });
        assert_eq!(
            ask(&mut ledger, "/tokens/global-freeze", &global),
            StatusCode::OK
        );
        let flags = ledger.account_flags(RLUSD_ISSUER).unwrap();
        assert_ne!(flags & account_flags::GLOBAL_FREEZE, 0);
        let rate = json!({ "rate": 1_002_000_000 });
        assert_eq!(
            ask(&mut ledger, "/tokens/transfer-rate", &rate),
            StatusCode::OK
        );
        let claw = json!({ "account": address, "value": "5" });
        assert_eq!(ask(&mut ledger, "/tokens/clawback", &claw), StatusCode::OK);
        assert_eq!(held(&ledger), Some(value("20.5")));
        assert!(ledger.tokens_conserved());
    }

    #[test]
    fn requests_the_issuer_cannot_take_are_refused() {
        let mut ledger = FakeLedger::new();
        let someone = AccountId([7; 20]);
        let address = someone.to_address();
        let refused = [
            ("/tokens/mint", json!({ "destination": address })),
            (
                "/tokens/mint",
                json!({ "destination": address, "value": "0" }),
            ),
            ("/tokens/mint", json!({ "value": "1" })),
            ("/tokens/freeze", json!({ "account": address })),
            (
                "/tokens/clawback",
                json!({ "account": address, "value": "-1" }),
            ),
            ("/tokens/transfer-rate", json!({ "rate": 5 })),
            ("/tokens/global-freeze", json!({})),
        ];
        for (path, body) in refused {
            assert_eq!(
                ask(&mut ledger, path, &body),
                StatusCode::BAD_REQUEST,
                "{path} {body}"
            );
        }
        assert_eq!(ledger.queued(), 0);
        assert_eq!(
            ask(&mut ledger, "/tokens/print", &json!({})),
            StatusCode::NOT_FOUND
        );
        // Anything else is the faucet.
        let fund = json!({ "destination": address });
        assert_eq!(ask(&mut ledger, "/accounts", &fund), StatusCode::OK);
        assert!(ledger.balance(someone).is_some());
    }
}
