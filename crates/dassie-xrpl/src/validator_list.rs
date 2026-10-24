//! A list publisher's signed validator list, as its site serves it, and
//! the publishers each network trusts
//! (spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers).
//!
//! A site answers with JSON: the publisher's master key and manifest, and
//! in version 1 one blob with its signature, in version 2 up to
//! [`MAX_BLOBS`], each with its signature and perhaps a newer manifest of
//! the publisher. A blob is JSON too: the list's sequence, when it expires
//! and perhaps when it takes effect, in seconds since 2000-01-01, and the
//! validators, each by its master key and perhaps with its manifest. The
//! publisher's signing key signs the blob's bytes as they came, so they are
//! kept as they came. What the lists mean together, the UNL, is
//! [`crate::unl`]'s.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

use crate::keys::{Key, PublicKey, Signature};
use crate::manifest::{MAX_MANIFEST_BYTES, Manifest};
use crate::network;
use crate::{base64, from_hex, hex_upper};

/// The longest answer a list site may give, in bytes: version 2's blobs,
/// each of the most validators, each with its manifest, fit.
pub const MAX_LIST_BYTES: usize = 524_288;
/// The most blobs one answer carries, as rippled bounds them.
pub const MAX_BLOBS: usize = 5;
/// The most validators one list names: several times mainnet's.
pub const MAX_LIST_VALIDATORS: usize = 128;
/// The most publishers an account trusts.
pub const MAX_PUBLISHERS: usize = 4;
/// The most sites one publisher serves its list from: a second for when
/// the first fails.
pub const MAX_SITES: usize = 2;
/// The longest site, in bytes.
pub const MAX_SITE_BYTES: usize = 256;

/// Ripple's list publisher on mainnet, `nHBe4vqSAzjpPRLKwSFzRFtmvzXaf5wPPmuVrQCAoJoS1zskgDA4`,
/// as rippled's default configuration names it.
pub const RIPPLE: PublicKey = PublicKey::built_in([
    0xED, 0x26, 0x77, 0xAB, 0xFF, 0xD1, 0xB3, 0x3A, 0xC6, 0xFB, 0xC3, 0x06, 0x2B, 0x71, 0xF1, 0xE8,
    0x39, 0x7C, 0x15, 0x05, 0xE1, 0xC4, 0x2C, 0x64, 0xD1, 0x1A, 0xD1, 0xB2, 0x8F, 0xF7, 0x3F, 0x47,
    0x34,
]);
/// Its site.
pub const RIPPLE_SITE: &str = "https://vl.ripple.com";
/// The XRP Ledger Foundation's list publisher on mainnet,
/// `nHBioecSeus4HRpEqjyq1P8a6gPH7KYhL51RfC9LAfWcKTDUHX8e`, as rippled's
/// default configuration names it.
pub const XRPLF: PublicKey = PublicKey::built_in([
    0xED, 0x42, 0xAE, 0xC5, 0x8B, 0x70, 0x1E, 0xEB, 0xB7, 0x73, 0x56, 0xFF, 0xFE, 0xC2, 0x6F, 0x83,
    0xC1, 0xF0, 0x40, 0x72, 0x63, 0x53, 0x0F, 0x06, 0x8C, 0x7C, 0x73, 0xD3, 0x92, 0xC7, 0xE0, 0x6F,
    0xD1,
]);
/// Its site: the address that serves the list, since no redirect is
/// followed (`vl.xrplf.org` redirects to it).
pub const XRPLF_SITE: &str = "https://unl.xrplf.org";
/// The testnet's list publisher,
/// `nHBeJBfBkbRDAfyon4idcDcUDspz8WnDWoKE7AE2Dta1y7qfSMu8`.
pub const TESTNET: PublicKey = PublicKey::built_in([
    0xED, 0x26, 0x48, 0x07, 0x10, 0x28, 0x05, 0x22, 0x0D, 0xA0, 0xF3, 0x12, 0xE7, 0x1F, 0xC2, 0xC6,
    0x9E, 0x15, 0x52, 0xC9, 0xC5, 0x79, 0x0F, 0x6C, 0x25, 0xE3, 0x72, 0x9D, 0xEB, 0x57, 0x3D, 0x58,
    0x60,
]);
/// Its site.
pub const TESTNET_SITE: &str = "https://vl.altnet.rippletest.net";

/// Why a list is not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListError {
    /// Longer than [`MAX_LIST_BYTES`], or with more blobs or validators
    /// than are read.
    TooLarge,
    /// Not a list, in either version.
    Malformed,
    /// Of a publisher the account does not trust.
    UnknownPublisher,
    /// With a publisher's manifest its master key did not sign.
    BadManifest,
    /// Of a publisher that revoked its key.
    Revoked,
    /// Not signed by the publisher's signing key.
    BadSignature,
    /// Expiring before it takes effect.
    Invalid,
    /// Below the highest sequence seen of its publisher.
    Stale,
    /// Expired.
    Expired,
}

/// A publisher the account trusts: its master key, and the sites that
/// serve its list, the first first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publisher {
    /// Its master key, which signs its manifests.
    pub master_key: PublicKey,
    /// Where its list is fetched, `https://` (or `http://`, which the node
    /// allows only where it allows private addresses).
    pub sites: Vec<String>,
}

/// The publishers an account trusts, whose lists make its UNL as rippled's
/// defaults combine them ([`crate::unl`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnlConfig {
    /// Each by its master key, in the order the account fetches them.
    pub publishers: Vec<Publisher>,
}

impl UnlConfig {
    /// Mainnet's: the two publishers rippled's default configuration names.
    #[must_use]
    pub fn mainnet() -> Self {
        Self {
            publishers: alloc::vec![
                Publisher {
                    master_key: RIPPLE,
                    sites: alloc::vec![RIPPLE_SITE.to_string()],
                },
                Publisher {
                    master_key: XRPLF,
                    sites: alloc::vec![XRPLF_SITE.to_string()],
                },
            ],
        }
    }

    /// The testnet's: its publisher.
    #[must_use]
    pub fn testnet() -> Self {
        Self {
            publishers: alloc::vec![Publisher {
                master_key: TESTNET,
                sites: alloc::vec![TESTNET_SITE.to_string()],
            }],
        }
    }

    /// The built-in config of network `network_id`: mainnet's or the
    /// testnet's, and no other network's.
    #[must_use]
    pub fn for_network(network_id: u32) -> Option<Self> {
        if network_id == network::MAINNET.id {
            Some(Self::mainnet())
        } else if network_id == network::TESTNET.id {
            Some(Self::testnet())
        } else {
            None
        }
    }

    /// The owner's publishers in `value`, which replace this config's
    /// entirely: `{"publishers": [{"key": …, "sites": […]}, …]}`, each key
    /// as hex or as a node public key. `None` if it is not that, or does
    /// not pass [`Self::check`].
    #[must_use]
    pub fn with_override(&self, value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let mut publishers = None;
        for (key, field) in object {
            match key.as_str() {
                "publishers" => publishers = Some(parse_publishers(field)?),
                _ => return None,
            }
        }
        let config = Self {
            publishers: publishers?,
        };
        config.check().then_some(config)
    }

    /// Whether it is one an account can follow: one to [`MAX_PUBLISHERS`]
    /// publishers, each once, each with one to [`MAX_SITES`] sites of at
    /// most [`MAX_SITE_BYTES`].
    #[must_use]
    pub fn check(&self) -> bool {
        let count = self.publishers.len();
        let distinct = self
            .publishers
            .iter()
            .enumerate()
            .all(|(index, publisher)| {
                (self.publishers.iter().skip(index + 1))
                    .all(|other| other.master_key != publisher.master_key)
            });
        let sites = self.publishers.iter().all(|publisher| {
            (1..=MAX_SITES).contains(&publisher.sites.len())
                && publisher.sites.iter().all(|site| site_fits(site))
        });
        (1..=MAX_PUBLISHERS).contains(&count) && distinct && sites
    }
}

fn parse_publishers(value: &Value) -> Option<Vec<Publisher>> {
    let entries = value.as_array()?;
    if entries.len() > MAX_PUBLISHERS {
        return None;
    }
    let mut publishers = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = entry.as_object()?;
        let mut master_key = None;
        let mut sites = None;
        for (key, field) in object {
            match key.as_str() {
                "key" => master_key = Some(parse_key(field.as_str()?)?),
                "sites" => {
                    let entries = field.as_array()?;
                    if entries.len() > MAX_SITES {
                        return None;
                    }
                    let parsed: Option<Vec<String>> = entries
                        .iter()
                        .map(|site| site.as_str().map(ToString::to_string))
                        .collect();
                    sites = Some(parsed?);
                }
                _ => return None,
            }
        }
        publishers.push(Publisher {
            master_key: master_key?,
            sites: sites?,
        });
    }
    Some(publishers)
}

/// A key as hex, or as a node public key.
fn parse_key(text: &str) -> Option<PublicKey> {
    PublicKey::from_node_public(text)
        .or_else(|| PublicKey::from_bytes(from_hex(text)?.try_into().ok()?))
}

fn site_fits(site: &str) -> bool {
    site.len() <= MAX_SITE_BYTES
        && (site.starts_with("https://") || site.starts_with("http://"))
        && site.chars().all(|character| character.is_ascii_graphic())
}

/// A blob as its site served it, with its signature, and in version 2
/// perhaps a manifest of its publisher's of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedBlob {
    /// The blob's bytes, as signed.
    pub blob: Vec<u8>,
    /// The publisher's signing key's signature of them.
    pub signature: Signature,
    /// A manifest of the publisher's, as it came.
    pub manifest: Option<Vec<u8>>,
}

impl SignedBlob {
    /// `list` written as a blob and signed by `signing`, as a publisher
    /// signs it.
    #[must_use]
    pub fn sign(list: &List, signing: &dyn Key) -> Self {
        let blob = list.to_blob();
        Self {
            signature: signing.sign(&blob),
            blob,
            manifest: None,
        }
    }
}

/// What a list site answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The publisher's master key.
    pub publisher: PublicKey,
    /// The publisher's manifest, as it came.
    pub manifest: Vec<u8>,
    /// The blobs: one in version 1, up to [`MAX_BLOBS`] in version 2.
    pub blobs: Vec<SignedBlob>,
}

impl Message {
    /// The answer in `body`, if it is one, in version 1 or 2. Nothing in it
    /// is verified.
    ///
    /// # Errors
    ///
    /// [`ListError::TooLarge`] or [`ListError::Malformed`].
    pub fn parse(body: &[u8]) -> Result<Self, ListError> {
        if body.len() > MAX_LIST_BYTES {
            return Err(ListError::TooLarge);
        }
        let value: Value = serde_json::from_slice(body).map_err(|_| ListError::Malformed)?;
        let object = value.as_object().ok_or(ListError::Malformed)?;
        let publisher = (object.get("public_key").and_then(Value::as_str))
            .and_then(|text| PublicKey::from_bytes(from_hex(text)?.try_into().ok()?))
            .ok_or(ListError::Malformed)?;
        let manifest = manifest_bytes(object.get("manifest"))?;
        let version = object.get("version").and_then(Value::as_u64);
        let blobs = match (version, object.get("blobs_v2")) {
            (Some(1), None) => alloc::vec![signed_blob(object, false)?],
            (Some(2), Some(Value::Array(blobs)))
                if !object.contains_key("blob") && !object.contains_key("signature") =>
            {
                if blobs.len() > MAX_BLOBS {
                    return Err(ListError::TooLarge);
                }
                let blobs: Result<Vec<SignedBlob>, ListError> = (blobs.iter())
                    .map(|blob| signed_blob(blob.as_object().ok_or(ListError::Malformed)?, true))
                    .collect();
                blobs?
            }
            _ => return Err(ListError::Malformed),
        };
        if blobs.is_empty() {
            return Err(ListError::Malformed);
        }
        Ok(Self {
            publisher,
            manifest: manifest.ok_or(ListError::Malformed)?,
            blobs,
        })
    }

    /// It as a site serves it: version 1 for one blob without a manifest of
    /// its own, version 2 otherwise.
    #[must_use]
    pub fn to_json(&self) -> Vec<u8> {
        let blob = |signed: &SignedBlob| {
            let mut entry = Map::new();
            entry.insert("blob".into(), base64::encode(&signed.blob).into());
            entry.insert(
                "signature".into(),
                hex_upper(signed.signature.as_bytes()).into(),
            );
            if let Some(manifest) = &signed.manifest {
                entry.insert("manifest".into(), base64::encode(manifest).into());
            }
            entry
        };
        let mut message = Map::new();
        message.insert(
            "public_key".into(),
            hex_upper(self.publisher.as_bytes()).into(),
        );
        message.insert("manifest".into(), base64::encode(&self.manifest).into());
        match self.blobs.as_slice() {
            [single] if single.manifest.is_none() => {
                message.extend(blob(single));
                message.insert("version".into(), 1.into());
            }
            blobs => {
                let blobs: Vec<Value> = blobs.iter().map(|b| Value::Object(blob(b))).collect();
                message.insert("blobs_v2".into(), blobs.into());
                message.insert("version".into(), 2.into());
            }
        }
        serde_json::to_vec(&Value::Object(message)).unwrap_or_default()
    }
}

/// A manifest in base64, at most [`MAX_MANIFEST_BYTES`]; `None` if there
/// is none.
fn manifest_bytes(value: Option<&Value>) -> Result<Option<Vec<u8>>, ListError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes = value
        .as_str()
        .and_then(base64::decode)
        .ok_or(ListError::Malformed)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ListError::Malformed);
    }
    Ok(Some(bytes))
}

/// A blob, its signature, and, in version 2, perhaps its own manifest.
fn signed_blob(object: &Map<String, Value>, version_2: bool) -> Result<SignedBlob, ListError> {
    let blob = (object.get("blob").and_then(Value::as_str))
        .and_then(base64::decode)
        .ok_or(ListError::Malformed)?;
    let signature = (object.get("signature").and_then(Value::as_str))
        .and_then(from_hex)
        .and_then(|bytes| Signature::from_slice(&bytes))
        .ok_or(ListError::Malformed)?;
    let manifest = if version_2 {
        manifest_bytes(object.get("manifest"))?
    } else {
        None
    };
    Ok(SignedBlob {
        blob,
        signature,
        manifest,
    })
}

/// A list, as its blob says. Times are seconds since 2000-01-01.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    /// Its sequence: a higher one replaces a lower.
    pub sequence: u32,
    /// When it expires.
    pub expiration: u32,
    /// When it takes effect, if later than it arrives.
    pub effective: Option<u32>,
    /// The validators it names, by master key, each with the manifest it
    /// carries for it, if any is the master's own. Whether a manifest
    /// verifies is the UNL's to check, and only if it is newer than one it
    /// holds.
    pub validators: BTreeMap<PublicKey, Option<Manifest>>,
}

impl List {
    /// The list in `blob`, if it is one: a sequence and an expiry, later
    /// than when it takes effect, and at most [`MAX_LIST_VALIDATORS`]
    /// validators, each by its master key as hex. A master named twice is
    /// named once, with the newer of its manifests; a manifest that does
    /// not read, or is another key's, is left out, as rippled leaves it.
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn parse(blob: &[u8]) -> Result<Self, ListError> {
        let value: Value = serde_json::from_slice(blob).map_err(|_| ListError::Malformed)?;
        let object = value.as_object().ok_or(ListError::Malformed)?;
        let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
        let field = |key| object.get(key).and_then(number).ok_or(ListError::Malformed);
        let sequence = field("sequence")?;
        let expiration = field("expiration")?;
        let effective = match object.get("effective") {
            None => None,
            Some(value) => Some(number(value).ok_or(ListError::Malformed)?),
        };
        if expiration <= effective.unwrap_or(0) {
            return Err(ListError::Invalid);
        }
        let entries =
            (object.get("validators").and_then(Value::as_array)).ok_or(ListError::Malformed)?;
        if entries.len() > MAX_LIST_VALIDATORS {
            return Err(ListError::TooLarge);
        }
        let mut validators: BTreeMap<PublicKey, Option<Manifest>> = BTreeMap::new();
        for entry in entries {
            let (master_key, manifest) = listed(entry).ok_or(ListError::Malformed)?;
            let newer = match (validators.get(&master_key), &manifest) {
                (Some(Some(kept)), Some(manifest)) => manifest.sequence > kept.sequence,
                (Some(Some(_)), None) => false,
                _ => true,
            };
            if newer {
                validators.insert(master_key, manifest);
            }
        }
        Ok(Self {
            sequence,
            expiration,
            effective,
            validators,
        })
    }

    /// Its blob, as a publisher writes it.
    #[must_use]
    pub fn to_blob(&self) -> Vec<u8> {
        let validators: Vec<Value> = (self.validators.iter())
            .map(|(master_key, manifest)| {
                let mut entry = Map::new();
                entry.insert(
                    "validation_public_key".into(),
                    hex_upper(master_key.as_bytes()).into(),
                );
                if let Some(manifest) = manifest {
                    entry.insert(
                        "manifest".into(),
                        base64::encode(&manifest.to_bytes()).into(),
                    );
                }
                Value::Object(entry)
            })
            .collect();
        let mut list = json!({
            "sequence": self.sequence,
            "expiration": self.expiration,
            "validators": validators,
        });
        if let (Some(effective), Some(object)) = (self.effective, list.as_object_mut()) {
            object.insert("effective".into(), effective.into());
        }
        serde_json::to_vec(&list).unwrap_or_default()
    }
}

/// A validator a list names: its master key, and its manifest if the list
/// carries one of that key's.
fn listed(entry: &Value) -> Option<(PublicKey, Option<Manifest>)> {
    let key = from_hex(entry.get("validation_public_key")?.as_str()?)?;
    let master_key = PublicKey::from_bytes(key.try_into().ok()?)?;
    let manifest = (entry.get("manifest").and_then(Value::as_str))
        .and_then(base64::decode)
        .and_then(|bytes| Manifest::from_bytes(&bytes).ok())
        .filter(|manifest| manifest.master_key == master_key);
    Some((master_key, manifest))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // The built-in keys are the publishers' as their node public keys
    // name them, as rippled's validators-example.txt has them.
    #[test]
    fn the_built_in_publishers_are_rippled_s() {
        assert_eq!(
            RIPPLE.to_node_public(),
            "nHBe4vqSAzjpPRLKwSFzRFtmvzXaf5wPPmuVrQCAoJoS1zskgDA4"
        );
        assert_eq!(
            XRPLF.to_node_public(),
            "nHBioecSeus4HRpEqjyq1P8a6gPH7KYhL51RfC9LAfWcKTDUHX8e"
        );
        assert_eq!(
            TESTNET.to_node_public(),
            "nHBeJBfBkbRDAfyon4idcDcUDspz8WnDWoKE7AE2Dta1y7qfSMu8"
        );
        for config in [UnlConfig::mainnet(), UnlConfig::testnet()] {
            assert!(config.check());
        }
        assert_eq!(UnlConfig::for_network(0), Some(UnlConfig::mainnet()));
        assert_eq!(UnlConfig::for_network(1), Some(UnlConfig::testnet()));
        assert_eq!(UnlConfig::for_network(2), None);
    }

    // spec/designs/xrpl-validation.md#10-configuration-and-state: the
    // owner's publishers replace the network's entirely, and name nothing
    // else.
    #[test]
    fn the_owner_s_publishers_replace_the_network_s() {
        let mainnet = UnlConfig::mainnet();
        let own = mainnet
            .with_override(&json!({
                "publishers": [{
                    "key": "nHBeJBfBkbRDAfyon4idcDcUDspz8WnDWoKE7AE2Dta1y7qfSMu8",
                    "sites": ["http://localhost:8080/vl", "https://example.com/vl"],
                }],
            }))
            .unwrap();
        assert_eq!(own.publishers.len(), 1);
        assert_eq!(own.publishers[0].master_key, TESTNET);
        let hex = hex_upper(XRPLF.as_bytes());
        let named = mainnet
            .with_override(&json!({"publishers": [{"key": hex, "sites": [XRPLF_SITE]}]}))
            .unwrap();
        assert_eq!(named.publishers[0].master_key, XRPLF);
        let site = |site: &str| json!({"publishers": [{"key": hex, "sites": [site]}]});
        for bad in [
            json!({}),
            json!({"publishers": []}),
            json!({"publishers": [{"key": hex, "sites": []}]}),
            // There is no floor.
            json!({"publishers": [{"key": hex, "sites": [XRPLF_SITE]}], "floor": 3}),
            json!({"publishers": [{"key": hex, "sites": [XRPLF_SITE]}], "other": 1}),
            json!({"publishers": [
                {"key": hex, "sites": [XRPLF_SITE]},
                {"key": hex, "sites": [XRPLF_SITE]},
            ]}),
            json!({"publishers": [{"key": "ED00", "sites": [XRPLF_SITE]}]}),
            site("ftp://example.com"),
            site("https://exa mple.com"),
            site(&alloc::format!("https://{}", "a".repeat(MAX_SITE_BYTES))),
        ] {
            assert_eq!(mainnet.with_override(&bad), None, "{bad}");
        }
    }

    #[test]
    fn lists_expire_after_they_take_effect() {
        let blob = |list: Value| List::parse(&serde_json::to_vec(&list).unwrap());
        let validators = json!([]);
        assert!(blob(json!({"sequence": 1, "expiration": 10, "validators": validators})).is_ok());
        assert_eq!(
            blob(json!({"sequence": 1, "expiration": 10, "effective": 10, "validators": []})),
            Err(ListError::Invalid)
        );
        assert_eq!(
            blob(json!({"sequence": 1, "expiration": 0, "validators": []})),
            Err(ListError::Invalid)
        );
        for bad in [
            json!({"sequence": -1, "expiration": 10, "validators": []}),
            json!({"sequence": 1, "expiration": 4_294_967_296_u64, "validators": []}),
            json!({"sequence": "1", "expiration": 10, "validators": []}),
            json!({"sequence": 1, "expiration": 10}),
            json!({"sequence": 1, "expiration": 10, "validators": [{"validation_public_key": "00"}]}),
            json!({"sequence": 1, "expiration": 10, "effective": 1.5, "validators": []}),
        ] {
            assert_eq!(blob(bad), Err(ListError::Malformed));
        }
    }

    #[test]
    fn messages_are_version_1_or_2_and_bounded() {
        let parse = |message: Value| Message::parse(&serde_json::to_vec(&message).unwrap());
        let key = hex_upper(TESTNET.as_bytes());
        let blob = base64::encode(b"{}");
        let signature = "00";
        let v1 = json!({"public_key": key, "manifest": "AA==", "blob": blob, "signature": signature, "version": 1});
        assert_eq!(parse(v1.clone()).unwrap().blobs.len(), 1);
        let entry = json!({"blob": blob, "signature": signature});
        let v2 = |count| json!({"public_key": key, "manifest": "AA==", "version": 2, "blobs_v2": alloc::vec![entry.clone(); count]});
        assert_eq!(parse(v2(MAX_BLOBS)).unwrap().blobs.len(), MAX_BLOBS);
        assert_eq!(parse(v2(MAX_BLOBS + 1)), Err(ListError::TooLarge));
        assert_eq!(parse(v2(0)), Err(ListError::Malformed));
        let mut both = v2(1);
        both["blob"] = blob.clone().into();
        assert_eq!(parse(both), Err(ListError::Malformed));
        let mut v3 = v1.clone();
        v3["version"] = 3.into();
        assert_eq!(parse(v3), Err(ListError::Malformed));
        let mut v1_with_blobs = v1.clone();
        v1_with_blobs["blobs_v2"] = json!([entry]);
        assert_eq!(parse(v1_with_blobs), Err(ListError::Malformed));
        let mut unpadded = v1;
        unpadded["manifest"] = "AA".into();
        assert_eq!(parse(unpadded), Err(ListError::Malformed));
        assert_eq!(
            Message::parse(&alloc::vec![b' '; MAX_LIST_BYTES + 1]),
            Err(ListError::TooLarge)
        );
    }
}
