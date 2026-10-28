//! Who is at the other end of a QUIC connection: each side presents a
//! self-signed certificate for its Ed25519 node key, with its SLH-DSA key in
//! an extension of its own, and the other checks the two against a node
//! ID, not a certificate authority (spec/areas/peer-protocol.md#transport).
//!
//! A dialler knows the node ID it wants, and refuses a server whose keys do
//! not hash to it. A server takes any client that proves a key, and learns
//! the client's node ID from its keys. The TLS handshake signs with the node
//! key; its signatures start with TLS's own context string, so they can
//! never pass for a link state's. Nothing proves the SLH-DSA key's secret:
//! a certificate that names another SLH-DSA key names another node ID.

use std::sync::{Arc, Mutex, PoisonError};

use dassie_identity::{
    IdentityKey, NodeId, NodeIdentity, PublicKey, SLH_DSA_PUBLIC_KEY_BYTES, SlhDsaPublicKey,
};
use dassie_oer::oid;
use rcgen::{CertificateParams, CustomExtension, DnType, KeyPair, PKCS_ED25519};
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
    identity: watch::Receiver<Option<Arc<IdentityKey>>>,
    made: Mutex<Option<(NodeId, Arc<CertifiedKey>)>>,
}

impl Keys {
    /// Certificates for the node key that `identity` holds, once set up.
    #[must_use]
    pub fn new(identity: watch::Receiver<Option<Arc<IdentityKey>>>) -> Self {
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

/// The arc of the certificate extension that carries a node's SLH-DSA
/// public key, `…1399.1.7` (spec/areas/identity.md#object-identifiers).
const SLH_DSA_EXTENSION_ARC: [u64; 11] = [1, 3, 6, 1, 4, 1, 54392, 5, 1399, 1, 7];

/// The same OID in OER, a length byte and then the DER content octets that
/// x509-parser holds an extension's OID as.
const SLH_DSA_EXTENSION_OID: [u8; 14] = oid::encode(&SLH_DSA_EXTENSION_ARC);

/// The extension's value: the DER of `OCTET STRING (SIZE (32))`.
const SLH_DSA_EXTENSION_PREFIX: [u8; 2] = [0x04, 0x20];

/// A self-signed certificate for `key`, with the key to sign handshakes,
/// and the node's SLH-DSA key in its extension.
///
/// # Errors
///
/// Making or loading the key failed, which a valid node key never does.
pub fn certify(key: &IdentityKey) -> Result<CertifiedKey, String> {
    let der = key.to_pkcs8_der();
    let pkcs8 = PrivatePkcs8KeyDer::from(der.as_bytes().as_slice());
    let pair =
        KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8, &PKCS_ED25519).map_err(|e| e.to_string())?;
    let mut params =
        CertificateParams::new(vec![CERTIFICATE_NAME.to_owned()]).map_err(|e| e.to_string())?;
    params
        .distinguished_name
        .push(DnType::CommonName, key.node_id().to_string());
    let mut value = SLH_DSA_EXTENSION_PREFIX.to_vec();
    value.extend_from_slice(key.identity().slh_dsa().as_bytes());
    params
        .custom_extensions
        .push(CustomExtension::from_oid_content(
            &SLH_DSA_EXTENSION_ARC,
            value,
        ));
    let certificate = params.self_signed(&pair).map_err(|e| e.to_string())?;
    let signing = rustls::crypto::ring::sign::any_eddsa_type(&pkcs8).map_err(|e| e.to_string())?;
    Ok(CertifiedKey::new(vec![certificate.der().clone()], signing))
}

/// The node a certificate is for: its Ed25519 key, which is not of small
/// order, and the SLH-DSA key in its one extension for it.
#[must_use]
pub fn node_identity_of(certificate: &CertificateDer<'_>) -> Option<NodeIdentity> {
    let (rest, parsed) = x509_parser::parse_x509_certificate(certificate).ok()?;
    if !rest.is_empty() {
        return None;
    }
    let key = PublicKey::from_spki_der(parsed.tbs_certificate.subject_pki.raw).ok()?;
    if key.is_weak() {
        return None;
    }
    let oid = SLH_DSA_EXTENSION_OID.get(1..)?;
    let mut values = parsed
        .tbs_certificate
        .extensions()
        .iter()
        .filter(|extension| extension.oid.as_bytes() == oid)
        .map(|extension| extension.value);
    let (Some(value), None) = (values.next(), values.next()) else {
        return None;
    };
    let bytes: [u8; SLH_DSA_PUBLIC_KEY_BYTES] = value
        .strip_prefix(&SLH_DSA_EXTENSION_PREFIX)?
        .try_into()
        .ok()?;
    Some(NodeIdentity::new(key, SlhDsaPublicKey::from_bytes(bytes)))
}

/// The node at the other end of an established connection, from the
/// certificate it presented.
#[must_use]
pub fn peer_node(connection: &quinn::Connection) -> Option<NodeId> {
    peer_identity(connection).map(|identity| identity.node_id())
}

/// The identity at the other end of an established connection, from the
/// certificate it presented.
#[must_use]
pub fn peer_identity(connection: &quinn::Connection) -> Option<NodeIdentity> {
    let identity = connection.peer_identity()?;
    let chain = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    let [leaf] = chain.as_slice() else {
        return None;
    };
    node_identity_of(leaf)
}

/// The identity of a single self-signed certificate, or why it is refused.
fn presented_identity(
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> Result<NodeIdentity, Error> {
    if !intermediates.is_empty() {
        return Err(Error::General("a node presents one certificate".into()));
    }
    node_identity_of(end_entity).ok_or(Error::InvalidCertificate(
        rustls::CertificateError::BadEncoding,
    ))
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    tls::provider().signature_verification_algorithms
}

/// Accepts a server only if its keys are the node's that was asked for.
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
        let identity = presented_identity(end_entity, intermediates)?;
        if identity.node_id() != self.expected {
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
        presented_identity(end_entity, intermediates)?;
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

    fn key(byte: u8) -> IdentityKey {
        dassie_identity::TableKeygen::identity_key(&[byte; 32])
    }

    #[test]
    fn a_certificate_names_its_node_key() {
        let key = key(1);
        let certified = certify(&key).unwrap();
        let [leaf] = certified.cert.as_slice() else {
            panic!("one certificate");
        };
        assert_eq!(node_identity_of(leaf), Some(key.identity()));
    }

    // The extension's OID is the arc's, and its value the key's 32 bytes in
    // an OCTET STRING: pinned, since every node reads it.
    #[test]
    fn the_extension_is_pinned() {
        assert_eq!(
            SLH_DSA_EXTENSION_OID,
            [
                0x0d, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xa8, 0x78, 0x05, 0x8a, 0x77, 0x01, 0x07
            ]
        );
        let key = key(1);
        let certified = certify(&key).unwrap();
        let der = certified.cert[0].as_ref();
        let mut extension = vec![0x06, 0x0d];
        extension.extend_from_slice(&SLH_DSA_EXTENSION_OID[1..]);
        extension.extend_from_slice(&[0x04, 0x22, 0x04, 0x20]);
        extension.extend_from_slice(key.identity().slh_dsa().as_bytes());
        assert!(
            der.windows(extension.len())
                .any(|window| window == extension)
        );
    }

    // A certificate without the SLH-DSA key, or with two, names no node.
    #[test]
    fn a_certificate_needs_one_slh_dsa_key() {
        let key = key(1);
        let der = key.to_pkcs8_der();
        let pkcs8 = PrivatePkcs8KeyDer::from(der.as_bytes().as_slice());
        let pair = KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8, &PKCS_ED25519).unwrap();
        let bare = CertificateParams::new(vec!["dassie".to_owned()]).unwrap();
        let certificate = bare.self_signed(&pair).unwrap();
        assert_eq!(node_identity_of(certificate.der()), None);
        let mut twice = CertificateParams::new(vec!["dassie".to_owned()]).unwrap();
        let mut value = SLH_DSA_EXTENSION_PREFIX.to_vec();
        value.extend_from_slice(key.identity().slh_dsa().as_bytes());
        for _ in 0..2 {
            twice
                .custom_extensions
                .push(CustomExtension::from_oid_content(
                    &SLH_DSA_EXTENSION_ARC,
                    value.clone(),
                ));
        }
        // rcgen may refuse a repeated extension itself; if it writes one,
        // it names no node.
        if let Ok(certificate) = twice.self_signed(&pair) {
            assert_eq!(node_identity_of(certificate.der()), None);
        }
    }

    // A server that proves the node's own Ed25519 key but names a foreign
    // SLH-DSA key in its certificate is another node ID, and refused.
    #[test]
    fn a_foreign_slh_dsa_key_is_another_node() {
        let wanted = key(1);
        let der = wanted.to_pkcs8_der();
        let pkcs8 = PrivatePkcs8KeyDer::from(der.as_bytes().as_slice());
        let pair = KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8, &PKCS_ED25519).unwrap();
        let mut params = CertificateParams::new(vec!["dassie".to_owned()]).unwrap();
        let mut value = SLH_DSA_EXTENSION_PREFIX.to_vec();
        value.extend_from_slice(&[9; 32]);
        params
            .custom_extensions
            .push(CustomExtension::from_oid_content(
                &SLH_DSA_EXTENSION_ARC,
                value,
            ));
        let certificate = params.self_signed(&pair).unwrap();
        let identity = node_identity_of(certificate.der()).unwrap();
        assert_eq!(identity.key(), wanted.public_key());
        assert_ne!(identity.node_id(), wanted.node_id());
        let verifier = PinnedServer::new(wanted.node_id());
        let name = ServerName::try_from("dassie").unwrap();
        let refused =
            verifier.verify_server_cert(certificate.der(), &[], &name, &[], UnixTime::now());
        assert!(refused.is_err());
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
        assert_eq!(node_identity_of(certificate.der()), None);
    }
}
