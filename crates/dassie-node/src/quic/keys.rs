//! Who is at the other end of a QUIC connection: each side presents a
//! self-signed certificate for its Ed25519 node key, and the other checks
//! that key, not a certificate authority
//! (spec/areas/peer-protocol.md#transport).
//!
//! A dialler knows the node ID it wants, and refuses a server whose key does
//! not hash to it. A server takes any client that proves a key, and learns
//! the client's node ID from it. The TLS handshake signs with the node key;
//! its signatures start with TLS's own context string, so they can never
//! pass for a link state's.

use std::sync::{Arc, Mutex, PoisonError};

use dassie_identity::{NodeId, NodeKey, PublicKey};
use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ED25519};
use rustls::DistinguishedName;
use rustls::client::ResolvesClientCert;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use tokio::sync::watch;

use crate::tls;

/// The name a node's certificate carries. Nobody checks it: the key is
/// what counts.
const CERTIFICATE_NAME: &str = "dassie";

/// The node key's certificate, made when the key first appears.
#[derive(Debug)]
pub struct Keys {
    identity: watch::Receiver<Option<Arc<NodeKey>>>,
    made: Mutex<Option<(NodeId, Arc<CertifiedKey>)>>,
}

impl Keys {
    /// Certificates for the node key that `identity` holds, once set up.
    #[must_use]
    pub fn new(identity: watch::Receiver<Option<Arc<NodeKey>>>) -> Self {
        Self {
            identity,
            made: Mutex::new(None),
        }
    }

    /// The node's ID and certificate, once it has a key.
    #[must_use]
    pub fn current(&self) -> Option<(NodeId, Arc<CertifiedKey>)> {
        let key = self.identity.borrow().clone()?;
        let id = key.node_id();
        let mut made = self.made.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((made_for, certified)) = made.as_ref()
            && *made_for == id
        {
            return Some((id, Arc::clone(certified)));
        }
        let certified = match certify(&key) {
            Ok(certified) => Arc::new(certified),
            Err(error) => {
                tracing::error!(target: crate::log::target::PEER, %error, "no certificate for QUIC");
                return None;
            }
        };
        *made = Some((id, Arc::clone(&certified)));
        Some((id, certified))
    }
}

/// A self-signed certificate for `key`, with the key to sign handshakes.
///
/// # Errors
///
/// Making or loading the key failed, which a valid node key never does.
pub fn certify(key: &NodeKey) -> Result<CertifiedKey, String> {
    let der = key.to_pkcs8_der();
    let pkcs8 = PrivatePkcs8KeyDer::from(der.as_bytes().as_slice());
    let pair =
        KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8, &PKCS_ED25519).map_err(|e| e.to_string())?;
    let mut params =
        CertificateParams::new(vec![CERTIFICATE_NAME.to_owned()]).map_err(|e| e.to_string())?;
    params
        .distinguished_name
        .push(DnType::CommonName, key.node_id().to_string());
    let certificate = params.self_signed(&pair).map_err(|e| e.to_string())?;
    let signing = rustls::crypto::ring::sign::any_eddsa_type(&pkcs8).map_err(|e| e.to_string())?;
    Ok(CertifiedKey::new(vec![certificate.der().clone()], signing))
}

/// The node key a certificate is for: an Ed25519 key, which is not of
/// small order.
#[must_use]
pub fn node_key_of(certificate: &CertificateDer<'_>) -> Option<PublicKey> {
    let (rest, parsed) = x509_parser::parse_x509_certificate(certificate).ok()?;
    if !rest.is_empty() {
        return None;
    }
    let key = PublicKey::from_spki_der(parsed.tbs_certificate.subject_pki.raw).ok()?;
    (!key.is_weak()).then_some(key)
}

/// The node at the other end of an established connection, from the
/// certificate it presented.
#[must_use]
pub fn peer_node(connection: &quinn::Connection) -> Option<NodeId> {
    peer_key(connection).map(|key| key.node_id())
}

/// The node key at the other end of an established connection, from the
/// certificate it presented.
#[must_use]
pub fn peer_key(connection: &quinn::Connection) -> Option<PublicKey> {
    let identity = connection.peer_identity()?;
    let chain = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    let [leaf] = chain.as_slice() else {
        return None;
    };
    node_key_of(leaf)
}

/// The key of a single self-signed certificate, or why it is refused.
fn presented_key(
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> Result<PublicKey, Error> {
    if !intermediates.is_empty() {
        return Err(Error::General("a node presents one certificate".into()));
    }
    node_key_of(end_entity).ok_or(Error::InvalidCertificate(
        rustls::CertificateError::BadEncoding,
    ))
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    tls::provider().signature_verification_algorithms
}

/// Accepts a server only if its key is the node's that was asked for.
#[derive(Debug)]
pub struct PinnedServer {
    expected: NodeId,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedServer {
    /// A verifier for the node `expected`.
    #[must_use]
    pub fn new(expected: NodeId) -> Self {
        Self {
            expected,
            algorithms: algorithms(),
        }
    }
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let key = presented_key(end_entity, intermediates)?;
        if key.node_id() != self.expected {
            return Err(Error::InvalidCertificate(
                rustls::CertificateError::NotValidForName,
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        // QUIC is TLS 1.3 only.
        Err(Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

/// Accepts any client that proves a node key; the server learns which from
/// the connection ([`peer_node`]).
#[derive(Debug)]
pub struct AnyNodeKey {
    algorithms: WebPkiSupportedAlgorithms,
}

impl AnyNodeKey {
    /// The verifier.
    #[must_use]
    pub fn new() -> Self {
        Self {
            algorithms: algorithms(),
        }
    }
}

impl Default for AnyNodeKey {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientCertVerifier for AnyNodeKey {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        presented_key(end_entity, intermediates)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

/// Presents the node's certificate, as server and as client.
#[derive(Debug)]
pub struct Present(pub Arc<Keys>);

impl ResolvesServerCert for Present {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.0.current().map(|(_, certified)| certified)
    }
}

impl ResolvesClientCert for Present {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        if !sigschemes.contains(&SignatureScheme::ED25519) {
            return None;
        }
        self.0.current().map(|(_, certified)| certified)
    }

    fn has_certs(&self) -> bool {
        self.0.current().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> NodeKey {
        NodeKey::from_bytes(&[byte; 32])
    }

    #[test]
    fn a_certificate_names_its_node_key() {
        let key = key(1);
        let certified = certify(&key).unwrap();
        let [leaf] = certified.cert.as_slice() else {
            panic!("one certificate");
        };
        assert_eq!(node_key_of(leaf), Some(key.public_key()));
    }

    #[test]
    fn a_server_with_another_key_is_refused() {
        let wanted = key(1);
        let other = certify(&key(2)).unwrap();
        let verifier = PinnedServer::new(wanted.node_id());
        let name = ServerName::try_from("dassie").unwrap();
        let refused = verifier.verify_server_cert(&other.cert[0], &[], &name, &[], UnixTime::now());
        assert!(refused.is_err());
        let right = certify(&wanted).unwrap();
        assert!(
            verifier
                .verify_server_cert(&right.cert[0], &[], &name, &[], UnixTime::now())
                .is_ok()
        );
    }

    #[test]
    fn a_chain_is_refused() {
        let certified = certify(&key(1)).unwrap();
        let verifier = AnyNodeKey::new();
        let chain = [certified.cert[0].clone()];
        assert!(
            verifier
                .verify_client_cert(&certified.cert[0], &chain, UnixTime::now())
                .is_err()
        );
    }

    #[test]
    fn a_certificate_that_is_not_ed25519_is_refused() {
        let pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let certificate = CertificateParams::new(vec!["dassie".to_owned()])
            .unwrap()
            .self_signed(&pair)
            .unwrap();
        assert_eq!(node_key_of(certificate.der()), None);
    }
}
