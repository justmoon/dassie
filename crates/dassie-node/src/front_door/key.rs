//! The front door's own Ed25519 key, which it proves to the nodes that
//! connect to it and inside every relayed stream it opens. It is no node's
//! key: it holds nothing and signs nothing else.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use base64::Engine as _;
use dassie_identity::{NodeKey, base64url};
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject;

/// Longest key file read: a PEM block with a comment fits.
const MAX_KEY_FILE_BYTES: u64 = 4096;

/// The bytes of a seed.
const SEED_BYTES: usize = 32;

/// Reads the key in `path`: 32 bytes of seed, or PKCS#8 as PEM or DER, as
/// `openssl genpkey -algorithm ed25519` writes it.
///
/// # Errors
///
/// The file cannot be read, or holds no such key.
pub fn read(path: &Path) -> Result<NodeKey, String> {
    let shown = path.display();
    let length = std::fs::metadata(path)
        .map_err(|error| format!("cannot read the key file {shown}: {error}"))?
        .len();
    if length > MAX_KEY_FILE_BYTES {
        return Err(format!("the key file {shown} is too long"));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("cannot read the key file {shown}: {error}"))?;
    parse(&bytes).ok_or_else(|| {
        format!("the key file {shown} holds neither a 32-byte seed nor an Ed25519 key in PKCS#8")
    })
}

fn parse(bytes: &[u8]) -> Option<NodeKey> {
    if let Ok(seed) = <[u8; SEED_BYTES]>::try_from(bytes) {
        return Some(NodeKey::from_bytes(&seed));
    }
    if bytes.starts_with(b"-----BEGIN") {
        return match PrivateKeyDer::from_pem_slice(bytes).ok()? {
            PrivateKeyDer::Pkcs8(der) => NodeKey::from_pkcs8_der(der.secret_pkcs8_der()).ok(),
            _ => None,
        };
    }
    NodeKey::from_pkcs8_der(bytes).ok()
}

/// The key in `path`, made first if there is no file: a new seed from the
/// OS, written as PKCS#8 PEM, readable by its owner alone. An existing
/// file is never replaced.
///
/// # Errors
///
/// The file cannot be read or made.
pub fn read_or_make(path: &Path) -> Result<NodeKey, String> {
    if path.exists() {
        return read(path);
    }
    let mut seed = [0u8; SEED_BYTES];
    getrandom::fill(&mut seed).map_err(|error| format!("no randomness: {error}"))?;
    let key = NodeKey::from_bytes(&seed);
    seed.fill(0);
    let encoded = base64::engine::general_purpose::STANDARD.encode(key.to_pkcs8_der().as_bytes());
    let pem = format!("-----BEGIN PRIVATE KEY-----\n{encoded}\n-----END PRIVATE KEY-----\n");
    let shown = path.display();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("cannot make the key file {shown}: {error}"))?;
    file.write_all(pem.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write the key file {shown}: {error}"))?;
    Ok(key)
}

/// The `DASSIE_RELAY` entry that names a front door with `key` at `url`,
/// as a periphery node takes it (spec/areas/node.md#a-chosen-relay).
#[must_use]
pub fn relay_entry(key: &NodeKey, url: &str) -> String {
    let public = key.public_key();
    let mut encoded = [0u8; 43];
    let length = base64url::encode(public.as_bytes(), &mut encoded);
    let encoded = std::str::from_utf8(encoded.get(..length).unwrap_or_default()).unwrap_or("");
    serde_json::json!({
        "id": key.node_id().as_str(),
        "url": url,
        "publicKey": encoded,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::env::{Env, Settings};

    #[test]
    fn a_key_reads_as_seed_pem_or_der() {
        let key = NodeKey::from_bytes(&[7; 32]);
        let der = key.to_pkcs8_der();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(der.as_bytes())
        );
        for bytes in [&[7u8; 32][..], der.as_bytes(), pem.as_bytes()] {
            assert_eq!(parse(bytes).unwrap().node_id(), key.node_id());
        }
        assert!(parse(&[7; 31]).is_none());
        assert!(parse(b"-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n").is_none());
    }

    #[test]
    fn a_key_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("front-door.key");
        let made = read_or_make(&path).unwrap();
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(read_or_make(&path).unwrap().node_id(), made.node_id());
        assert_eq!(read(&path).unwrap().node_id(), made.node_id());
    }

    #[test]
    fn the_relay_entry_is_one_a_node_takes() {
        let key = NodeKey::from_bytes(&[8; 32]);
        let entry = relay_entry(&key, "https://relay.dassiepay.net");
        let env = Env::from_pairs([
            ("DASSIE_STATE_DIRECTORY", "/tmp/unused"),
            ("DASSIE_RELAY", entry.as_str()),
        ]);
        let relay = Settings::from_env(&env).unwrap().relay.unwrap();
        assert_eq!(relay.id, key.node_id());
        assert_eq!(relay.url, "https://relay.dassiepay.net");
    }
}
