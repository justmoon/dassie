//! Founding hubs, on the validator's side (spec/areas/hubs.md#founding-a-hub):
//! the requests of a sponsor on the hub's founding stream, decided by
//! `dassie_hub::founding::validator` and written before they are answered,
//! and the hubs founded so, which the validator role runs beside those of
//! the owner's file.
//!
//! Only a node whose owner named a file in `DASSIE_VALIDATOR_CONFIG` takes
//! part, for the sponsors it lists; every other node refuses.

use std::sync::Arc;

use dassie_hub::HubId;
use dassie_hub::founding::validator::{self, Policy, Record};
use dassie_hub::founding::{FoundingAnswer, FoundingRequest, Refusal};
use dassie_hub::rules::Rules;
use dassie_identity::{NodeId, NodeKey, PublicKey};
use dassie_oer::{Decode, Encode, Reader};
use tokio::sync::{Semaphore, watch};

use super::config::{HubConfig, MAX_VALIDATED_HUBS, ValidatorConfig};
use super::driver::hex;
use crate::log::target;
use crate::node::Node;
use crate::store::{FoundingRecordRow, Store, StoreError};

/// The most founding requests a node decides at once: each is small, and
/// only a listed sponsor's take more than a lookup.
pub const MAX_FOUNDING_REQUESTS: usize = 4;

/// The network of the XRP Ledger testnet, the only one hubs run on while
/// they hold no real money (spec/decisions/real-money.md).
pub const TESTNET_NETWORK: u32 = 1;

/// The validator's side of founding hubs.
#[derive(Debug)]
pub struct Founding {
    sponsors: Vec<NodeId>,
    xrpl: String,
    /// The hubs of the owner's file, which hold places of their own.
    configured: Vec<HubId>,
    /// One decision at a time: each reads the records and writes one.
    deciding: tokio::sync::Mutex<()>,
    places: Arc<Semaphore>,
    /// Counts the hubs founded since the node started, for the validator
    /// role to start them.
    founded: watch::Sender<u64>,
}

impl Founding {
    /// The validator's side, as `config` allows it.
    #[must_use]
    pub fn new(config: &ValidatorConfig) -> Self {
        Self {
            sponsors: config.sponsors.clone(),
            xrpl: config.xrpl.clone(),
            configured: config.hubs.iter().map(|hub| hub.rules.id()).collect(),
            deciding: tokio::sync::Mutex::new(()),
            places: Arc::new(Semaphore::new(MAX_FOUNDING_REQUESTS)),
            founded: watch::channel(0).0,
        }
    }

    /// Changes each time a hub is founded with the node.
    #[must_use]
    pub fn founded(&self) -> watch::Receiver<u64> {
        self.founded.subscribe()
    }

    /// The hubs founded with the node, which it runs: their rules, genesis
    /// and the XRPL server its escrow is followed on.
    ///
    /// # Errors
    ///
    /// The database failed, or holds rules that do not read.
    pub fn hubs(&self, store: &Store) -> Result<Vec<HubConfig>, StoreError> {
        let mut hubs = Vec::new();
        for row in store.hub_founding_records()? {
            let Some((rules, genesis_ms)) = row.genesis.filter(|_| row.running) else {
                continue;
            };
            hubs.push(HubConfig {
                rules: rules_of(&rules)?,
                genesis_ms,
                xrpl: self.xrpl.clone(),
            });
        }
        Ok(hubs)
    }

    /// Answers `request` on the founding stream from `from`, whose key the
    /// connection proved, with the encoded answer; `None` for bytes that
    /// are no founding request.
    pub async fn answer(&self, node: &Node, from: &PublicKey, request: &[u8]) -> Option<Vec<u8>> {
        let request = FoundingRequest::decode_exact(request).ok()?;
        let refused = |refusal| Some(FoundingAnswer::Refused(refusal).to_vec());
        let Ok(_place) = self.places.try_acquire() else {
            return refused(Refusal::Busy);
        };
        let Some(key) = node.node_key() else {
            return refused(Refusal::Busy);
        };
        let _deciding = self.deciding.lock().await;
        let store = node.store_handle();
        let endpoint = node
            .own_info()
            .url
            .and_then(|url| url.strip_prefix("https://").map(str::to_owned));
        let decision = Decision {
            founding: self,
            key: &key,
            endpoint: endpoint.as_deref(),
            now: node.now_ms(),
        };
        let answer = match decision.decide(&store, from, &request) {
            Ok(answer) => answer,
            Err(error) => {
                tracing::error!(target: target::HUB, %error, "cannot keep what a hub founding needs");
                FoundingAnswer::Refused(Refusal::Busy)
            }
        };
        if let (FoundingRequest::Status(hub), FoundingAnswer::Running { .. }) = (&request, &answer)
        {
            let checked = node
                .hubs()
                .statuses()
                .iter()
                .any(|status| status.hub == *hub && status.escrow_checked);
            return Some(
                FoundingAnswer::Running {
                    escrow_checked: checked,
                }
                .to_vec(),
            );
        }
        Some(answer.to_vec())
    }
}

/// One decision, with what it needs.
struct Decision<'a> {
    founding: &'a Founding,
    key: &'a NodeKey,
    endpoint: Option<&'a str>,
    now: u64,
}

impl Decision<'_> {
    /// Decides `request` from `from` and writes what it changed.
    fn decide(
        &self,
        store: &Store,
        from: &PublicKey,
        request: &FoundingRequest,
    ) -> Result<FoundingAnswer, StoreError> {
        let rows = store.hub_founding_records()?;
        let mut records = Vec::with_capacity(rows.len());
        let mut lapsed = Vec::new();
        for row in &rows {
            let record = record_of(row)?;
            if record.holds_place(self.now) {
                records.push(record);
            } else {
                lapsed.push(record.nonce);
            }
        }
        let running_elsewhere = self
            .founding
            .configured
            .iter()
            .filter(|hub| {
                !records
                    .iter()
                    .any(|r| r.running && r.hub().as_ref() == Some(hub))
            })
            .count();
        let policy = Policy {
            sponsors: &self.founding.sponsors,
            max_hubs: MAX_VALIDATED_HUBS,
            endpoint: self.endpoint,
            network: TESTNET_NETWORK,
        };
        let held = (records.as_slice(), running_elsewhere);
        let (answer, record) = match request {
            FoundingRequest::Propose(terms) => {
                match validator::accept(self.key, &policy, held, from, terms, self.now) {
                    Ok((acceptance, record)) => (FoundingAnswer::Accepted(acceptance), record),
                    Err(refusal) => return Ok(FoundingAnswer::Refused(refusal)),
                }
            }
            FoundingRequest::Genesis { rules, genesis_ms } => {
                let genesis = (rules, *genesis_ms);
                match validator::sign_genesis(self.key, &policy, held, from, genesis, self.now) {
                    Ok((signature, record)) => (FoundingAnswer::Signed(signature), record),
                    Err(refusal) => return Ok(FoundingAnswer::Refused(refusal)),
                }
            }
            FoundingRequest::Certificate(certificate) => {
                let taken = validator::take_certificate(
                    self.key,
                    &policy,
                    held,
                    from,
                    certificate,
                    self.now,
                );
                match taken {
                    Ok(record) => (
                        FoundingAnswer::Running {
                            escrow_checked: false,
                        },
                        record,
                    ),
                    Err(refusal) => return Ok(FoundingAnswer::Refused(refusal)),
                }
            }
            FoundingRequest::Status(hub) => return Ok(status(&records, from, hub)),
        };
        let known = records.iter().find(|r| r.nonce == record.nonce);
        if known != Some(&record) {
            store.put_hub_founding_record(&row_of(&record), &lapsed)?;
            self.log(known, &record);
        }
        Ok(answer)
    }

    /// Says what changed, and has the validator role start a hub founded
    /// now.
    fn log(&self, known: Option<&Record>, record: &Record) {
        let nonce = hex(&record.nonce);
        let sponsor = record.sponsor.node_id();
        if record.running && !known.is_some_and(|r| r.running) {
            let hub = record.hub().map(|hub| hex(hub.0.as_bytes()));
            tracing::info!(target: target::HUB, %sponsor, hub = hub.as_deref().unwrap_or_default(), "a hub founded with its genesis certificate: validating it");
            self.founding
                .founded
                .send_modify(|count| *count = count.wrapping_add(1));
        } else if record.genesis.is_some() && known.is_none_or(|r| r.genesis.is_none()) {
            tracing::info!(target: target::HUB, %sponsor, %nonce, "signed the genesis of a hub");
        } else if known.is_none() {
            tracing::info!(target: target::HUB, %sponsor, %nonce, "accepted the terms of a hub");
        }
    }
}

/// How the validator stands with the hub `hub`, to its sponsor: whether it
/// runs it; the host adds whether a quorum checked its escrow.
fn status(records: &[Record], from: &PublicKey, hub: &HubId) -> FoundingAnswer {
    let runs = records
        .iter()
        .any(|r| r.running && r.sponsor == *from && r.hub().as_ref() == Some(hub));
    if runs {
        FoundingAnswer::Running {
            escrow_checked: false,
        }
    } else {
        FoundingAnswer::Refused(Refusal::UnknownHub)
    }
}

fn rules_of(bytes: &[u8]) -> Result<Rules, StoreError> {
    let mut reader = Reader::new(bytes);
    let rules =
        Rules::decode(&mut reader).map_err(|_| StoreError::Corrupt("a founded hub's rules"))?;
    reader
        .finish()
        .map_err(|_| StoreError::Corrupt("a founded hub's rules"))?;
    Ok(rules)
}

fn record_of(row: &FoundingRecordRow) -> Result<Record, StoreError> {
    let sponsor = PublicKey::from_bytes(&row.sponsor)
        .map_err(|_| StoreError::Corrupt("a hub founding's sponsor"))?;
    let genesis = match &row.genesis {
        Some((rules, at)) => Some((rules_of(rules)?, *at)),
        None => None,
    };
    Ok(Record {
        nonce: row.nonce,
        sponsor,
        terms: dassie_hub::Digest(row.terms),
        accepted_ms: row.accepted_at_ms,
        genesis,
        running: row.running,
    })
}

fn row_of(record: &Record) -> FoundingRecordRow {
    FoundingRecordRow {
        nonce: record.nonce,
        sponsor: *record.sponsor.as_bytes(),
        terms: record.terms.0,
        accepted_at_ms: record.accepted_ms,
        genesis: record
            .genesis
            .as_ref()
            .map(|(rules, at)| (rules.to_vec(), *at)),
        running: record.running,
    }
}

#[cfg(test)]
mod tests {
    use dassie_hub::escrow::ValidatorKeys;
    use dassie_hub::founding::GenesisCertificate;
    use dassie_hub::founding::genesis_digest;
    use dassie_hub::validators::ValidatorIndex;

    use super::*;
    use crate::hub::config::tests::terms;

    const NOW: u64 = 1_800_000_000_000;

    fn config(sponsor: &NodeKey) -> ValidatorConfig {
        ValidatorConfig {
            sponsors: vec![sponsor.node_id()],
            hubs: Vec::new(),
            xrpl: "ws://127.0.0.1:1".into(),
        }
    }

    // A validator decides each request and writes it before it answers;
    // the hub a certificate founds is among the hubs it runs, and stays
    // so across a restart, which reads the same records.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn a_founded_hub_is_kept_and_run() {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let store = Store::open_in_memory().unwrap();
        let founding = Founding::new(&config(&sponsor));
        let nodes: Vec<NodeKey> = (1..=4).map(|i| NodeKey::from_bytes(&[i; 32])).collect();
        let terms = terms(&sponsor);
        let from = sponsor.public_key();
        let decide = |key: &NodeKey, request: &FoundingRequest| {
            let decision = Decision {
                founding: &founding,
                key,
                endpoint: Some("v.example:443"),
                now: NOW,
            };
            decision.decide(&store, &from, request).unwrap()
        };
        let FoundingAnswer::Accepted(ours) =
            decide(&nodes[0], &FoundingRequest::Propose(terms.clone()))
        else {
            panic!("accepted");
        };
        let validators: Vec<_> = std::iter::once(ours)
            .chain(nodes[1..].iter().map(|node| {
                ValidatorKeys::derive(node, &terms.nonce)
                    .unwrap()
                    .accept(node, &terms, "w.example:443".into())
                    .unwrap()
            }))
            .collect();
        let rules = Rules {
            terms: terms.clone(),
            validators,
        };
        let genesis = FoundingRequest::Genesis {
            rules: rules.clone(),
            genesis_ms: NOW,
        };
        let FoundingAnswer::Signed(signature) = decide(&nodes[0], &genesis) else {
            panic!("signed");
        };
        let digest = genesis_digest(&rules, NOW);
        let signatures = std::iter::once((ValidatorIndex::new(0).unwrap(), signature))
            .chain((1..3).map(|i| {
                let keys = ValidatorKeys::derive(&nodes[i], &terms.nonce).unwrap();
                (
                    ValidatorIndex::new(i).unwrap(),
                    keys.consensus.sign(digest.as_bytes()),
                )
            }))
            .collect();
        let certificate = GenesisCertificate {
            rules: rules.clone(),
            genesis_ms: NOW,
            signatures,
        };
        let mut founded = founding.founded();
        let answer = decide(&nodes[0], &FoundingRequest::Certificate(certificate));
        assert!(matches!(answer, FoundingAnswer::Running { .. }));
        assert!(founded.has_changed().unwrap());
        founded.mark_unchanged();
        let hubs = founding.hubs(&store).unwrap();
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].rules, rules);
        assert_eq!(hubs[0].genesis_ms, NOW);
        let status = decide(&nodes[0], &FoundingRequest::Status(rules.id()));
        assert!(matches!(status, FoundingAnswer::Running { .. }));
        let unknown = decide(
            &nodes[0],
            &FoundingRequest::Status(HubId(dassie_hub::Digest([1; 32]))),
        );
        assert_eq!(unknown, FoundingAnswer::Refused(Refusal::UnknownHub));
        // Another sponsor is not answered.
        let stranger = NodeKey::from_bytes(&[55; 32]).public_key();
        let decision = Decision {
            founding: &founding,
            key: &nodes[0],
            endpoint: Some("v.example:443"),
            now: NOW,
        };
        let refused = decision
            .decide(&store, &stranger, &FoundingRequest::Propose(terms))
            .unwrap();
        assert_eq!(refused, FoundingAnswer::Refused(Refusal::NotAllowed));
    }
}
