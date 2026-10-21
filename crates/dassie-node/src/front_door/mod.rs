//! A front door (spec/areas/http.md#front-doors): the server that lets
//! ordinary Open Payments clients reach nodes without a public address, at
//! `<label>.<domain>`.
//!
//! It is no node: it has no ledger, no peer protocol and no wallet. It
//! runs two listeners with a key of its own:
//!
//! - a QUIC endpoint ([`relay`]), which periphery nodes keep a connection
//!   to by naming it as their relay (`DASSIE_RELAY`,
//!   spec/areas/node.md#a-chosen-relay);
//! - an HTTPS listener ([`https`]) with a wildcard certificate, which
//!   passes Open Payments requests to the node a name's label names, over
//!   relayed streams it opens on that node's connection ([`nodes`]), with
//!   TLS inside pinned to the node's key, and answers the root of a name
//!   with a static page of its own about the node ([`page`]).
//!
//! `dassie front-door` runs it.

pub mod certificate;
pub mod https;
pub mod key;
pub mod nodes;
pub mod page;
pub mod relay;
pub mod settings;

use std::fmt;
use std::sync::Arc;

use dassie_identity::NodeKey;
use dassie_node_core::config::Hostname;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub use self::certificate::CertificateFiles;
pub use self::settings::Settings;
use crate::clock::Clock;
use crate::log::target;
use crate::quic::Quic;
use crate::tls::{self, CertificateSlot};

/// Why a front door could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartError(pub String);

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StartError {}

/// What a front door serves with.
#[derive(Debug)]
pub struct Parts {
    /// The domain whose names it serves.
    pub domain: Hostname,
    /// Its own key.
    pub key: NodeKey,
    /// Its certificate for `*.<domain>`.
    pub certificate: CertificateFiles,
}

/// Its sockets, bound already.
#[derive(Debug)]
pub struct Sockets {
    /// The HTTPS listener.
    pub https: std::net::TcpListener,
    /// The QUIC endpoint's socket.
    pub quic: std::net::UdpSocket,
}

/// Serves as a front door until `shutdown`, then drains both listeners.
///
/// # Errors
///
/// The certificate does not load, or a socket cannot be used.
pub async fn serve(
    parts: Parts,
    sockets: Sockets,
    clock: Arc<dyn Clock>,
    shutdown: CancellationToken,
) -> Result<(), StartError> {
    let domain = parts.domain.as_str().to_string();
    let (stored, loaded) = parts
        .certificate
        .load(&domain)
        .map_err(|error| StartError(format!("the certificate cannot be served: {error}")))?;
    let slot = Arc::new(CertificateSlot::new(loaded));
    let config = tls::server_config(Arc::clone(&slot)).map_err(|e| StartError(e.to_string()))?;
    let (_identity, keys) = watch::channel(Some(Arc::new(parts.key)));
    let quic = Quic::new(sockets.quic, keys, false)
        .map_err(|error| StartError(format!("the QUIC endpoint cannot start: {error}")))?;
    let quic = Arc::new(quic);
    sockets
        .https
        .set_nonblocking(true)
        .map_err(|e| StartError(e.to_string()))?;
    let listener = tokio::net::TcpListener::from_std(sockets.https)
        .map_err(|error| StartError(format!("the HTTPS listener cannot start: {error}")))?;
    let nodes = Arc::new(nodes::Nodes::new(Arc::clone(&quic), shutdown.clone()));
    let door = Arc::new(https::Door::new(&domain, nodes, clock));
    tracing::info!(target: target::FRONT_DOOR, %domain, "front door serving");
    let reloading = tokio::spawn(certificate::keep_current(
        parts.certificate,
        domain,
        slot,
        stored,
        shutdown.clone(),
    ));
    tokio::join!(
        Arc::clone(&quic).serve(Arc::new(relay::Relay), shutdown.clone()),
        https::serve(door, listener, config, shutdown.clone()),
    );
    let _ = reloading.await;
    Ok(())
}

/// Runs a front door with `settings` until `shutdown`: reads its key and
/// certificate, binds its listeners and serves.
///
/// # Errors
///
/// Something it needs is missing or cannot be used.
pub async fn run(
    settings: &Settings,
    clock: Arc<dyn Clock>,
    shutdown: CancellationToken,
) -> Result<(), StartError> {
    let missing =
        |what: &str, variable: &str| StartError(format!("no {what}: set {variable} or its flag"));
    let key_file = settings
        .key_file
        .as_ref()
        .ok_or_else(|| missing("key file", "DASSIE_FRONT_DOOR_KEY_FILE"))?;
    let certificate = CertificateFiles {
        chain: settings
            .certificate
            .clone()
            .ok_or_else(|| missing("certificate", "DASSIE_FRONT_DOOR_CERTIFICATE"))?,
        key: settings
            .certificate_key
            .clone()
            .ok_or_else(|| missing("certificate key", "DASSIE_FRONT_DOOR_CERTIFICATE_KEY"))?,
    };
    let key = key::read(key_file).map_err(StartError)?;
    let entry = key::relay_entry(&key, &settings.relay_url());
    tracing::info!(target: target::FRONT_DOOR, "nodes name this front door as their relay with DASSIE_RELAY='{entry}'");
    let https = std::net::TcpListener::bind(settings.https_listen).map_err(|error| {
        StartError(format!(
            "cannot listen on {}: {error}",
            settings.https_listen
        ))
    })?;
    let quic = std::net::UdpSocket::bind(settings.quic_listen).map_err(|error| {
        StartError(format!(
            "cannot listen on {}: {error}",
            settings.quic_listen
        ))
    })?;
    let parts = Parts {
        domain: settings.domain.clone(),
        key,
        certificate,
    };
    serve(parts, Sockets { https, quic }, clock, shutdown).await
}
