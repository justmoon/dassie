//! Where a sponsor's founding of a hub stands, and what it asks whom next
//! (spec/designs/settlement-hubs.md#2-founding-a-hub), without IO.
//!
//! The founding goes through its [`Stage`]s in order: every invited
//! validator accepts the terms; a quorum signs the genesis of the rules;
//! a quorum runs the hub the certificate founds; the sponsor sets up the
//! escrow, which f + 1 validators then say a quorum checked. Each request
//! goes again until its validator answers it, waiting longer after each
//! failure, so a validator that is down or refuses for now is asked again
//! for as long as the founding lasts. The host persists the founding
//! ([`Sponsor::encode_to`]) after every answer that changed it.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, Signature};
use dassie_oer::{Alphabet, Decode, Encode, Reader, Size, Writer, size};

use super::genesis_digest;
use super::wire::{FoundingAnswer, FoundingRequest, GenesisCertificate, Refusal};
use crate::codec::{HubDecodeError, SIGNATURE_LEN, read_signature, write_signature};
use crate::rules::{Acceptance, HubId, MAX_ENDPOINT_LEN, Millis, Rules, Terms};
use crate::validators::{MAX_VALIDATORS, MIN_VALIDATORS, ValidatorIndex};

/// The first wait before asking a validator again that did not answer, or
/// refused, and the longest.
pub const RETRY_MIN_MS: Millis = 1_000;
/// The longest wait before asking a validator again.
pub const RETRY_MAX_MS: Millis = 60_000;

/// How often the validators that run the hub are asked whether a quorum
/// checked its escrow, once it is set up.
pub const STATUS_EVERY_MS: Millis = 5_000;

const ENDPOINT_SIZE: Size = Size::range(1, MAX_ENDPOINT_LEN);
const ENDPOINT_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'A', b'Z')
    .with_range(b'0', b'9')
    .with_chars(b".-:[]");

/// Where a founding stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Asking each validator to accept the terms.
    Accepting = 0,
    /// The rules are fixed: asking each to sign their genesis.
    Signing = 1,
    /// A quorum signed: handing each the certificate.
    Certifying = 2,
    /// A quorum runs the hub: the sponsor sets up its escrow, and asks
    /// whether a quorum checked it.
    Escrow = 3,
    /// f + 1 validators say a quorum checked the escrow: the hub takes
    /// deposits.
    Founded = 4,
}

impl Stage {
    const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Accepting,
            1 => Self::Signing,
            2 => Self::Certifying,
            3 => Self::Escrow,
            4 => Self::Founded,
            _ => return None,
        })
    }
}

/// A validator the sponsor picked, and what it answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invited {
    /// Its node, which the transport pins.
    pub node: NodeId,
    /// Where the sponsor reaches it, `host:port`.
    pub endpoint: String,
    /// Its acceptance of the terms, once it gave one that checks.
    pub acceptance: Option<Acceptance>,
    /// Its signature of the genesis, once it gave one that checks.
    pub signature: Option<Signature>,
    /// Whether it said it runs the hub.
    pub running: bool,
    /// Whether it said a quorum checked the escrow.
    pub escrow_checked: bool,
    /// Its last refusal, until it answers otherwise.
    pub refusal: Option<Refusal>,
    /// When it is asked next, and how long it waits after a failure.
    next_ms: Millis,
    wait_ms: Millis,
}

/// Why a founding cannot start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoundingError {
    /// The terms do not check.
    Terms(&'static str),
    /// Too few or too many validators, one named twice, or an endpoint an
    /// acceptance could not carry.
    Validators,
}

/// A sponsor's founding of a hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sponsor {
    terms: Terms,
    invited: Vec<Invited>,
    stage: Stage,
    /// The genesis timestamp, fixed with the rules.
    genesis_ms: Option<Millis>,
    /// Whether the escrow is set up on the underlying ledger.
    escrow_set_up: bool,
}

impl Sponsor {
    /// A founding of a hub of `terms` with the validators at `invited`.
    ///
    /// # Errors
    ///
    /// The terms do not check, or the validators are not a set.
    pub fn new(terms: Terms, invited: Vec<(NodeId, String)>) -> Result<Self, FoundingError> {
        terms.check().map_err(|error| match error {
            crate::rules::RulesError::Terms(why) => FoundingError::Terms(why),
            _ => FoundingError::Terms("the terms do not check"),
        })?;
        let distinct = invited.iter().enumerate().all(|(i, (node, _))| {
            invited
                .iter()
                .skip(i.saturating_add(1))
                .all(|(other, _)| other != node)
        });
        let endpoints = invited
            .iter()
            .all(|(_, endpoint)| Acceptance::valid_endpoint(endpoint));
        if !(MIN_VALIDATORS..=MAX_VALIDATORS).contains(&invited.len()) || !distinct || !endpoints {
            return Err(FoundingError::Validators);
        }
        let invited = invited
            .into_iter()
            .map(|(node, endpoint)| Invited::new(node, endpoint))
            .collect();
        Ok(Self {
            terms,
            invited,
            stage: Stage::Accepting,
            genesis_ms: None,
            escrow_set_up: false,
        })
    }

    /// The terms.
    #[must_use]
    pub const fn terms(&self) -> &Terms {
        &self.terms
    }

    /// The validators, in the order of the set.
    #[must_use]
    pub fn invited(&self) -> &[Invited] {
        &self.invited
    }

    /// Where it stands.
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.stage
    }

    /// The genesis timestamp, once the rules are fixed.
    #[must_use]
    pub const fn genesis_ms(&self) -> Option<Millis> {
        self.genesis_ms
    }

    /// Whether the escrow is set up.
    #[must_use]
    pub const fn escrow_set_up(&self) -> bool {
        self.escrow_set_up
    }

    /// The hub's rules, once every validator accepted.
    #[must_use]
    pub fn rules(&self) -> Option<Rules> {
        let validators = self
            .invited
            .iter()
            .map(|invited| invited.acceptance.clone())
            .collect::<Option<Vec<_>>>()?;
        Some(Rules {
            terms: self.terms.clone(),
            validators,
        })
    }

    /// The hub's ID, once its rules are fixed.
    #[must_use]
    pub fn hub(&self) -> Option<HubId> {
        self.rules().map(|rules| rules.id())
    }

    /// The quorum of the set.
    fn quorum(&self) -> usize {
        let n = self.invited.len();
        n.saturating_sub(n.saturating_sub(1) / 3)
    }

    /// The genesis certificate, once a quorum signed.
    #[must_use]
    pub fn certificate(&self) -> Option<GenesisCertificate> {
        let rules = self.rules()?;
        let genesis_ms = self.genesis_ms?;
        let signatures: Vec<(ValidatorIndex, Signature)> = self
            .invited
            .iter()
            .enumerate()
            .filter_map(|(i, invited)| Some((ValidatorIndex::new(i)?, invited.signature?)))
            .collect();
        (signatures.len() >= self.quorum()).then_some(GenesisCertificate {
            rules,
            genesis_ms,
            signatures,
        })
    }

    /// When a request is due next.
    #[must_use]
    pub fn deadline(&self) -> Option<Millis> {
        self.invited
            .iter()
            .enumerate()
            .filter(|(i, _)| self.wanted(*i).is_some())
            .map(|(_, invited)| invited.next_ms)
            .min()
    }

    /// What to ask validator `index` at this stage, if anything.
    fn wanted(&self, index: usize) -> Option<FoundingRequest> {
        let invited = self.invited.get(index)?;
        match self.stage {
            Stage::Accepting if invited.acceptance.is_none() => {
                Some(FoundingRequest::Propose(self.terms.clone()))
            }
            Stage::Signing if invited.signature.is_none() => Some(FoundingRequest::Genesis {
                rules: self.rules()?,
                genesis_ms: self.genesis_ms?,
            }),
            Stage::Certifying | Stage::Escrow | Stage::Founded if !invited.running => {
                self.certificate().map(FoundingRequest::Certificate)
            }
            Stage::Escrow if self.escrow_set_up && !invited.escrow_checked => {
                self.hub().map(FoundingRequest::Status)
            }
            _ => None,
        }
    }

    /// The requests due at `now`, each to a validator by its place: each
    /// waits for its answer ([`answer`](Self::answer)) before it is due
    /// again.
    pub fn due(&mut self, now: Millis) -> Vec<(usize, FoundingRequest)> {
        let mut due = Vec::new();
        for index in 0..self.invited.len() {
            let ready = self
                .invited
                .get(index)
                .is_some_and(|invited| invited.next_ms <= now);
            if !ready {
                continue;
            }
            if let Some(request) = self.wanted(index) {
                if let Some(invited) = self.invited.get_mut(index) {
                    // Asked again only after the answer, or a failure.
                    invited.next_ms = Millis::MAX;
                }
                due.push((index, request));
            }
        }
        due
    }

    /// Takes validator `index`'s answer at `now`, or that none came
    /// (`None`): whether the founding changed, to persist.
    pub fn answer(&mut self, index: usize, now: Millis, answer: Option<FoundingAnswer>) -> bool {
        // A validator that runs the hub answers each poll of the escrow's
        // status alike: that is no failure to back off from.
        let polled = matches!(answer, Some(FoundingAnswer::Running { .. }));
        let changed = match answer {
            Some(FoundingAnswer::Accepted(acceptance)) => self.accepted(index, acceptance),
            Some(FoundingAnswer::Signed(signature)) => self.signed(index, signature),
            Some(FoundingAnswer::Running { escrow_checked }) => self.running(index, escrow_checked),
            Some(FoundingAnswer::Refused(refusal)) => {
                self.set(index, |invited| invited.refusal = Some(refusal));
                true
            }
            None => false,
        };
        let progressed =
            (changed || polled) && self.invited.get(index).is_some_and(|i| i.refusal.is_none());
        self.reschedule(index, now, progressed);
        if self.advance(now) {
            for invited in &mut self.invited {
                invited.next_ms = now;
                invited.wait_ms = 0;
            }
        }
        self.check_invariants();
        changed
    }

    /// The escrow is set up on the underlying ledger: the validators are
    /// asked whether a quorum checked it.
    pub fn set_escrow_set_up(&mut self, now: Millis) {
        if !self.escrow_set_up {
            self.escrow_set_up = true;
            for invited in &mut self.invited {
                invited.next_ms = now;
            }
        }
    }

    /// Validator `index` is asked again: at once after progress, otherwise
    /// after a wait that doubles; the status of a checked escrow every
    /// [`STATUS_EVERY_MS`].
    fn reschedule(&mut self, index: usize, now: Millis, progressed: bool) {
        let status = self.stage == Stage::Escrow && self.escrow_set_up;
        self.set(index, |invited| {
            if progressed {
                invited.wait_ms = 0;
                invited.next_ms = if status {
                    now.saturating_add(STATUS_EVERY_MS)
                } else {
                    now
                };
            } else {
                invited.wait_ms = invited
                    .wait_ms
                    .saturating_mul(2)
                    .clamp(RETRY_MIN_MS, RETRY_MAX_MS);
                invited.next_ms = now.saturating_add(invited.wait_ms);
            }
        });
    }

    fn set(&mut self, index: usize, change: impl FnOnce(&mut Invited)) {
        if let Some(invited) = self.invited.get_mut(index) {
            change(invited);
        }
    }

    fn accepted(&mut self, index: usize, acceptance: Acceptance) -> bool {
        let terms = self.terms.digest();
        let Some(invited) = self.invited.get_mut(index) else {
            return false;
        };
        let checks = acceptance.node.node_id() == invited.node
            && acceptance.verifies(&terms, &self.terms.nonce)
            && acceptance.keys_usable(&self.terms.escrow)
            && Acceptance::valid_endpoint(&acceptance.endpoint);
        if self.stage != Stage::Accepting || !checks {
            return false;
        }
        invited.acceptance = Some(acceptance);
        invited.refusal = None;
        true
    }

    fn signed(&mut self, index: usize, signature: Signature) -> bool {
        let (Some(rules), Some(genesis_ms)) = (self.rules(), self.genesis_ms) else {
            return false;
        };
        let digest = genesis_digest(&rules, genesis_ms);
        let Some(invited) = self.invited.get_mut(index) else {
            return false;
        };
        let checks = invited.acceptance.as_ref().is_some_and(|acceptance| {
            acceptance
                .consensus
                .verify(digest.as_bytes(), &signature)
                .is_ok()
        });
        if self.stage != Stage::Signing || !checks {
            return false;
        }
        invited.signature = Some(signature);
        invited.refusal = None;
        true
    }

    fn running(&mut self, index: usize, escrow_checked: bool) -> bool {
        if self.stage < Stage::Certifying {
            return false;
        }
        let Some(invited) = self.invited.get_mut(index) else {
            return false;
        };
        let changed = !invited.running || invited.escrow_checked != escrow_checked;
        invited.running = true;
        invited.escrow_checked = escrow_checked;
        invited.refusal = None;
        changed
    }

    /// Moves to the next stage once this one is done: whether it moved.
    fn advance(&mut self, now: Millis) -> bool {
        let count = |test: fn(&Invited) -> bool| self.invited.iter().filter(|i| test(i)).count();
        let next = match self.stage {
            Stage::Accepting if self.rules().is_some_and(|rules| rules.check().is_ok()) => {
                self.genesis_ms = Some(now);
                Stage::Signing
            }
            Stage::Signing if self.certificate().is_some() => Stage::Certifying,
            Stage::Certifying if count(|i| i.running) >= self.quorum() => Stage::Escrow,
            Stage::Escrow
                if self.escrow_set_up
                    && count(|i| i.escrow_checked)
                        >= self
                            .invited
                            .len()
                            .saturating_sub(self.quorum())
                            .saturating_add(1) =>
            {
                Stage::Founded
            }
            _ => return false,
        };
        self.stage = next;
        true
    }

    /// Checks what always holds.
    ///
    /// # Panics
    ///
    /// If something does not.
    pub fn check_invariants(&self) {
        assert!((MIN_VALIDATORS..=MAX_VALIDATORS).contains(&self.invited.len()));
        assert_eq!(self.genesis_ms.is_some(), self.stage >= Stage::Signing);
        if self.stage >= Stage::Signing {
            assert!(self.rules().is_some(), "every validator accepted");
        }
        if self.stage >= Stage::Certifying {
            assert!(self.certificate().is_some(), "a quorum signed");
        }
        assert!(self.stage < Stage::Founded || self.escrow_set_up);
    }
}

impl Invited {
    const fn new(node: NodeId, endpoint: String) -> Self {
        Self {
            node,
            endpoint,
            acceptance: None,
            signature: None,
            running: false,
            escrow_checked: false,
            refusal: None,
            next_ms: 0,
            wait_ms: 0,
        }
    }
}

impl Encode for Invited {
    fn encoded_len(&self) -> usize {
        self.node.encoded_len()
            + size::prefixed(self.endpoint.len())
            + 1
            + self.acceptance.as_ref().map_or(0, Encode::encoded_len)
            + 1
            + self.signature.map_or(0, |_| SIGNATURE_LEN)
            + 3
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.node.encode_to(writer);
        writer.write_ascii(&self.endpoint, ENDPOINT_SIZE, ENDPOINT_ALPHABET);
        writer.write_bool(self.acceptance.is_some());
        if let Some(acceptance) = &self.acceptance {
            acceptance.encode_to(writer);
        }
        writer.write_bool(self.signature.is_some());
        if let Some(signature) = &self.signature {
            write_signature(writer, signature);
        }
        writer.write_bool(self.running);
        writer.write_bool(self.escrow_checked);
        writer.write_u8(self.refusal.map_or(0, Refusal::code));
    }
}

impl<'a> Decode<'a> for Invited {
    type Error = HubDecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, HubDecodeError> {
        let node = NodeId::decode(reader)?;
        let endpoint = reader.read_ascii(ENDPOINT_SIZE, ENDPOINT_ALPHABET)?.into();
        let mut invited = Self::new(node, endpoint);
        if reader.read_bool()? {
            invited.acceptance = Some(Acceptance::decode(reader)?);
        }
        if reader.read_bool()? {
            invited.signature = Some(read_signature(reader)?);
        }
        invited.running = reader.read_bool()?;
        invited.escrow_checked = reader.read_bool()?;
        invited.refusal = match reader.read_u8()? {
            0 => None,
            code => Some(Refusal::from_code(code).ok_or(HubDecodeError::Invalid("refusal"))?),
        };
        Ok(invited)
    }
}

impl Encode for Sponsor {
    fn encoded_len(&self) -> usize {
        self.terms.encoded_len()
            + 1
            + 1
            + self.genesis_ms.map_or(0, |_| 8)
            + 1
            + size::quantity(self.invited.len())
            + self.invited.iter().map(Encode::encoded_len).sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.terms.encode_to(writer);
        writer.write_u8(self.stage as u8);
        writer.write_bool(self.genesis_ms.is_some());
        if let Some(genesis_ms) = self.genesis_ms {
            writer.write_u64(genesis_ms);
        }
        writer.write_bool(self.escrow_set_up);
        writer.write_quantity(self.invited.len());
        for invited in &self.invited {
            invited.encode_to(writer);
        }
    }
}

impl<'a> Decode<'a> for Sponsor {
    type Error = HubDecodeError;

    /// Reads a founding as it was persisted: every request is due at once.
    fn decode(reader: &mut Reader<'a>) -> Result<Self, HubDecodeError> {
        let terms = Terms::decode(reader)?;
        let stage = Stage::from_code(reader.read_u8()?).ok_or(HubDecodeError::Invalid("stage"))?;
        let genesis_ms = if reader.read_bool()? {
            Some(reader.read_u64()?)
        } else {
            None
        };
        let escrow_set_up = reader.read_bool()?;
        let count = reader.read_quantity(MAX_VALIDATORS)?;
        let mut invited = Vec::with_capacity(count);
        for _ in 0..count {
            invited.push(Invited::decode(reader)?);
        }
        let sponsor = Self {
            terms,
            invited,
            stage,
            genesis_ms,
            escrow_set_up,
        };
        if !sponsor.consistent() {
            return Err(HubDecodeError::Invalid(
                "a founding that does not fit together",
            ));
        }
        Ok(sponsor)
    }
}

impl Sponsor {
    /// Whether what was read fits together, as [`check_invariants`](Self::check_invariants)
    /// asserts of a founding in memory.
    fn consistent(&self) -> bool {
        (MIN_VALIDATORS..=MAX_VALIDATORS).contains(&self.invited.len())
            && self.genesis_ms.is_some() == (self.stage >= Stage::Signing)
            && (self.stage < Stage::Signing || self.rules().is_some())
            && (self.stage < Stage::Certifying || self.certificate().is_some())
            && (self.stage < Stage::Founded || self.escrow_set_up)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use dassie_identity::IdentityKey;

    use super::*;
    use crate::founding::validator::{self, Policy, Record};
    use crate::rules::tests::terms;

    const NOW: Millis = 1_800_000_000_000;

    /// A sponsor and four validators that answer with the validator's own
    /// decisions, each keeping its records.
    struct Founding {
        sponsor: Sponsor,
        sponsor_key: IdentityKey,
        nodes: Vec<IdentityKey>,
        records: Vec<Vec<Record>>,
        listed: Vec<NodeId>,
        now: Millis,
    }

    impl Founding {
        fn new() -> Self {
            let sponsor_key = dassie_identity::TableKeygen::identity_key(&[100; 32]);
            let nodes: Vec<IdentityKey> = (1..=4)
                .map(|i| dassie_identity::TableKeygen::identity_key(&[i; 32]))
                .collect();
            let invited = nodes
                .iter()
                .enumerate()
                .map(|(i, node)| (node.node_id(), alloc::format!("v{i}.example:443")))
                .collect();
            let sponsor = Sponsor::new(terms(&sponsor_key), invited).unwrap();
            Self {
                sponsor,
                listed: vec![sponsor_key.node_id()],
                sponsor_key,
                nodes,
                records: vec![Vec::new(); 4],
                now: NOW,
            }
        }

        fn answer(&mut self, index: usize, request: &FoundingRequest) -> FoundingAnswer {
            let policy = Policy {
                sponsors: &self.listed,
                max_hubs: 4,
                endpoint: Some("v.example:443"),
                network: 1,
            };
            let node = &self.nodes[index];
            let from = self.sponsor_key.identity();
            let now = self.now;
            let records = (self.records[index].as_slice(), 0);
            let (answer, record) = match request {
                FoundingRequest::Propose(terms) => {
                    match validator::accept(node, &policy, records, &from, terms, now) {
                        Ok((acceptance, record)) => (FoundingAnswer::Accepted(acceptance), record),
                        Err(refusal) => return FoundingAnswer::Refused(refusal),
                    }
                }
                FoundingRequest::Genesis { rules, genesis_ms } => {
                    let genesis = (rules, *genesis_ms);
                    match validator::sign_genesis(node, &policy, records, &from, genesis, now) {
                        Ok((signature, record)) => (FoundingAnswer::Signed(signature), record),
                        Err(refusal) => return FoundingAnswer::Refused(refusal),
                    }
                }
                FoundingRequest::Certificate(certificate) => {
                    match validator::take_certificate(
                        node,
                        &policy,
                        records,
                        &from,
                        certificate,
                        now,
                    ) {
                        Ok(record) => (
                            FoundingAnswer::Running {
                                escrow_checked: false,
                            },
                            record,
                        ),
                        Err(refusal) => return FoundingAnswer::Refused(refusal),
                    }
                }
                FoundingRequest::Status(_) => {
                    return FoundingAnswer::Running {
                        escrow_checked: true,
                    };
                }
            };
            self.records[index] = vec![record];
            answer
        }

        /// Runs the founding until nothing is due, `down` never answering.
        fn run(&mut self, down: &[usize]) {
            for _ in 0..20 {
                self.now += RETRY_MAX_MS;
                let now = self.now;
                for (index, request) in self.sponsor.due(now) {
                    let answer = (!down.contains(&index)).then(|| self.answer(index, &request));
                    self.sponsor.answer(index, now, answer);
                    // What it persisted reads back.
                    let bytes = self.sponsor.to_vec();
                    let read = Sponsor::decode_exact(&bytes).unwrap();
                    assert_eq!(read.to_vec(), bytes);
                }
            }
        }
    }

    #[test]
    fn a_founding_goes_through_its_stages() {
        let mut founding = Founding::new();
        assert_eq!(founding.sponsor.stage(), Stage::Accepting);
        founding.run(&[]);
        assert_eq!(founding.sponsor.stage(), Stage::Escrow);
        let rules = founding.sponsor.rules().unwrap();
        rules.check().unwrap();
        assert!(founding.sponsor.certificate().unwrap().verifies());
        assert!(founding.sponsor.invited().iter().all(|i| i.running));
        assert!(founding.records.iter().all(|r| r[0].running));
        // Nothing more is asked until the escrow is set up.
        assert!(founding.sponsor.due(founding.now).is_empty());
        founding.sponsor.set_escrow_set_up(founding.now);
        founding.run(&[]);
        assert_eq!(founding.sponsor.stage(), Stage::Founded);
    }

    // Every validator must accept before the rules are fixed; once fixed,
    // a quorum founds the hub, and the last is handed the certificate
    // when it is back.
    #[test]
    fn a_quorum_founds_it_once_all_accepted() {
        let mut founding = Founding::new();
        founding.run(&[3]);
        assert_eq!(founding.sponsor.stage(), Stage::Accepting);
        let mut founding = Founding::new();
        // All accept, then one is down from the genesis on.
        let now = NOW;
        for (index, request) in founding.sponsor.due(now) {
            let answer = founding.answer(index, &request);
            founding.sponsor.answer(index, now, Some(answer));
        }
        assert_eq!(founding.sponsor.stage(), Stage::Signing);
        founding.run(&[3]);
        assert_eq!(founding.sponsor.stage(), Stage::Escrow);
        assert!(!founding.sponsor.invited()[3].running);
        founding.run(&[]);
        assert!(founding.sponsor.invited()[3].running);
        assert!(founding.records[3][0].running);
    }

    // What does not check is not taken: an acceptance by another node, or
    // a signature by another key.
    #[test]
    fn forged_answers_are_not_taken() {
        let mut founding = Founding::new();
        let terms = founding.sponsor.terms().clone();
        let stranger = dassie_identity::TableKeygen::identity_key(&[77; 32]);
        let keys =
            crate::escrow::ValidatorKeys::derive(&stranger, terms.network, &terms.nonce).unwrap();
        let theirs = keys
            .accept(&stranger, &terms, "x.example:1".into())
            .unwrap();
        assert_eq!(founding.sponsor.due(NOW).len(), 4);
        let forged = FoundingAnswer::Accepted(theirs);
        assert!(!founding.sponsor.answer(0, NOW, Some(forged)));
        assert_eq!(founding.sponsor.invited()[0].acceptance, None);
        let refused = FoundingAnswer::Refused(Refusal::NotAllowed);
        assert!(founding.sponsor.answer(1, NOW, Some(refused)));
        assert_eq!(
            founding.sponsor.invited()[1].refusal,
            Some(Refusal::NotAllowed)
        );
        founding.sponsor.answer(2, NOW, None);
        founding.sponsor.answer(3, NOW, None);
        assert_eq!(
            founding.sponsor.deadline(),
            Some(NOW + RETRY_MIN_MS),
            "asked again later"
        );
        // All accept; a signature by another key is not taken.
        let later = NOW + RETRY_MIN_MS;
        for (index, request) in founding.sponsor.due(later) {
            let answer = founding.answer(index, &request);
            founding.sponsor.answer(index, later, Some(answer));
        }
        assert_eq!(founding.sponsor.stage(), Stage::Signing);
        assert_eq!(founding.sponsor.due(later).len(), 4);
        let forged = FoundingAnswer::Signed(stranger.sign(b"genesis"));
        assert!(!founding.sponsor.answer(0, later, Some(forged)));
        assert_eq!(founding.sponsor.invited()[0].signature, None);
        for index in 1..4 {
            founding.sponsor.answer(index, later, None);
        }
        founding.run(&[]);
        assert_eq!(founding.sponsor.stage(), Stage::Escrow);
    }

    #[test]
    fn foundings_need_a_set() {
        let key = dassie_identity::TableKeygen::identity_key(&[1; 32]);
        let three = (1..=3)
            .map(|i| {
                (
                    dassie_identity::TableKeygen::identity_key(&[i; 32]).node_id(),
                    "v.example:1".into(),
                )
            })
            .collect();
        assert_eq!(
            Sponsor::new(terms(&key), three),
            Err(FoundingError::Validators)
        );
        let twice = vec![(key.node_id(), "v.example:1".into()); 4];
        assert_eq!(
            Sponsor::new(terms(&key), twice),
            Err(FoundingError::Validators)
        );
    }
}
