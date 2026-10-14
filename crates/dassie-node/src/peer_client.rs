//! Sending peer requests (spec 7 §4.7): `POST {url}/peer` over HTTPS to
//! public addresses only, with bounded concurrency and response sizes.
//!
//! The node owns one client, so its connection pool outlives restarts of
//! the engine. Whether a failure means the request surely did not arrive
//! matters for packets: only then is a packet rejected at once.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use dassie_node_core::address::is_public;
use dassie_peer::wire::{MESSAGE_CONTENT_TYPE, RESPONSE_CONTENT_TYPE};
use dassie_peer::{Failure, Request, Response};
use http::header::{ACCEPT, CONTENT_TYPE};
use http_body_util::{BodyExt, Full, Limited};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::dns::Name;
use hyper_util::rt::TokioExecutor;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::tls;

/// Time to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest response body read: a node list of 4096 IDs fits.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Most requests in flight, in total and to one host.
const MAX_IN_FLIGHT: usize = 128;
const MAX_PER_HOST: usize = 16;

/// Resolves names, keeping only addresses the node may send to.
#[derive(Clone, Debug)]
struct GuardedResolver {
    allow_private: bool,
}

type Addresses = std::vec::IntoIter<SocketAddr>;

impl tower_service::Service<Name> for GuardedResolver {
    type Response = Addresses;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Addresses, std::io::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let allow_private = self.allow_private;
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let allowed: Vec<SocketAddr> = found
                .filter(|address| allow_private || is_public(address.ip()))
                .collect();
            if allowed.is_empty() {
                return Err(std::io::Error::other("no public address"));
            }
            Ok(allowed.into_iter())
        })
    }
}

type Https = HyperClient<HttpsConnector<HttpConnector<GuardedResolver>>, Full<Bytes>>;

/// The node's peer client.
#[derive(Clone)]
pub struct PeerClient {
    http: Https,
    allow_private: bool,
    in_flight: Arc<Mutex<InFlight>>,
}

#[derive(Default)]
struct InFlight {
    total: usize,
    by_host: BTreeMap<String, usize>,
}

impl fmt::Debug for PeerClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PeerClient")
    }
}

/// A request's place in the bounds, given back when it ends.
struct Slot {
    in_flight: Arc<Mutex<InFlight>>,
    host: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        in_flight.total = in_flight.total.saturating_sub(1);
        if let Some(count) = in_flight.by_host.get_mut(&self.host) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_flight.by_host.remove(&self.host);
            }
        }
    }
}

impl PeerClient {
    /// A client trusting the platform's roots and, for dev and tests, the
    /// roots in `extra_roots`; refusing private addresses unless
    /// `allow_private`.
    ///
    /// # Errors
    ///
    /// The roots file cannot be read, or the platform's roots cannot be
    /// loaded.
    pub fn new(extra_roots: Option<&Path>, allow_private: bool) -> Result<Self, String> {
        let mut roots = Vec::new();
        if let Some(path) = extra_roots {
            for certificate in CertificateDer::pem_file_iter(path).map_err(|e| e.to_string())? {
                roots.push(certificate.map_err(|e| e.to_string())?);
            }
        }
        let verifier =
            rustls_platform_verifier::Verifier::new_with_extra_roots(roots, tls::provider())
                .map_err(|e| e.to_string())?;
        let config = rustls::ClientConfig::builder_with_provider(tls::provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        let mut http = HttpConnector::new_with_resolver(GuardedResolver { allow_private });
        http.set_connect_timeout(Some(CONNECT_TIMEOUT));
        http.enforce_http(false);
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_only()
            .enable_http1()
            .wrap_connector(http);
        let http = HyperClient::builder(TokioExecutor::new())
            .pool_max_idle_per_host(4)
            .build(connector);
        Ok(Self {
            http,
            allow_private,
            in_flight: Arc::new(Mutex::new(InFlight::default())),
        })
    }

    /// Takes a place for a request to `host`, if the bounds allow.
    fn slot(&self, host: &str) -> Option<Slot> {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let for_host = in_flight.by_host.get(host).copied().unwrap_or(0);
        if in_flight.total >= MAX_IN_FLIGHT || for_host >= MAX_PER_HOST {
            return None;
        }
        in_flight.total += 1;
        in_flight.by_host.insert(host.to_owned(), for_host + 1);
        Some(Slot {
            in_flight: Arc::clone(&self.in_flight),
            host: host.to_owned(),
        })
    }

    /// Sends `request` and reads its response.
    ///
    /// # Errors
    ///
    /// [`Failure::NotDelivered`] when the request surely did not reach the
    /// node: refused here, or the connection failed. [`Failure::Unknown`]
    /// when it may have: a timeout or a broken response.
    pub async fn send(&self, request: Request) -> Result<Response, Failure> {
        let url = format!("{}/peer", request.url);
        let host = request.url.strip_prefix("https://").unwrap_or_default();
        if !self.allow_private && literal_is_private(host) {
            return Err(Failure::NotDelivered);
        }
        let Some(_slot) = self.slot(host) else {
            return Err(Failure::NotDelivered);
        };
        let Ok(http_request) = http::Request::post(&url)
            .header(ACCEPT, MESSAGE_CONTENT_TYPE)
            .header(CONTENT_TYPE, MESSAGE_CONTENT_TYPE)
            .body(Full::new(Bytes::from(request.body)))
        else {
            return Err(Failure::NotDelivered);
        };
        let timeout = Duration::from_millis(request.timeout_ms.max(1));
        match tokio::time::timeout(timeout, self.exchange(http_request)).await {
            Ok(result) => result,
            Err(_) => Err(Failure::Unknown),
        }
    }

    async fn exchange(&self, request: http::Request<Full<Bytes>>) -> Result<Response, Failure> {
        let response = self.http.request(request).await.map_err(|error| {
            if error.is_connect() {
                Failure::NotDelivered
            } else {
                Failure::Unknown
            }
        })?;
        let status = response.status().as_u16();
        let content_type_ok = response
            .headers()
            .get(CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes() == RESPONSE_CONTENT_TYPE.as_bytes());
        let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|_| Failure::Unknown)?
            .to_bytes()
            .to_vec();
        if status == 200 && !content_type_ok {
            // Not a peer's answer: treat it as an error, which no status
            // says arrived.
            return Ok(Response {
                status: 502,
                body: Vec::new(),
            });
        }
        Ok(Response { status, body })
    }
}

/// Whether a URL's host is an IP literal that is not public. Names are
/// checked when resolved; literals never are.
fn literal_is_private(host: &str) -> bool {
    let without_port = match host.rsplit_once(':') {
        Some((name, port)) if !name.ends_with(']') || host.starts_with('[') => {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                name
            } else {
                host
            }
        }
        _ => host,
    };
    let literal = without_port
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(without_port);
    literal
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| !is_public(address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_literals_are_refused_before_sending() {
        assert!(literal_is_private("127.0.0.1:8443"));
        assert!(literal_is_private("[::1]:8443"));
        assert!(literal_is_private("10.0.0.1"));
        assert!(!literal_is_private("1.1.1.1:443"));
        assert!(!literal_is_private("example.com:443"));
        assert!(!literal_is_private("localhost"));
    }
}
