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

/// The network of the XRP Ledger testnet, the only one a node founds hubs
/// on as a sponsor: it funds the escrow from its testnet XRP
/// (spec/areas/hubs.md#founding-a-hub). A validator takes its network from
/// its owner's file ([`ValidatorConfig::network`]).
pub const TESTNET_NETWORK: u32 = dassie_hub::ledger::TESTNET_NETWORK;

/// Whether a founding request from a node may be read, decided by the key
/// its connection proved before anything of the request is read.
#[derive(Debug)]
pub enum FoundingAccess {
    /// The node founds nothing with this one: the stream is reset unread.
    NotListed,
    /// A listed sponsor, but every place for a request is taken: answered
    /// busy, unread.
    Busy,
    /// A listed sponsor, with the place its request is read and decided in.
    Place(FoundingPlace),
}

/// One of the [`MAX_FOUNDING_REQUESTS`] places, held while a request is
/// read and decided.
#[derive(Debug)]
pub struct FoundingPlace {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// The validator's side of founding hubs.
#[derive(Debug)]
pub struct Founding {
    sponsors: Vec<NodeId>,
    xrpl: String,
    /// The network of the hubs it validates, from its owner's file: it
    /// accepts terms on no other.
    network: u32,
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
            network: config.network.id,
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
    /// and the XRPL server its escrow is followed on. A hub on another
    /// network than the file's is left out, and said so in the log: the
    /// file's server could not follow its escrow. It keeps its record, and
    /// its place, and runs again once the file names its network.
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
            let rules = rules_of(&rules)?;
            if rules.terms.network != self.network {
                tracing::warn!(
                    target: target::HUB,
                    hub = %hex(rules.id().0.as_bytes()),
                    network = rules.terms.network,
                    "not validating a hub founded on another network than the validator file's"
                );
                continue;
            }
            hubs.push(HubConfig {
                rules,
                genesis_ms,
                xrpl: self.xrpl.clone(),
            });
        }
        Ok(hubs)
    }

    /// Whether a founding request of `from`, whose key the connection
    /// proved, is read: only a listed sponsor's, and only in one of the
    /// places for them, so nobody else makes the node buffer a request.
    #[must_use]
    pub fn access(&self, from: &PublicKey) -> FoundingAccess {
        if !self.sponsors.contains(&from.node_id()) {
            return FoundingAccess::NotListed;
        }
        match Arc::clone(&self.places).try_acquire_owned() {
            Ok(permit) => FoundingAccess::Place(FoundingPlace { _permit: permit }),
            Err(_) => FoundingAccess::Busy,
        }
    }

    /// Answers `request` on the founding stream from `from`, read in
    /// `place` ([`access`](Self::access)), with the encoded answer; `None`
    /// for bytes that are no founding request.
    pub async fn answer(
        &self,
        node: &Node,
        from: &PublicKey,
        request: &[u8],
        place: FoundingPlace,
    ) -> Option<Vec<u8>> {
        let _place = place;
        let request = FoundingRequest::decode_exact(request).ok()?;
        let refused = |refusal| Some(FoundingAnswer::Refused(refusal).to_vec());
        if !self.sponsors.contains(&from.node_id()) {
            return refused(Refusal::NotAllowed);
        }
        let Some(key) = node.node_key() else {
            return refused(Refusal::Busy);
        };
        let endpoint = node
            .own_info()
            .url
            .and_then(|url| url.strip_prefix("https://").map(str::to_owned));
        let decision = self.decision(key, endpoint, node.now_ms());
        let decided = self
            .decide(decision, node.store_handle(), *from, request.clone())
            .await;
        let answer = match decided {
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

impl Founding {
    /// What a decision at `now` needs, as the node of `key` reached at
    /// `endpoint`.
    fn decision(&self, key: Arc<NodeKey>, endpoint: Option<String>, now: u64) -> Decision {
        Decision {
            sponsors: self.sponsors.clone(),
            network: self.network,
            configured: self.configured.clone(),
            key,
            endpoint,
            now,
        }
    }

    /// Decides `request` from `from` and writes what it changed, one
    /// decision at a time, off the async runtime as the validator's steps
    /// are written: the database's work never holds up the node's other
    /// tasks.
    async fn decide(
        &self,
        decision: Decision,
        store: Arc<Store>,
        from: PublicKey,
        request: FoundingRequest,
    ) -> Result<FoundingAnswer, StoreError> {
        let _deciding = self.deciding.lock().await;
        let decided =
            tokio::task::spawn_blocking(move || decision.decide(&store, &from, &request)).await;
        let (answer, changed) =
            decided.map_err(|_| StoreError::Corrupt("a founding decision did not finish"))??;
        if let Some((known, record)) = changed {
            self.log(known.as_ref(), &record);
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
            self.founded
                .send_modify(|count| *count = count.wrapping_add(1));
        } else if record.genesis.is_some() && known.is_none_or(|r| r.genesis.is_none()) {
            tracing::info!(target: target::HUB, %sponsor, %nonce, "signed the genesis of a hub");
        } else if known.is_none() {
            tracing::info!(target: target::HUB, %sponsor, %nonce, "accepted the terms of a hub");
        }
    }
}

/// What a founding decision changed: the record of its nonce as it was, if
/// there was one, and as it is now.
type Changed = Option<(Option<Record>, Record)>;

/// One decision, with what it needs, owned: it runs off the async runtime.
struct Decision {
    sponsors: Vec<NodeId>,
    network: u32,
    configured: Vec<HubId>,
    key: Arc<NodeKey>,
    endpoint: Option<String>,
    now: u64,
}

impl Decision {
    /// Decides `request` from `from`, writes what it changed, and says so.
    fn decide(
        &self,
        store: &Store,
        from: &PublicKey,
        request: &FoundingRequest,
    ) -> Result<(FoundingAnswer, Changed), StoreError> {
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
            .configured
            .iter()
            .filter(|hub| {
                !records
                    .iter()
                    .any(|r| r.running && r.hub().as_ref() == Some(hub))
            })
            .count();
        let policy = Policy {
            sponsors: &self.sponsors,
            max_hubs: MAX_VALIDATED_HUBS,
            endpoint: self.endpoint.as_deref(),
            network: self.network,
        };
        let held = (records.as_slice(), running_elsewhere);
        let (answer, record) = match request {
            FoundingRequest::Propose(terms) => {
                match validator::accept(&self.key, &policy, held, from, terms, self.now) {
                    Ok((acceptance, record)) => (FoundingAnswer::Accepted(acceptance), record),
                    Err(refusal) => return Ok((FoundingAnswer::Refused(refusal), None)),
                }
            }
            FoundingRequest::Genesis { rules, genesis_ms } => {
                let genesis = (rules, *genesis_ms);
                match validator::sign_genesis(&self.key, &policy, held, from, genesis, self.now) {
                    Ok((signature, record)) => (FoundingAnswer::Signed(signature), record),
                    Err(refusal) => return Ok((FoundingAnswer::Refused(refusal), None)),
                }
            }
            FoundingRequest::Certificate(certificate) => {
                let taken = validator::take_certificate(
                    &self.key,
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
                    Err(refusal) => return Ok((FoundingAnswer::Refused(refusal), None)),
                }
            }
            FoundingRequest::Status(hub) => return Ok((status(&records, from, hub), None)),
        };
        let known = records.iter().find(|r| r.nonce == record.nonce);
        if known == Some(&record) {
            return Ok((answer, None));
        }
        store.put_hub_founding_record(&row_of(&record), &lapsed)?;
        Ok((answer, Some((known.cloned(), record))))
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
            network: &dassie_xrpl::network::TESTNET,
            unl: dassie_xrpl::validator_list::UnlConfig::testnet(),
        }
    }

    /// What `founding` answers `request` from `from`, as the node of `key`.
    async fn decide(
        founding: &Founding,
        store: &Arc<Store>,
        (key, from): (&NodeKey, &PublicKey),
        request: &FoundingRequest,
    ) -> FoundingAnswer {
        let key = Arc::new(NodeKey::from_bytes(key.as_bytes()));
        let decision = founding.decision(key, Some("v.example:443".into()), NOW);
        founding
            .decide(decision, Arc::clone(store), *from, request.clone())
            .await
            .unwrap()
    }

    // A validator decides each request and writes it before it answers;
    // the hub a certificate founds is among the hubs it runs, and stays
    // so across a restart, which reads the same records.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn a_founded_hub_is_kept_and_run() {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let founding = Founding::new(&config(&sponsor));
        let nodes: Vec<NodeKey> = (1..=4).map(|i| NodeKey::from_bytes(&[i; 32])).collect();
        let terms = terms(&sponsor);
        let from = sponsor.public_key();
        let ours = (&nodes[0], &from);
        let propose = FoundingRequest::Propose(terms.clone());
        let FoundingAnswer::Accepted(accepted) = decide(&founding, &store, ours, &propose).await
        else {
            panic!("accepted");
        };
        let validators: Vec<_> = std::iter::once(accepted)
            .chain(nodes[1..].iter().map(|node| {
                ValidatorKeys::derive(node, terms.network, &terms.nonce)
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
        let FoundingAnswer::Signed(signature) = decide(&founding, &store, ours, &genesis).await
        else {
            panic!("signed");
        };
        let digest = genesis_digest(&rules, NOW);
        let signatures = std::iter::once((ValidatorIndex::new(0).unwrap(), signature))
            .chain((1..3).map(|i| {
                let keys = ValidatorKeys::derive(&nodes[i], terms.network, &terms.nonce).unwrap();
                (
                    ValidatorIndex::new(i).unwrap(),
                    keys.consensus.sign(digest.as_bytes()),
                )
            }))
            .collect();
        let certificate = FoundingRequest::Certificate(GenesisCertificate {
            rules: rules.clone(),
            genesis_ms: NOW,
            signatures,
        });
        let mut founded = founding.founded();
        let answer = decide(&founding, &store, ours, &certificate).await;
        assert!(matches!(answer, FoundingAnswer::Running { .. }));
        assert!(founded.has_changed().unwrap());
        founded.mark_unchanged();
        let hubs = founding.hubs(&store).unwrap();
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].rules, rules);
        assert_eq!(hubs[0].genesis_ms, NOW);
        // A file that names another network leaves the hub out, and its
        // record stays: the testnet's file runs it again.
        let mut on_mainnet = config(&sponsor);
        on_mainnet.network = &dassie_xrpl::network::MAINNET;
        assert!(Founding::new(&on_mainnet).hubs(&store).unwrap().is_empty());
        assert_eq!(founding.hubs(&store).unwrap().len(), 1);
        let status = decide(
            &founding,
            &store,
            ours,
            &FoundingRequest::Status(rules.id()),
        )
        .await;
        assert!(matches!(status, FoundingAnswer::Running { .. }));
        let unknown = FoundingRequest::Status(HubId(dassie_hub::Digest([1; 32])));
        let unknown = decide(&founding, &store, ours, &unknown).await;
        assert_eq!(unknown, FoundingAnswer::Refused(Refusal::UnknownHub));
        // Another sponsor is not answered.
        let stranger = NodeKey::from_bytes(&[55; 32]).public_key();
        let refused = decide(&founding, &store, (&nodes[0], &stranger), &propose).await;
        assert_eq!(refused, FoundingAnswer::Refused(Refusal::NotAllowed));
    }

    // The validator accepts terms only on its file's network.
    #[tokio::test]
    async fn terms_on_another_network_are_refused() {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let store = Arc::new(Store::open_in_memory().unwrap());
        let mut on_mainnet = terms(&sponsor);
        on_mainnet.network = dassie_xrpl::network::MAINNET.id;
        let from = sponsor.public_key();
        let ours = (&NodeKey::from_bytes(&[1; 32]), &from);
        let propose = FoundingRequest::Propose(on_mainnet);
        let founding = Founding::new(&config(&sponsor));
        let refused = decide(&founding, &store, ours, &propose).await;
        assert_eq!(refused, FoundingAnswer::Refused(Refusal::Terms));
    }

    // A decision reads and writes the database off the async runtime: while
    // another connection holds the database's write lock, the decision
    // waits for it on a blocking task, and the runtime's other tasks go on.
    #[test]
    #[allow(clippy::disallowed_methods)]
    fn deciding_leaves_the_runtime_free() {
        use std::sync::atomic::{AtomicU64, Ordering};

        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let other =
            rusqlite::Connection::open(dir.path().join(crate::store::DATABASE_FILE)).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            other.execute_batch("COMMIT").unwrap();
        });
        let founding = Founding::new(&config(&sponsor));
        let ticks = Arc::new(AtomicU64::new(0));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let answer = runtime.block_on(async {
            let counted = Arc::clone(&ticks);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    counted.fetch_add(1, Ordering::Relaxed);
                }
            });
            tokio::task::yield_now().await;
            let node = NodeKey::from_bytes(&[1; 32]);
            let propose = FoundingRequest::Propose(terms(&sponsor));
            decide(&founding, &store, (&node, &sponsor.public_key()), &propose).await
        });
        release.join().unwrap();
        assert!(matches!(answer, FoundingAnswer::Accepted(_)), "{answer:?}");
        let ticked = ticks.load(Ordering::Relaxed);
        assert!(ticked >= 10, "the runtime stood still: {ticked} ticks");
    }
}
