//! A front door (spec/areas/http.md#front-doors): the server that lets
//! ordinary Open Payments clients reach nodes without a public address, at
//! `<label>.<domain>`, and at `<name>.<domain>` for a name's holder.
//!
//! It is no node: it has no ledger and no wallet, and takes no peer
//! requests. It runs two listeners with a key of its own, and reads names
//! as a client of the peer protocol:
//!
//! - a QUIC endpoint ([`relay`]), which periphery nodes keep a connection
//!   to by naming it as their relay (`DASSIE_RELAY`,
//!   spec/areas/node.md#a-chosen-relay);
//! - an HTTPS listener ([`https`]) with a wildcard certificate, which
//!   passes Open Payments requests to the node a name's label names, or
//!   to the holder of the name it is, over relayed streams it opens on that
//!   node's connection ([`nodes`]), with TLS inside pinned to the node's
//!   key, and answers the root of a name with a static page of its own
//!   about the node ([`page`]); and on the relay's host, takes tunnels for
//!   QUIC at `/quic` into the QUIC endpoint, for the nodes whose UDP does
//!   not reach it;
//! - a poller ([`names`]) that reads the registrars' name lists over the
//!   same QUIC endpoint, and takes each name's holder from them as every
//!   node does.
//!
//! `dassie front-door` runs it.

pub mod certificate;
pub mod https;
pub mod key;
pub mod names;
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
pub use self::names::Registrars;
pub use self::settings::Settings;
use crate::clock::{Clock, SharedRng};
use crate::log::target;
use crate::outbound::Resolver;
use crate::quic::Quic;
use crate::quic::tunnel::TunnelBounds;
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
    /// The URL nodes dial its QUIC endpoint at, `https://host[:port]`:
    /// over a tunnel too, at `/quic` on that host and port over TCP.
    pub relay_url: String,
    /// The registrars whose name lists it reads.
    pub registrars: Registrars,
    /// Whether registrars may be at private addresses, such as `localhost`
    /// in dev and tests (`DASSIE_PEER_ALLOW_PRIVATE=1`).
    pub allow_private: bool,
    /// How long a tunnel taken may hold its place among the QUIC
    /// connections being set up: [`TunnelBounds::DEFAULT`] but in tests.
    pub tunnel_bounds: TunnelBounds,
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
/// The RNG makes its sessions with the registrars.
///
/// # Errors
///
/// The certificate does not load, or a socket cannot be used.
pub async fn serve(
    parts: Parts,
    sockets: Sockets,
    (clock, rng): (Arc<dyn Clock>, Box<dyn SharedRng>),
    shutdown: CancellationToken,
) -> Result<(), StartError> {
    let domain = parts.domain.as_str().to_string();
    let relay = https::relay_authority(&parts.relay_url).ok_or_else(|| {
        StartError(format!(
            "{:?} is not of the form https://host[:port]",
            parts.relay_url
        ))
    })?;
    let (stored, loaded) = parts
        .certificate
        .load(&domain)
        .map_err(|error| StartError(format!("the certificate cannot be served: {error}")))?;
    let slot = Arc::new(CertificateSlot::new(loaded));
    let config = tls::server_config(Arc::clone(&slot)).map_err(|e| StartError(e.to_string()))?;
    let key = Arc::new(parts.key);
    let (_identity, keys) = watch::channel(Some(Arc::clone(&key)));
    let quic = Quic::new(sockets.quic, keys, parts.allow_private)
        .map_err(|error| StartError(format!("the QUIC endpoint cannot start: {error}")))?
        .with_tunnel_bounds(parts.tunnel_bounds);
    let quic = Arc::new(quic);
    let resolver = Resolver::new(parts.allow_private);
    sockets
        .https
        .set_nonblocking(true)
        .map_err(|e| StartError(e.to_string()))?;
    let listener = tokio::net::TcpListener::from_std(sockets.https)
        .map_err(|error| StartError(format!("the HTTPS listener cannot start: {error}")))?;
    let names = Arc::new(names::Names::new(&parts.registrars));
    let poller = names::Poller::new(
        Arc::clone(&names),
        parts.registrars,
        (Arc::clone(&quic), key),
        resolver,
        Arc::clone(&clock),
        rng,
    );
    let nodes = Arc::new(nodes::Nodes::new(
        Arc::clone(&quic),
        Arc::clone(&clock),
        shutdown.clone(),
    ));
    let door = Arc::new(https::Door::new(
        &domain,
        (relay, Arc::clone(&quic)),
        (nodes, names),
        clock,
    ));
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
        poller.run(shutdown.clone()),
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
    (clock, rng): (Arc<dyn Clock>, Box<dyn SharedRng>),
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
    let relay_url = settings.relay_url();
    let entry = key::relay_entry(&key, &relay_url);
    tracing::info!(target: target::FRONT_DOOR, "nodes name this front door as their relay with DASSIE_RELAY='{entry}'");
    for variable in settings.exposed_dev_flags() {
        tracing::warn!(target: target::FRONT_DOOR, "{variable} is set: it is for development; unset it on a public front door");
    }
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
        relay_url,
        registrars: settings.registrars(),
        allow_private: settings.peer_allow_private,
        tunnel_bounds: TunnelBounds::DEFAULT,
    };
    serve(parts, Sockets { https, quic }, (clock, rng), shutdown).await
}
