//! A source of exchange rates over HTTPS, in Coinbase's format, whose
//! answer a test changes.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use dassie_node::store::StoredCertificate;
use dassie_node::tls;
use dassie_node_core::certificate::CertificateSource;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::pair::{CERT, KEY};

/// What the source answers: a status and a body.
type Answer = Arc<Mutex<(u16, String)>>;

/// The source, served until dropped.
pub struct RatesServer {
    pub address: SocketAddr,
    answer: Answer,
    /// The requests it answered.
    asked: Arc<AtomicUsize>,
    cancel: CancellationToken,
}

/// A Coinbase-format answer, in dollars.
pub fn coinbase(rates: &[(&str, &str)]) -> String {
    let rates: Vec<String> = rates
        .iter()
        .map(|(code, rate)| format!(r#""{code}":"{rate}""#))
        .collect();
    format!(
        r#"{{"data":{{"currency":"USD","rates":{{{}}}}}}}"#,
        rates.join(",")
    )
}

impl RatesServer {
    pub async fn start(body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let certificate = StoredCertificate {
            source: CertificateSource::Manual,
            chain_pem: std::fs::read_to_string(CERT).unwrap(),
            key_pem: std::fs::read_to_string(KEY).unwrap(),
        };
        let slot = Arc::new(tls::CertificateSlot::new(tls::load(&certificate).unwrap()));
        let acceptor = tokio_rustls::TlsAcceptor::from(tls::server_config(slot).unwrap());
        let answer: Answer = Arc::new(Mutex::new((200, body)));
        let asked = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let (served, counted, stop) = (Arc::clone(&answer), Arc::clone(&asked), cancel.clone());
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    () = stop.cancelled() => return,
                    accepted = listener.accept() => accepted,
                };
                let Ok((stream, _)) = accepted else { continue };
                let (acceptor, served) = (acceptor.clone(), Arc::clone(&served));
                let counted = Arc::clone(&counted);
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let service = service_fn(move |_| {
                        let (status, body) = served.lock().unwrap().clone();
                        counted.fetch_add(1, Ordering::SeqCst);
                        let mut response = Response::new(Full::new(Bytes::from(body)));
                        *response.status_mut() = StatusCode::from_u16(status).unwrap();
                        async move { Ok::<_, Infallible>(response) }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), service)
                        .await;
                });
            }
        });
        Self {
            address,
            answer,
            asked,
            cancel,
        }
    }

    /// Where it answers.
    pub fn url(&self) -> String {
        format!("https://localhost:{}/rates.json", self.address.port())
    }

    /// Answers this from now on.
    pub fn answer(&self, status: u16, body: String) {
        *self.answer.lock().unwrap() = (status, body);
    }

    /// How many requests it answered.
    pub fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl Drop for RatesServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
