//! Certificates: reading and checking them, making the self-signed one, and
//! serving whichever is current.
//!
//! The HTTPS listener always has a certificate. It reads the current one
//! from a [`CertificateSlot`] at every handshake, so a renewal or an
//! installed certificate takes effect at once, without restarting the
//! listener (the original restarted its server, which raced on the port,
//! spec 2 §2.2).
//!
//! Installing a certificate checks it first: the chain parses, the key is
//! the leaf's, and the leaf is valid now. The original stored whatever the
//! RPC sent and then failed at the next TLS handshake (RT-12).

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use dassie_node_core::certificate::{self, CertificateInfo, CertificateSource};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::ServerConfig;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use x509_parser::extensions::GeneralName;

use crate::store::StoredCertificate;

/// Longest PEM accepted for a chain or a key, in bytes.
pub const MAX_PEM_BYTES: usize = 64 * 1024;

/// The only crypto provider: ring (see the manifest).
#[must_use]
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A certificate could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TlsError {
    /// The PEM is too long, or holds no certificate or no key.
    Pem(String),
    /// The leaf certificate could not be parsed.
    Certificate(String),
    /// The key is unusable, or not the leaf's.
    Key(String),
    /// The leaf is not valid at this time.
    NotValidNow,
    /// Making a certificate failed.
    Generate(String),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pem(problem) => write!(f, "invalid PEM: {problem}"),
            Self::Certificate(problem) => write!(f, "invalid certificate: {problem}"),
            Self::Key(problem) => write!(f, "invalid private key: {problem}"),
            Self::NotValidNow => f.write_str("the certificate is not valid at this time"),
            Self::Generate(problem) => write!(f, "could not make a certificate: {problem}"),
        }
    }
}

impl std::error::Error for TlsError {}

/// A certificate ready to serve, with what the node knows about it.
#[derive(Clone, Debug)]
pub struct LoadedCertificate {
    /// Its names, validity and source.
    pub info: CertificateInfo,
    /// The chain and key for rustls.
    pub key: Arc<CertifiedKey>,
    /// The PEM it came from, for storing.
    pub stored: StoredCertificate,
}

/// Reads a stored certificate and checks that its key matches.
pub fn load(stored: &StoredCertificate) -> Result<LoadedCertificate, TlsError> {
    if stored.chain_pem.len() > MAX_PEM_BYTES || stored.key_pem.len() > MAX_PEM_BYTES {
        return Err(TlsError::Pem("longer than 64 KiB".into()));
    }
    let chain = CertificateDer::pem_slice_iter(stored.chain_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| TlsError::Pem(error.to_string()))?;
    let leaf = chain
        .first()
        .ok_or_else(|| TlsError::Pem("no certificate".into()))?;
    let info = inspect(leaf, stored.source)?;
    let key = PrivateKeyDer::from_pem_slice(stored.key_pem.as_bytes())
        .map_err(|error| TlsError::Pem(error.to_string()))?;
    let signing_key = provider()
        .key_provider
        .load_private_key(key)
        .map_err(|error| TlsError::Key(error.to_string()))?;
    let certified = CertifiedKey::new(chain, signing_key);
    certified
        .keys_match()
        .map_err(|error| TlsError::Key(error.to_string()))?;
    Ok(LoadedCertificate {
        info,
        key: Arc::new(certified),
        stored: stored.clone(),
    })
}

/// Like [`load`], and also requires the leaf to be valid at `now_ms`. For
/// certificates the owner installs.
pub fn load_valid(stored: &StoredCertificate, now_ms: u64) -> Result<LoadedCertificate, TlsError> {
    let loaded = load(stored)?;
    if loaded.info.not_before_ms <= now_ms && now_ms < loaded.info.not_after_ms {
        Ok(loaded)
    } else {
        Err(TlsError::NotValidNow)
    }
}

/// Reads a leaf's names and validity.
fn inspect(
    leaf: &CertificateDer<'_>,
    source: CertificateSource,
) -> Result<CertificateInfo, TlsError> {
    let invalid = |error: String| TlsError::Certificate(error);
    let (_, certificate) =
        x509_parser::parse_x509_certificate(leaf.as_ref()).map_err(|e| invalid(e.to_string()))?;
    let validity = certificate.validity();
    let mut names = vec![];
    if let Some(extension) = certificate
        .subject_alternative_name()
        .map_err(|e| invalid(e.to_string()))?
    {
        for name in &extension.value.general_names {
            match name {
                GeneralName::DNSName(name) => names.push(name.to_ascii_lowercase()),
                GeneralName::IPAddress(bytes) => names.extend(ip_text(bytes)),
                _ => {}
            }
        }
    }
    Ok(CertificateInfo {
        source,
        names,
        not_before_ms: timestamp_ms(validity.not_before.timestamp()),
        not_after_ms: timestamp_ms(validity.not_after.timestamp()),
    })
}

fn ip_text(bytes: &[u8]) -> Option<String> {
    match bytes.len() {
        4 => <[u8; 4]>::try_from(bytes)
            .ok()
            .map(|b| std::net::Ipv4Addr::from(b).to_string()),
        16 => <[u8; 16]>::try_from(bytes)
            .ok()
            .map(|b| std::net::Ipv6Addr::from(b).to_string()),
        _ => None,
    }
}

fn timestamp_ms(seconds: i64) -> u64 {
    u64::try_from(seconds).unwrap_or(0).saturating_mul(1_000)
}

/// Makes a self-signed certificate for `names`, valid from an hour before
/// `now_ms` (for clocks that disagree a little) for ten years.
///
/// The key is ECDSA P-256 from ring's RNG: key generation is crypto behind
/// a narrow interface, which a simulation replaces whole (D16). The serial
/// is derived from the key, positive and 20 bytes (the original's could be
/// negative, RT-14).
pub fn self_signed(names: &[String], now_ms: u64) -> Result<StoredCertificate, TlsError> {
    let generate = |error: rcgen::Error| TlsError::Generate(error.to_string());
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(generate)?;
    let mut params = CertificateParams::new(names.to_vec()).map_err(generate)?;
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "Dassie node");
    name.push(DnType::OrganizationName, "Dassie");
    params.distinguished_name = name;
    let now_s = i64::try_from(now_ms / 1_000).unwrap_or(i64::MAX);
    let from = now_s.saturating_sub(3_600);
    let validity_s =
        i64::try_from(certificate::SELF_SIGNED_VALIDITY_MS / 1_000).unwrap_or(i64::MAX);
    let until = now_s.saturating_add(validity_s);
    params.not_before = time::OffsetDateTime::from_unix_timestamp(from)
        .map_err(|e| TlsError::Generate(e.to_string()))?;
    params.not_after = time::OffsetDateTime::from_unix_timestamp(until)
        .map_err(|e| TlsError::Generate(e.to_string()))?;
    let certificate = params.self_signed(&key).map_err(generate)?;
    Ok(StoredCertificate {
        source: CertificateSource::SelfSigned,
        chain_pem: certificate.pem(),
        key_pem: key.serialize_pem(),
    })
}

/// The certificate being served. The listener reads it at every handshake.
#[derive(Debug)]
pub struct CertificateSlot {
    current: RwLock<LoadedCertificate>,
}

impl CertificateSlot {
    /// A slot holding `certificate`.
    #[must_use]
    pub fn new(certificate: LoadedCertificate) -> Self {
        Self {
            current: RwLock::new(certificate),
        }
    }

    /// Serves `certificate` from the next handshake on.
    pub fn replace(&self, certificate: LoadedCertificate) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = certificate;
    }

    /// What is being served.
    #[must_use]
    pub fn info(&self) -> CertificateInfo {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .info
            .clone()
    }

    fn key(&self) -> Arc<CertifiedKey> {
        Arc::clone(
            &self
                .current
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .key,
        )
    }
}

/// Hands rustls the slot's certificate, whatever name the client asks for:
/// the node has one certificate.
#[derive(Debug)]
struct SlotResolver(Arc<CertificateSlot>);

impl ResolvesServerCert for SlotResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.key())
    }
}

/// The HTTPS listener's TLS configuration: TLS 1.2 and 1.3 with rustls's
/// safe defaults, HTTP/1.1 by ALPN.
pub fn server_config(slot: Arc<CertificateSlot>) -> Result<Arc<ServerConfig>, rustls::Error> {
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(SlotResolver(slot)));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use dassie_node_core::DAY_MS;

    const NOW_MS: u64 = 1_800_000_000_000;

    fn names() -> Vec<String> {
        certificate::self_signed_names(None)
    }

    #[test]
    fn self_signed_certificates_load() {
        let stored = self_signed(&names(), NOW_MS).unwrap();
        let loaded = load_valid(&stored, NOW_MS).unwrap();
        assert_eq!(loaded.info.names, ["localhost", "127.0.0.1", "::1"]);
        assert_eq!(loaded.info.source, CertificateSource::SelfSigned);
        assert_eq!(
            loaded.info.not_after_ms,
            NOW_MS + certificate::SELF_SIGNED_VALIDITY_MS
        );
        assert!(loaded.info.not_before_ms < NOW_MS);
    }

    // RT-12: the original installed any cert/key pair it was sent.
    #[test]
    fn a_key_of_another_certificate_is_refused() {
        let first = self_signed(&names(), NOW_MS).unwrap();
        let second = self_signed(&names(), NOW_MS).unwrap();
        let mixed = StoredCertificate {
            key_pem: second.key_pem,
            ..first
        };
        assert!(matches!(load(&mixed), Err(TlsError::Key(_))));
    }

    #[test]
    fn garbage_and_expired_certificates_are_refused() {
        let good = self_signed(&names(), NOW_MS).unwrap();
        let no_chain = StoredCertificate {
            chain_pem: "nothing".into(),
            ..good.clone()
        };
        assert!(matches!(load(&no_chain), Err(TlsError::Pem(_))));
        let no_key = StoredCertificate {
            key_pem: String::new(),
            ..good.clone()
        };
        assert!(matches!(load(&no_key), Err(TlsError::Pem(_))));
        let late = NOW_MS + certificate::SELF_SIGNED_VALIDITY_MS + DAY_MS;
        assert_eq!(load_valid(&good, late).err(), Some(TlsError::NotValidNow));
        let huge = StoredCertificate {
            chain_pem: "x".repeat(MAX_PEM_BYTES + 1),
            ..good
        };
        assert!(matches!(load(&huge), Err(TlsError::Pem(_))));
    }
}
