//! The fake XRP Ledger of `dassie-xrpl`, served as rippled serves the real
//! one: its WebSocket API on `ws://`, its faucet on HTTPS, and a ledger
//! closed every so often. For tests and dev environments only (feature
//! `testing`): CI and `node --run dev -- --xrpl fake` settle against it.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use dassie_xrpl::AccountId;
use dassie_xrpl::fake::FakeLedger;
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
}

impl Server {
    /// Starts serving.
    ///
    /// # Errors
    ///
    /// A listener cannot bind, or the certificate is invalid.
    pub async fn start(options: Options) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            ledger: Mutex::new(FakeLedger::new()),
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
        let faucet_url = match options.faucet {
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
                Some(format!("https://localhost:{}/accounts", address.port()))
            }
            None => None,
        };
        Ok(Self {
            shared,
            cancel,
            url: format!("ws://{address}"),
            faucet_url,
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
            .close();
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
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(stream).await else {
                return;
            };
            let service = service_fn(move |request| {
                let shared = Arc::clone(&shared);
                async move { Ok::<_, Infallible>(fund(&shared, request).await) }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(tls), service)
                .await;
        });
    }
}

/// The faucet: `{"destination": "r…"}` gets 1000 test XRP in the next
/// ledger.
async fn fund(shared: &Shared, request: Request<hyper::body::Incoming>) -> Response<Full<Bytes>> {
    let respond = |status: StatusCode, body: &str| {
        let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
        *response.status_mut() = status;
        response
    };
    let Ok(body) = Limited::new(request.into_body(), 4096).collect().await else {
        return respond(StatusCode::BAD_REQUEST, "{}");
    };
    let destination = serde_json::from_slice::<serde_json::Value>(&body.to_bytes())
        .ok()
        .and_then(|value| {
            value
                .get("destination")?
                .as_str()
                .and_then(AccountId::parse)
        });
    let Some(destination) = destination else {
        return respond(StatusCode::BAD_REQUEST, r#"{"error":"no destination"}"#);
    };
    shared
        .ledger
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .fund(destination);
    respond(
        StatusCode::OK,
        &format!(r#"{{"account":{{"address":"{destination}"}}}}"#),
    )
}
