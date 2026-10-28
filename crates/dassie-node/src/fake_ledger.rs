//! The fake XRP Ledger of `dassie-xrpl`, served as rippled serves the real
//! one: its WebSocket API on `ws://`, its faucet on HTTPS, and a ledger
//! closed every so often. For tests and dev environments only (feature
//! `testing`): CI and `node --run dev` settle against it.
//! It is the testnet, or mainnet, which has no faucet
//! (spec/decisions/mainnet-schemes.md).
//!
//! Next to the faucet, under `/tokens/`, the issuer of its network's dollar
//! takes requests: it mints to an address, freezes, deep-freezes and
//! globally freezes, sets a transfer fee and claws back, and makes or sets
//! an AMM pool of XRP and its token, each in the next ledger. At `/deposit`,
//! XRP arrives from outside, as an owner sends it to fund an account on
//! mainnet (spec/areas/xrpl.md#the-fake-ledger).
//!
//! Its validators sign every ledger, and their publisher's list is served
//! at `/vl`, as a list site serves one; under `/validators/` a validator's
//! key rotates, it is withheld or revokes its key, and a new list is
//! published; and `/lie` makes every connection lie in one way, or be
//! honest again (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use dassie_xrpl::AccountId;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::codec::{Currency, TokenValue};
use dassie_xrpl::fake::{FakeLedger, Lie};
use dassie_xrpl::keys::PublicKey;
use dassie_xrpl::network::{MAINNET, Network};
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

use crate::clock::{Clock, SystemClock, interval_passed};
use crate::store::StoredCertificate;

/// How the fake ledger is served.
#[derive(Clone, Debug)]
pub struct Options {
    /// The network it is: the testnet or mainnet.
    pub network: &'static Network,
    /// Where its WebSocket API listens.
    pub listen: SocketAddr,
    /// Where its faucet, its issuer and its deposits listen, over HTTPS
    /// with this certificate. Mainnet's has no faucet.
    pub faucet: Option<(SocketAddr, StoredCertificate)>,
    /// How often a ledger closes: in real time, or on the clock a test
    /// gives it ([`Server::close_on`]).
    pub close_every: Duration,
}

/// The most XRP one request to `/deposit` sends, in drops: a hundred
/// thousand XRP, more than any test needs and far less than genesis holds.
pub const MAX_DEPOSIT_DROPS: u64 = 100_000 * dassie_xrpl::DROPS_PER_XRP;

struct Shared {
    ledger: Mutex<FakeLedger>,
    connections: Mutex<BTreeMap<u64, mpsc::Sender<Vec<u8>>>>,
    next: AtomicU64,
    /// The clock its ledgers close on.
    clock: Mutex<Arc<dyn Clock>>,
    /// Told when a test sets the clock, so that its closes go by it at once.
    clock_set: tokio::sync::Notify,
}

/// A running fake ledger.
pub struct Server {
    shared: Arc<Shared>,
    cancel: CancellationToken,
    /// Its WebSocket URL.
    pub url: String,
    /// Its faucet's URL, if it has one: never on mainnet.
    pub faucet_url: Option<String>,
    /// Where its issuer takes requests, with the faucet: `…/tokens`.
    pub tokens_url: Option<String>,
    /// Where XRP is deposited from outside, with the faucet: `…/deposit`.
    pub deposit_url: Option<String>,
    /// Where its validators' publisher serves its list, with the faucet:
    /// `…/vl`.
    pub validator_list_url: Option<String>,
    /// Where its validators and its servers take requests, with the
    /// faucet: `…/validators/…` and `…/lie` under it.
    pub control_url: Option<String>,
    /// The master key of its validators' publisher.
    pub publisher_key: PublicKey,
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
        let mut ledger = if *options.network == MAINNET {
            FakeLedger::mainnet()
        } else {
            FakeLedger::new()
        };
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        ledger.set_close_time(close_time_of(clock.as_ref()).saturating_add(1));
        let publisher_key = ledger.publisher_key();
        let shared = Arc::new(Shared {
            ledger: Mutex::new(ledger),
            connections: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
            clock: Mutex::new(clock),
            clock_set: tokio::sync::Notify::new(),
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
        let (faucet_url, tokens_url, deposit_url, validator_list_url, control_url) =
            match options.faucet {
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
                    let base = format!("https://localhost:{}", address.port());
                    (
                        options.network.faucet.map(|_| format!("{base}/accounts")),
                        Some(format!("{base}/tokens")),
                        Some(format!("{base}/deposit")),
                        Some(format!("{base}/vl")),
                        Some(base),
                    )
                }
                None => (None, None, None, None, None),
            };
        Ok(Self {
            shared,
            cancel,
            url: format!("ws://{address}"),
            faucet_url,
            tokens_url,
            deposit_url,
            validator_list_url,
            control_url,
            publisher_key,
        })
    }

    /// The ledger, for a test to look at or change.
    pub fn ledger(&self) -> MutexGuard<'_, FakeLedger> {
        self.shared
            .ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Closes its ledgers on `clock` from now on, a test's, whose nodes then
    /// read its validations as fresh: a second after the last at least, so
    /// a ledger never closes before its parent. A clock moved by hand paces
    /// the closes too: one closes each time the test moves it on by
    /// [`Options::close_every`], a second at least, and none while it
    /// stands still, so that a test waits for a close by moving its clock,
    /// never for real time (spec/areas/testing.md#over-real-sockets).
    pub fn close_on(&self, clock: Arc<dyn Clock>) {
        *self
            .shared
            .clock
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = clock;
        self.shared.clock_set.notify_one();
    }

    /// What an account's config names to trust its validators: its
    /// publisher, whose list it serves, as an owner's publishers; `None`
    /// without the HTTPS listener that
    /// serves the list.
    #[must_use]
    pub fn publishers(&self) -> Option<serde_json::Value> {
        let site = self.validator_list_url.as_ref()?;
        Some(self.ledger().publishers(site))
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
    close_time_of(&SystemClock)
}

/// Now by `clock`, as the XRP Ledger counts close times.
fn close_time_of(clock: &dyn Clock) -> u64 {
    (clock.now_ms() / 1_000).saturating_sub(RIPPLE_EPOCH_UNIX_S)
}

/// The least time between two closes on a clock moved by hand: a ledger's
/// close time counts whole seconds, and each is a second after its
/// parent's at least, so closes any closer would run the ledger's time
/// ahead of the test's.
const LEAST_CLOSE_ON_A_TEST_CLOCK_MS: u64 = 1_000;

/// Closes a ledger every `every` on the server's clock: of real time on the
/// system clock, and on a test's whenever the test moves it that far, a
/// second at least ([`Server::close_on`]), so that a ledger closes as the
/// test's daemons' time passes and never while it stands still.
async fn close_ledgers(every: Duration, shared: Arc<Shared>, cancel: CancellationToken) {
    let every_ms = u64::try_from(every.as_millis()).unwrap_or(u64::MAX);
    loop {
        let clock = Arc::clone(&shared.clock.lock().unwrap_or_else(PoisonError::into_inner));
        let mut moves = clock.subscribe();
        let wait_ms = if moves.is_some() {
            every_ms.max(LEAST_CLOSE_ON_A_TEST_CLOCK_MS)
        } else {
            every_ms
        };
        tokio::select! {
            () = cancel.cancelled() => return,
            // A test set its clock: the wait starts again on it.
            () = shared.clock_set.notified() => continue,
            () = interval_passed(clock.as_ref(), &mut moves, wait_ms) => {}
        }
        let clock = Arc::clone(&shared.clock.lock().unwrap_or_else(PoisonError::into_inner));
        let out = shared
            .ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close_at(close_time_of(clock.as_ref()));
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

/// What a request to `path` with `body` gets, done to `ledger`: the
/// validator list, a validator's or a lie's under `/validators/` and
/// `/lie`, an issuer's request under `/tokens/`, a deposit at `/deposit`,
/// else the faucet's, which mainnet does not have.
fn answer(ledger: &mut FakeLedger, path: &str, body: &[u8]) -> (StatusCode, String) {
    let body = serde_json::from_slice::<serde_json::Value>(body).unwrap_or_default();
    if path == "/vl" {
        let list = String::from_utf8_lossy(&ledger.validator_list()).into_owned();
        return (StatusCode::OK, list);
    }
    if let Some(answered) = validators_request(ledger, path, &body) {
        return match answered {
            Ok(()) => (StatusCode::OK, "{}".into()),
            Err(error) => (
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": error }).to_string(),
            ),
        };
    }
    let destination = body
        .get("destination")
        .and_then(serde_json::Value::as_str)
        .and_then(AccountId::parse);
    let no_destination = || {
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":"no destination"}"#.to_string(),
        )
    };
    if path == "/deposit" {
        // `{"destination": "r…", "drops": "1000000"}`: XRP from outside,
        // in the next ledger, as an owner funds an account.
        let drops = body
            .get("drops")
            .and_then(serde_json::Value::as_str)
            .and_then(|drops| drops.parse::<u64>().ok())
            .filter(|drops| (1..=MAX_DEPOSIT_DROPS).contains(drops));
        let (Some(destination), Some(drops)) = (destination, drops) else {
            return (
                StatusCode::BAD_REQUEST,
                r#"{"error":"no destination or drops"}"#.into(),
            );
        };
        ledger.deposit(destination, drops);
        return (StatusCode::OK, r#"{"queued":true}"#.into());
    }
    let Some(action) = path.strip_prefix("/tokens/") else {
        // The faucet: `{"destination": "r…"}` gets 1000 test XRP in the
        // next ledger. Mainnet has none.
        if ledger.network().faucet.is_none() {
            return (StatusCode::NOT_FOUND, r#"{"error":"no faucet"}"#.into());
        }
        let Some(destination) = destination else {
            return no_destination();
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

/// What the validators and the server do on request, at once; `None` if
/// `path` is none of theirs.
///
/// - `/lie`, `{"kind": "altered-object"}`: every connection, open or to
///   come, tells that lie ([`Lie::name`]); `"honest"` ends it.
/// - `/validators/rotate`, `{"validator": 0}`: the validator signs with a
///   new key from the next ledger.
/// - `/validators/withhold`, `{"validator": 0, "on": true}`: it signs
///   nothing while on.
/// - `/validators/revoke`, `{"validator": 0}`: it revokes its master key.
/// - `/validators/publish`, `{"expiration": 900000000}`: the publisher
///   publishes a list of the validators as their keys stand, expiring
///   then, in seconds since 2000-01-01, and in effect from `"effective"`,
///   if given.
fn validators_request(
    ledger: &mut FakeLedger,
    path: &str,
    body: &serde_json::Value,
) -> Option<Result<(), &'static str>> {
    let validator = || {
        body.get("validator")
            .and_then(serde_json::Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < dassie_xrpl::fake::VALIDATORS)
            .ok_or("no validator")
    };
    let seconds = |key: &str| {
        body.get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|seconds| u32::try_from(seconds).ok())
    };
    Some(match path {
        "/lie" => match body.get("kind").and_then(serde_json::Value::as_str) {
            Some("honest") => {
                ledger.set_lie_for_all(None);
                Ok(())
            }
            Some(kind) => Lie::by_name(kind)
                .map(|lie| ledger.set_lie_for_all(Some(lie)))
                .ok_or("no such lie"),
            None => Err("no kind"),
        },
        "/validators/rotate" => validator().map(|index| ledger.rotate_validator(index)),
        "/validators/revoke" => validator().map(|index| ledger.revoke_validator(index)),
        "/validators/withhold" => validator().and_then(|index| {
            let on = body.get("on").and_then(serde_json::Value::as_bool);
            on.map(|on| ledger.withhold_validator(index, on))
                .ok_or("no on")
        }),
        "/validators/publish" => seconds("expiration")
            .map(|expiration| ledger.publish_validator_list(expiration, seconds("effective")))
            .ok_or("no expiration"),
        _ => return None,
    })
}

/// The issuer of the fake's dollar token, or of the token `"token"` names
/// (`"RLUSD"` or `"DSUSD"`), does `action`, in the next ledger; `None` if
/// there is no such action.
///
/// - `mint`, `{"destination": "r…", "value": "12.5"}`: pays it the token.
/// - `freeze` and `deep-freeze`, `{"account": "r…", "on": true}`: freezes
///   its line or deep-freezes it; `false` lifts the freeze (and any deep
///   freeze), or the deep freeze alone.
/// - `global-freeze`, `{"on": true}`: freezes every line, or lifts that.
/// - `transfer-rate`, `{"rate": 1002000000}`: a fee on transfers between
///   holders, in billionths; 0 for none.
/// - `clawback`, `{"account": "r…", "value": "2"}`: takes it back.
/// - `amm`, `{"drops": "1000000000", "value": "500", "fee": 500}`: makes
///   the pool of XRP and the token, or sets it, to hold these drops and
///   this value and charge this trading fee, in hundred-thousandths. With
///   `"currency"` and `"issuer"`, the pool of that token, of one of the
///   fake's issuers.
fn issuer_request(
    ledger: &mut FakeLedger,
    action: &str,
    body: &serde_json::Value,
) -> Option<Result<(), &'static str>> {
    let token = match body.get("token") {
        None => dollar(ledger),
        Some(code) => match code.as_str().and_then(IssuedAsset::by_code) {
            Some(token) => token,
            None => return Some(Err("no such token")),
        },
    };
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
    let pool = |ledger: &mut FakeLedger| {
        let token = match (body.get("currency"), body.get("issuer")) {
            (None, None) => token,
            (currency, _) => IssuedAsset {
                currency: currency
                    .and_then(serde_json::Value::as_str)
                    .and_then(Currency::parse)
                    .ok_or("no currency")?,
                issuer: account("issuer")?,
            },
        };
        let drops = body
            .get("drops")
            .and_then(serde_json::Value::as_str)
            .and_then(|drops| drops.parse::<u64>().ok())
            .ok_or("no drops")?;
        let fee = body
            .get("fee")
            .and_then(serde_json::Value::as_u64)
            .and_then(|fee| u32::try_from(fee).ok())
            .ok_or("no fee")?;
        ledger.set_pool(token, drops, value()?, fee)
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
        "amm" => pool(ledger),
        _ => return None,
    })
}

/// The dollar token of the fake's network, which a request to its issuer
/// is of unless it names another: DSUSD on the testnet, RLUSD on mainnet,
/// where only RLUSD runs (spec/decisions/mainnet-schemes.md).
fn dollar(ledger: &FakeLedger) -> IssuedAsset {
    if *ledger.network() == dassie_xrpl::network::MAINNET {
        IssuedAsset::RLUSD
    } else {
        IssuedAsset::DSUSD
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use dassie_xrpl::Wallet;
    use dassie_xrpl::api::account_flags;
    use dassie_xrpl::codec::{Signing, TF_SET_NO_RIPPLE, Transact, TrustSet};
    use dassie_xrpl::fake::{DSUSD_ISSUER, FEE_BASE, RLUSD_ISSUER};
    use serde_json::json;

    use super::*;

    fn value(text: &str) -> TokenValue {
        TokenValue::parse(text).unwrap()
    }

    /// A holder with a line to the issuer of `token`, as the scheme makes
    /// it.
    fn holder(ledger: &mut FakeLedger, token: IssuedAsset) -> AccountId {
        let wallet = Wallet::from_seed([1; 16]);
        ledger.deposit(wallet.account(), 100_000_000);
        ledger.close();
        let signed = TrustSet {
            account: wallet.account(),
            limit: token.amount(TokenValue::MAX),
            fee: FEE_BASE,
            // A new account's first sequence is the ledger it was made in.
            sequence: ledger.ledger(),
            last_ledger_sequence: Some(ledger.ledger() + 5),
            flags: TF_SET_NO_RIPPLE,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(wallet.public_key()),
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

    // A request that names no token is of DSUSD's issuer, the testnet's.
    #[test]
    fn the_issuer_mints_freezes_and_claws_back_on_request() {
        let mut ledger = FakeLedger::new();
        let holder = holder(&mut ledger, IssuedAsset::DSUSD);
        let address = holder.to_address();
        let held = |ledger: &FakeLedger| ledger.token_balance(holder, IssuedAsset::DSUSD);
        let theirs = |ledger: &FakeLedger| {
            let line = ledger
                .line(holder, DSUSD_ISSUER, IssuedAsset::DSUSD.currency)
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
        let flags = ledger.account_flags(DSUSD_ISSUER).unwrap();
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
    fn the_dsusd_issuer_mints_and_freezes_on_request() {
        let mut ledger = FakeLedger::new();
        let wallet = Wallet::from_seed([2; 16]);
        ledger.deposit(wallet.account(), 100_000_000);
        ledger.close();
        let signed = TrustSet {
            account: wallet.account(),
            limit: IssuedAsset::DSUSD.amount(TokenValue::MAX),
            fee: FEE_BASE,
            sequence: ledger.ledger(),
            last_ledger_sequence: Some(ledger.ledger() + 5),
            flags: TF_SET_NO_RIPPLE,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(wallet.public_key()),
        }
        .sign(&wallet);
        let submit =
            json!({ "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&signed.blob) });
        ledger.request(1, &serde_json::to_vec(&submit).unwrap());
        ledger.close();
        let address = wallet.account().to_address();
        let mint = json!({ "token": "DSUSD", "destination": address, "value": "7" });
        assert_eq!(ask(&mut ledger, "/tokens/mint", &mint), StatusCode::OK);
        assert_eq!(
            ledger.token_balance(wallet.account(), IssuedAsset::DSUSD),
            Some(value("7"))
        );
        assert_eq!(
            ledger.token_balance(wallet.account(), IssuedAsset::RLUSD),
            None
        );
        // RLUSD's issuer is mainnet's, which the testnet does not have.
        let rlusd = json!({ "token": "RLUSD", "destination": address, "value": "7" });
        assert_eq!(
            ask(&mut ledger, "/tokens/mint", &rlusd),
            StatusCode::BAD_REQUEST
        );
        let on = json!({ "token": "DSUSD", "account": address, "on": true });
        assert_eq!(ask(&mut ledger, "/tokens/freeze", &on), StatusCode::OK);
        let line = ledger
            .line(wallet.account(), DSUSD_ISSUER, IssuedAsset::DSUSD.currency)
            .unwrap();
        assert!(line.view(wallet.account()).unwrap().theirs.freeze);
        let unknown = json!({ "token": "EUR", "destination": address, "value": "7" });
        assert_eq!(
            ask(&mut ledger, "/tokens/mint", &unknown),
            StatusCode::BAD_REQUEST
        );
        assert!(ledger.tokens_conserved());
    }

    // On mainnet, a request that names no token is of RLUSD's issuer, at
    // its mainnet address; DSUSD is not there.
    #[test]
    fn on_mainnet_the_rlusd_issuer_takes_requests() {
        let mut ledger = FakeLedger::mainnet();
        let holder = holder(&mut ledger, IssuedAsset::RLUSD);
        let address = holder.to_address();
        let mint = json!({ "destination": address, "value": "3" });
        assert_eq!(ask(&mut ledger, "/tokens/mint", &mint), StatusCode::OK);
        assert_eq!(
            ledger.token_balance(holder, IssuedAsset::RLUSD),
            Some(value("3"))
        );
        assert!(
            ledger
                .line(holder, RLUSD_ISSUER, IssuedAsset::RLUSD.currency)
                .is_some()
        );
        let dsusd = json!({ "token": "DSUSD", "destination": address, "value": "3" });
        assert_eq!(
            ask(&mut ledger, "/tokens/mint", &dsusd),
            StatusCode::BAD_REQUEST
        );
        assert!(ledger.tokens_conserved());
    }

    #[test]
    fn the_issuer_makes_and_sets_a_pool_on_request() {
        let mut ledger = FakeLedger::new();
        let pool = json!({ "drops": "1000000000", "value": "500", "fee": 500 });
        assert_eq!(ask(&mut ledger, "/tokens/amm", &pool), StatusCode::OK);
        let state = ledger.pool(IssuedAsset::DSUSD).unwrap();
        assert_eq!(
            (state.drops, state.value, state.trading_fee),
            (1_000_000_000, value("500"), 500)
        );
        // Set again, naming the token.
        let again = json!({
            "drops": "2000000000",
            "value": "250.5",
            "fee": 0,
            "currency": IssuedAsset::DSUSD.currency.to_string(),
            "issuer": DSUSD_ISSUER.to_address(),
        });
        assert_eq!(ask(&mut ledger, "/tokens/amm", &again), StatusCode::OK);
        let state = ledger.pool(IssuedAsset::DSUSD).unwrap();
        assert_eq!(
            (state.drops, state.value, state.trading_fee),
            (2_000_000_000, value("250.5"), 0)
        );
        // Named by its code.
        let dsusd = json!({ "token": "DSUSD", "drops": "3000000000", "value": "4", "fee": 300 });
        assert_eq!(ask(&mut ledger, "/tokens/amm", &dsusd), StatusCode::OK);
        let state = ledger.pool(IssuedAsset::DSUSD).unwrap();
        assert_eq!(
            (state.drops, state.value, state.trading_fee),
            (3_000_000_000, value("4"), 300)
        );
        assert!(ledger.tokens_conserved());
        // Amounts and fees the ledger refuses, and issuers the fake is not.
        for refused in [
            json!({ "drops": "1000", "value": "1" }),
            json!({ "drops": 1000, "value": "1", "fee": 0 }),
            json!({ "drops": "0", "value": "1", "fee": 0 }),
            json!({ "drops": "1000", "value": "0", "fee": 0 }),
            json!({ "drops": "1000", "value": "1", "fee": 1001 }),
            json!({ "drops": "1000", "value": "1", "fee": 0, "currency": "USD" }),
            json!({
                "drops": "1000", "value": "1", "fee": 0,
                "currency": "USD", "issuer": AccountId([7; 20]).to_address(),
            }),
        ] {
            assert_eq!(
                ask(&mut ledger, "/tokens/amm", &refused),
                StatusCode::BAD_REQUEST,
                "{refused}"
            );
        }
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

    // Mainnet has no faucet: an account there is funded by a deposit from
    // outside, as an owner sends XRP, which either network takes.
    #[test]
    fn mainnet_takes_deposits_and_has_no_faucet() {
        let mut ledger = FakeLedger::mainnet();
        let someone = AccountId([7; 20]);
        let address = someone.to_address();
        let fund = json!({ "destination": address });
        assert_eq!(ask(&mut ledger, "/accounts", &fund), StatusCode::NOT_FOUND);
        assert_eq!(ledger.balance(someone), None);
        for refused in [
            json!({ "destination": address }),
            json!({ "destination": address, "drops": 5 }),
            json!({ "destination": address, "drops": "0" }),
            json!({ "destination": address, "drops": (MAX_DEPOSIT_DROPS + 1).to_string() }),
            json!({ "drops": "5" }),
        ] {
            assert_eq!(
                ask(&mut ledger, "/deposit", &refused),
                StatusCode::BAD_REQUEST,
                "{refused}"
            );
        }
        assert_eq!(ledger.queued(), 0);
        let deposit = json!({ "destination": address, "drops": "50000000" });
        assert_eq!(ask(&mut ledger, "/deposit", &deposit), StatusCode::OK);
        assert_eq!(ledger.balance(someone), Some(50_000_000));
        let mut testnet = FakeLedger::new();
        assert_eq!(ask(&mut testnet, "/deposit", &deposit), StatusCode::OK);
        assert_eq!(testnet.balance(someone), Some(50_000_000));
    }

    /// What the ledger answers `request` on connection 1.
    fn command(ledger: &mut FakeLedger, request: &serde_json::Value) -> serde_json::Value {
        let reply = ledger.request(1, &serde_json::to_vec(request).unwrap());
        serde_json::from_slice(&reply).unwrap()
    }

    #[test]
    fn the_list_is_served_and_the_validators_and_lies_change_on_request() {
        let mut ledger = FakeLedger::new();
        let (status, list) = answer(&mut ledger, "/vl", b"");
        assert_eq!(status, StatusCode::OK);
        let message = dassie_xrpl::validator_list::Message::parse(list.as_bytes()).unwrap();
        assert_eq!(message.publisher, ledger.publisher_key());
        // A validator's key rotates at once, and a list carries it once
        // published.
        let rotate = json!({ "validator": 0 });
        assert_eq!(
            ask(&mut ledger, "/validators/rotate", &rotate),
            StatusCode::OK
        );
        let master = ledger.validator_keys()[0].to_node_public();
        let manifest = command(
            &mut ledger,
            &json!({ "command": "manifest", "public_key": master }),
        );
        assert_eq!(manifest["result"]["details"]["seq"], 2);
        let publish = json!({ "expiration": 900_000_000 });
        assert_eq!(
            ask(&mut ledger, "/validators/publish", &publish),
            StatusCode::OK
        );
        let (_, list) = answer(&mut ledger, "/vl", b"");
        let message = dassie_xrpl::validator_list::Message::parse(list.as_bytes()).unwrap();
        let blob = dassie_xrpl::validator_list::List::parse(&message.blobs[0].blob).unwrap();
        assert_eq!((blob.sequence, blob.expiration), (2, 900_000_000));
        let withhold = json!({ "validator": 1, "on": true });
        assert_eq!(
            ask(&mut ledger, "/validators/withhold", &withhold),
            StatusCode::OK
        );
        let revoke = json!({ "validator": 2 });
        assert_eq!(
            ask(&mut ledger, "/validators/revoke", &revoke),
            StatusCode::OK
        );
        // Every connection lies, then is honest again.
        let proof = json!({ "command": "state_proof", "keys": [dassie_xrpl::hex_upper(&[1; 32])] });
        assert_eq!(
            ask(&mut ledger, "/lie", &json!({ "kind": "refusal" })),
            StatusCode::OK
        );
        assert_eq!(command(&mut ledger, &proof)["error"], "tooBusy");
        assert_eq!(
            ask(&mut ledger, "/lie", &json!({ "kind": "honest" })),
            StatusCode::OK
        );
        assert_eq!(command(&mut ledger, &proof)["status"], "success");
        for (path, body) in [
            ("/lie", json!({ "kind": "truth" })),
            ("/lie", json!({})),
            ("/validators/rotate", json!({ "validator": 9 })),
            ("/validators/withhold", json!({ "validator": 0 })),
            ("/validators/publish", json!({})),
        ] {
            assert_eq!(
                ask(&mut ledger, path, &body),
                StatusCode::BAD_REQUEST,
                "{path} {body}"
            );
        }
    }
}
