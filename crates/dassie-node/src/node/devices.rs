//! Pairing and paired devices (spec/areas/rpc.md#paired-devices).
//!
//! The owner starts a pairing where only the owner can: on the IPC socket,
//! or in a browser on loopback. Its secret lives only in memory, for
//! [`PAIRING_LIFETIME_MS`], and is claimed once, by a device that proved
//! its key on a stream with purpose `owner`. A new pairing replaces one not
//! claimed yet, so there is never more than one.

use std::fmt;

use dassie_identity::NodeId;
use dassie_node_core::hex;
use subtle::ConstantTimeEq as _;

use super::{Node, Revocation, lock};
use crate::log::target;
use crate::store::{
    AddDevice, DEVICE_SEEN_GRANULARITY_MS, DeviceRow, MAX_DEVICE_NAME_BYTES, StoreError,
};

/// How long a pairing code works.
pub const PAIRING_LIFETIME_MS: u64 = 5 * 60 * 1_000;

/// Bytes of a pairing secret.
const SECRET_BYTES: usize = 32;

/// Most relays a pairing code names: enough to reach the node through
/// another when one is down, few enough for a QR code a phone reads.
pub const MAX_CODE_RELAYS: usize = 4;

/// The pairing waiting to be claimed.
pub(super) struct Pairing {
    secret: [u8; SECRET_BYTES],
    expires_at_ms: u64,
}

/// A pairing the owner started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedPairing {
    /// The `dassie-pair:` URI.
    pub code: String,
    /// When it stops working.
    pub expires_at_ms: u64,
}

/// Why a pairing was not started or not claimed.
#[derive(Debug)]
pub enum PairingError {
    /// The node has no key yet.
    NotSetUp,
    /// No pairing has this secret now: wrong, expired, claimed or replaced.
    NoPairing,
    /// The name is empty or too long.
    InvalidName,
    /// The node keeps as many devices as it may.
    Full,
    /// The database failed.
    Store(StoreError),
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSetUp => f.write_str("the node is not set up yet"),
            Self::NoPairing => f.write_str("no pairing has this secret; start a new one"),
            Self::InvalidName => write!(f, "a device's name is 1 to {MAX_DEVICE_NAME_BYTES} bytes"),
            Self::Full => f.write_str("the node keeps as many devices as it may; revoke one first"),
            Self::Store(error) => error.fmt(f),
        }
    }
}

impl From<StoreError> for PairingError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Where a device reaches the node, for its pairing code: the node's own
/// URL if it has one, else the relays it is connected to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// The node's own URL, whose port is its QUIC endpoint's.
    pub direct: Option<String>,
    /// Nodes that relay to it, with their URLs.
    pub relays: Vec<(NodeId, String)>,
}

/// The version of the pairing code's parameters, its `v`. An app reads a
/// code of a version it knows and refuses another; within a version, it
/// ignores parameters it does not know, so a version adds parameters
/// without a new number (spec/areas/rpc.md#paired-devices).
pub const PAIRING_CODE_VERSION: u32 = 1;

/// The pairing code for `node`'s `secret`, reached as `reach` says:
/// `dassie-pair:<node ID>?v=1&secret=<hex>`, then `direct=<URL>` and
/// `relay=<node ID>@<URL>` for each place, URLs percent-encoded.
#[must_use]
pub fn pairing_code(node: &NodeId, secret: &[u8; SECRET_BYTES], reach: &Reach) -> String {
    let mut code = format!(
        "dassie-pair:{}?v={PAIRING_CODE_VERSION}&secret={}",
        node.as_str(),
        hex::encode(secret)
    );
    if let Some(url) = &reach.direct {
        code.push_str("&direct=");
        code.push_str(&encode(url));
    }
    for (relay, url) in reach.relays.iter().take(MAX_CODE_RELAYS) {
        code.push_str("&relay=");
        code.push_str(relay.as_str());
        code.push_str("%40");
        code.push_str(&encode(url));
    }
    code
}

/// Percent-encodes everything but unreserved characters (RFC 3986).
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

impl Node {
    /// Starts a pairing that devices reach as `reach` says, replacing one
    /// not claimed yet. The caller must be the owner on this machine.
    pub fn start_pairing(&self, reach: &Reach) -> Result<StartedPairing, PairingError> {
        let key = self.node_key().ok_or(PairingError::NotSetUp)?;
        let mut secret = [0u8; SECRET_BYTES];
        self.with_rng(|rng| rng.fill_bytes(&mut secret));
        let expires_at_ms = self.now_ms().saturating_add(PAIRING_LIFETIME_MS);
        *lock(&self.pairing) = Some(Pairing {
            secret,
            expires_at_ms,
        });
        tracing::info!(target: target::AUTH, "pairing started");
        Ok(StartedPairing {
            code: pairing_code(&key.node_id(), &secret, reach),
            expires_at_ms,
        })
    }

    /// Pairs `device`, named `name`, if `secret` is the pairing's and it has
    /// not expired. The pairing is used up either way it succeeds.
    pub fn claim_pairing(
        &self,
        device: NodeId,
        secret: &str,
        name: &str,
    ) -> Result<(), PairingError> {
        if name.is_empty() || name.len() > MAX_DEVICE_NAME_BYTES {
            return Err(PairingError::InvalidName);
        }
        let now_ms = self.now_ms();
        {
            let mut pairing = lock(&self.pairing);
            let matches = pairing.as_ref().is_some_and(|pairing| {
                now_ms < pairing.expires_at_ms
                    && hex::decode_exact::<SECRET_BYTES>(secret)
                        .is_some_and(|bytes| bool::from(bytes.ct_eq(&pairing.secret)))
            });
            if !matches {
                return Err(PairingError::NoPairing);
            }
            *pairing = None;
        }
        let row = DeviceRow {
            id: device,
            name: name.to_owned(),
            paired_at_ms: now_ms,
            last_seen_at_ms: now_ms,
        };
        match self.store.add_device(&row) {
            Ok(()) => {
                tracing::info!(target: target::AUTH, %device, "device paired");
                Ok(())
            }
            Err(AddDevice::Full) => Err(PairingError::Full),
            Err(AddDevice::Failed(error)) => Err(error.into()),
        }
    }

    /// Whether `device` is paired now. Records that it was seen, at most
    /// once per [`DEVICE_SEEN_GRANULARITY_MS`].
    pub fn check_device(&self, device: &NodeId) -> Result<bool, StoreError> {
        let Some(row) = self.store.device(device)? else {
            return Ok(false);
        };
        let now_ms = self.now_ms();
        if now_ms.saturating_sub(row.last_seen_at_ms) >= DEVICE_SEEN_GRANULARITY_MS {
            self.store.touch_device(device, now_ms)?;
        }
        Ok(true)
    }

    /// The paired devices, most recently paired first.
    pub fn devices(&self) -> Result<Vec<DeviceRow>, StoreError> {
        self.store.devices()
    }

    /// Unpairs a device and closes its connections; says whether it was
    /// paired.
    pub fn revoke_device(&self, device: &NodeId) -> Result<bool, StoreError> {
        let existed = self.store.delete_device(device)?;
        self.revoke(Revocation::Device(*device));
        if existed {
            tracing::info!(target: target::AUTH, %device, "device revoked");
        }
        Ok(existed)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use dassie_identity::IdentityKey;

    use super::*;

    #[test]
    fn a_code_names_the_node_its_secret_and_where_to_reach_it() {
        let node = IdentityKey::from_bytes(&[1; 32]).node_id();
        let relay = IdentityKey::from_bytes(&[2; 32]).node_id();
        let reach = Reach {
            direct: Some("https://node.example:4443".into()),
            relays: vec![(relay, "https://relay.example".into())],
        };
        let code = pairing_code(&node, &[0xab; 32], &reach);
        assert_eq!(
            code,
            format!(
                "dassie-pair:{node}?v=1&secret={}&direct=https%3A%2F%2Fnode.example%3A4443\
                 &relay={relay}%40https%3A%2F%2Frelay.example",
                "ab".repeat(32)
            )
        );
    }

    #[test]
    fn a_code_names_a_bounded_number_of_relays() {
        let node = IdentityKey::from_bytes(&[1; 32]).node_id();
        let relays = (0..10u8)
            .map(|seed| (NodeId::from_bytes(&[seed + 10; 24]), "https://r".into()))
            .collect();
        let code = pairing_code(
            &node,
            &[0; 32],
            &Reach {
                direct: None,
                relays,
            },
        );
        assert_eq!(code.matches("&relay=").count(), MAX_CODE_RELAYS);
    }
}
