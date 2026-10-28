//! What a validator decides while a sponsor founds a hub with it
//! (spec/designs/settlement-hubs.md#2-founding-a-hub), without IO: whether
//! it accepts the terms, whether it signs a genesis, and whether it runs a
//! hub a certificate founds. The node keeps a [`Record`] per nonce and
//! writes what each decision returns before it answers.
//!
//! - **Opt-in, per sponsor.** It answers only a sponsor its owner lists,
//!   and only that sponsor's own requests: the transport proved who asks.
//! - **Bounded.** Every hub it runs and every founding it took part in that
//!   is still open holds one of its places ([`Policy::max_hubs`]): an
//!   acceptance until it lapses ([`ACCEPTANCE_TTL_MS`]), a signed genesis
//!   until its hub runs.
//! - **One genesis per nonce.** Its keys for a hub derive from the nonce,
//!   so it signs only one genesis under a nonce, and runs no other.

use alloc::vec::Vec;

use dassie_identity::{IdentityKey, NodeId, NodeIdentity, Signature};

use super::wire::{GenesisCertificate, Refusal};
use super::{ACCEPTANCE_TTL_MS, MAX_GENESIS_AGE_MS, MAX_GENESIS_AHEAD_MS, genesis_digest};
use crate::digest::Digest;
use crate::escrow::ValidatorKeys;
use crate::rules::{Acceptance, HubId, Millis, Rules, Terms};

/// What the validator's owner allows.
#[derive(Clone, Copy, Debug)]
pub struct Policy<'a> {
    /// The sponsors it validates for.
    pub sponsors: &'a [NodeId],
    /// The most hubs it validates or helps found at once.
    pub max_hubs: usize,
    /// Where it is reached, `host:port`, if it has a public endpoint.
    pub endpoint: Option<&'a str>,
    /// The underlying ledger's network it validates hubs on, as its owner
    /// set it: the node takes one of real money only where the live realm
    /// runs (spec/decisions/real-money.md).
    pub network: u32,
}

/// What a validator keeps of one founding, by the nonce of its terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The terms' nonce.
    pub nonce: [u8; 32],
    /// The sponsor's identity.
    pub sponsor: NodeIdentity,
    /// The digest of the terms it accepted.
    pub terms: Digest,
    /// When it accepted them.
    pub accepted_ms: Millis,
    /// The genesis it signed under this nonce, if it did: the rules and
    /// the timestamp.
    pub genesis: Option<(Rules, Millis)>,
    /// Whether it runs the hub: a certificate founded it.
    pub running: bool,
}

impl Record {
    /// Whether it holds one of the validator's places at `now`: a hub it
    /// runs, a genesis it signed, or an acceptance that has not lapsed.
    #[must_use]
    pub const fn holds_place(&self, now: Millis) -> bool {
        self.running
            || self.genesis.is_some()
            || now.saturating_sub(self.accepted_ms) < ACCEPTANCE_TTL_MS
    }

    /// The hub it signed the genesis of, if any.
    #[must_use]
    pub fn hub(&self) -> Option<HubId> {
        self.genesis.as_ref().map(|(rules, _)| rules.id())
    }
}

/// Whether `from` may found hubs with this validator.
fn allowed(
    policy: &Policy<'_>,
    from: &NodeIdentity,
    sponsor: &NodeIdentity,
) -> Result<(), Refusal> {
    if from != sponsor || !policy.sponsors.contains(&sponsor.node_id()) {
        return Err(Refusal::NotAllowed);
    }
    Ok(())
}

/// Whether a founding under `nonce` may take a place: one it already has,
/// or one of those free besides `running_elsewhere` hubs it runs from its
/// owner's file and `records` of other nonces that hold one.
fn has_room(
    policy: &Policy<'_>,
    records: &[Record],
    running_elsewhere: usize,
    nonce: &[u8; 32],
    now: Millis,
) -> bool {
    let taken = records
        .iter()
        .filter(|record| record.nonce != *nonce && record.holds_place(now))
        .count()
        .saturating_add(running_elsewhere);
    taken < policy.max_hubs
}

/// Accepts `terms`, proposed by `from`: the validator's acceptance, and the
/// record to keep. The same terms again get the same acceptance; other
/// terms under a nonce whose genesis it signed are refused.
///
/// # Errors
///
/// Why it does not accept.
pub fn accept(
    node: &IdentityKey,
    policy: &Policy<'_>,
    (records, running_elsewhere): (&[Record], usize),
    from: &NodeIdentity,
    terms: &Terms,
    now: Millis,
) -> Result<(Acceptance, Record), Refusal> {
    allowed(policy, from, &terms.sponsor)?;
    if terms.network != policy.network || terms.check().is_err() {
        return Err(Refusal::Terms);
    }
    let digest = terms.digest();
    let existing = records.iter().find(|record| record.nonce == terms.nonce);
    if existing.is_some_and(|record| record.genesis.is_some() && record.terms != digest) {
        return Err(Refusal::OtherGenesis);
    }
    if !has_room(policy, records, running_elsewhere, &terms.nonce, now) {
        return Err(Refusal::Full);
    }
    let endpoint = policy
        .endpoint
        .filter(|endpoint| Acceptance::valid_endpoint(endpoint))
        .ok_or(Refusal::NoEndpoint)?;
    let keys = ValidatorKeys::derive(node, terms.network, &terms.nonce).ok_or(Refusal::Busy)?;
    let acceptance = keys
        .accept(node, terms, endpoint.into())
        .filter(|acceptance| acceptance.keys_usable(&terms.escrow))
        .ok_or(Refusal::Terms)?;
    let record = match existing {
        Some(record) if record.terms == digest => record.clone(),
        _ => Record {
            nonce: terms.nonce,
            sponsor: terms.sponsor,
            terms: digest,
            accepted_ms: now,
            genesis: None,
            running: false,
        },
    };
    Ok((acceptance, record))
}

/// Whether `rules` carry an acceptance of this validator's, with the keys
/// it makes under their nonce.
fn ours(node: &IdentityKey, rules: &Rules) -> Result<ValidatorKeys, Refusal> {
    let keys = ValidatorKeys::derive(node, rules.terms.network, &rules.terms.nonce)
        .ok_or(Refusal::Busy)?;
    let own = node.identity();
    let consensus = keys.consensus.public_key();
    let escrow = *keys.escrow.public_key().as_bytes();
    let payout = keys.payout.public().ok_or(Refusal::Busy)?;
    let found = rules.validators.iter().any(|acceptance| {
        acceptance.node == own
            && acceptance.consensus == consensus
            && acceptance.escrow == escrow
            && acceptance.payout == payout.key
            && acceptance.payout_chain == payout.chain
    });
    if found {
        Ok(keys)
    } else {
        Err(Refusal::NotInRules)
    }
}

/// Signs the genesis of `rules` stamped `genesis_ms`, asked by `from`: the
/// consensus key's signature, and the record to keep. The same genesis
/// again gets the same signature; another under the nonce is refused.
///
/// # Errors
///
/// Why it does not sign.
pub fn sign_genesis(
    node: &IdentityKey,
    policy: &Policy<'_>,
    (records, running_elsewhere): (&[Record], usize),
    from: &NodeIdentity,
    (rules, genesis_ms): (&Rules, Millis),
    now: Millis,
) -> Result<(Signature, Record), Refusal> {
    allowed(policy, from, &rules.terms.sponsor)?;
    let record = genesis_record(policy, (records, running_elsewhere), rules, genesis_ms, now)?;
    let keys = ours(node, rules)?;
    let digest = genesis_digest(rules, genesis_ms);
    Ok((keys.consensus.sign(digest.as_bytes()), record))
}

/// The record of a validator that signs, or runs, the genesis of `rules`
/// stamped `genesis_ms`: refused if it signed another under the nonce, if
/// it has no room, if the rules do not check, or if it would sign it for
/// the first time far from its clock.
fn genesis_record(
    policy: &Policy<'_>,
    (records, running_elsewhere): (&[Record], usize),
    rules: &Rules,
    genesis_ms: Millis,
    now: Millis,
) -> Result<Record, Refusal> {
    if rules.terms.network != policy.network || rules.check().is_err() {
        return Err(Refusal::Terms);
    }
    let nonce = rules.terms.nonce;
    let existing = records.iter().find(|record| record.nonce == nonce);
    if let Some((signed, at)) = existing.and_then(|record| record.genesis.as_ref()) {
        return if signed.id() == rules.id() && *at == genesis_ms {
            existing.cloned().ok_or(Refusal::Busy)
        } else {
            Err(Refusal::OtherGenesis)
        };
    }
    let timely = genesis_ms <= now.saturating_add(MAX_GENESIS_AHEAD_MS)
        && now.saturating_sub(genesis_ms) <= MAX_GENESIS_AGE_MS;
    if !timely {
        return Err(Refusal::Time);
    }
    if !has_room(policy, records, running_elsewhere, &nonce, now) {
        return Err(Refusal::Full);
    }
    Ok(Record {
        nonce,
        sponsor: rules.terms.sponsor,
        terms: rules.terms.digest(),
        accepted_ms: existing.map_or(now, |record| record.accepted_ms),
        genesis: Some((rules.clone(), genesis_ms)),
        running: false,
    })
}

/// Takes `certificate`, delivered by `from`: the record of the hub it now
/// runs. A validator whose signature the certificate lacks still runs the
/// hub, if it signed no other genesis under the nonce: the certificate
/// shows a quorum committed to the rules, its acceptance among them.
///
/// # Errors
///
/// Why it does not run the hub.
pub fn take_certificate(
    node: &IdentityKey,
    policy: &Policy<'_>,
    (records, running_elsewhere): (&[Record], usize),
    from: &NodeIdentity,
    certificate: &GenesisCertificate,
    now: Millis,
) -> Result<Record, Refusal> {
    let rules = &certificate.rules;
    allowed(policy, from, &rules.terms.sponsor)?;
    if rules.terms.network != policy.network || !certificate.verifies() {
        return Err(Refusal::Certificate);
    }
    ours(node, rules)?;
    let existing = records
        .iter()
        .find(|record| record.nonce == rules.terms.nonce);
    let record = match existing {
        Some(record) if record.genesis.is_some() => {
            let same = record.hub() == Some(rules.id())
                && record
                    .genesis
                    .as_ref()
                    .is_some_and(|(_, at)| *at == certificate.genesis_ms);
            if !same {
                return Err(Refusal::OtherGenesis);
            }
            record.clone()
        }
        // The timestamp is the quorum's to judge: it signed it.
        _ => {
            if !has_room(policy, records, running_elsewhere, &rules.terms.nonce, now) {
                return Err(Refusal::Full);
            }
            Record {
                nonce: rules.terms.nonce,
                sponsor: rules.terms.sponsor,
                terms: rules.terms.digest(),
                accepted_ms: existing.map_or(now, |record| record.accepted_ms),
                genesis: Some((rules.clone(), certificate.genesis_ms)),
                running: false,
            }
        }
    };
    Ok(Record {
        running: true,
        ..record
    })
}

/// The records that still hold a place at `now`; the rest lapsed.
#[must_use]
pub fn kept(records: Vec<Record>, now: Millis) -> Vec<Record> {
    records
        .into_iter()
        .filter(|record| record.holds_place(now))
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::founding::GenesisCertificate;
    use crate::rules::tests::terms;
    use crate::validators::ValidatorIndex;

    const NOW: Millis = 1_800_000_000_000;

    struct Validators {
        sponsor: IdentityKey,
        nodes: Vec<IdentityKey>,
        sponsors: Vec<NodeId>,
    }

    impl Validators {
        fn new() -> Self {
            let sponsor = dassie_identity::TableKeygen::identity_key(&[100; 32]);
            let nodes = (1..=4)
                .map(|i| dassie_identity::TableKeygen::identity_key(&[i; 32]))
                .collect();
            let sponsors = vec![sponsor.node_id()];
            Self {
                sponsor,
                nodes,
                sponsors,
            }
        }

        fn policy(&self) -> Policy<'_> {
            Policy {
                sponsors: &self.sponsors,
                max_hubs: 2,
                endpoint: Some("v.example:443"),
                network: 1,
            }
        }

        /// Every validator accepts; the rules and each one's record.
        fn rules(&self, terms: &Terms) -> (Rules, Vec<Record>) {
            let from = self.sponsor.identity();
            let (validators, records) = self
                .nodes
                .iter()
                .map(|node| accept(node, &self.policy(), (&[], 0), &from, terms, NOW).unwrap())
                .unzip();
            (
                Rules {
                    terms: terms.clone(),
                    validators,
                },
                records,
            )
        }
    }

    #[test]
    fn a_listed_sponsor_founds_a_hub() {
        let v = Validators::new();
        let terms = terms(&v.sponsor);
        let (rules, records) = v.rules(&terms);
        rules.check().unwrap();
        let from = v.sponsor.identity();
        let mut signatures = Vec::new();
        let mut signed = Vec::new();
        for (i, node) in v.nodes.iter().enumerate() {
            let record = core::slice::from_ref(&records[i]);
            let (signature, kept) =
                sign_genesis(node, &v.policy(), (record, 0), &from, (&rules, NOW), NOW).unwrap();
            // Asked again, it signs the same.
            let again = sign_genesis(
                node,
                &v.policy(),
                (core::slice::from_ref(&kept), 0),
                &from,
                (&rules, NOW),
                NOW + 5,
            )
            .unwrap();
            assert_eq!(again.0, signature);
            signatures.push((ValidatorIndex::new(i).unwrap(), signature));
            signed.push(kept);
        }
        let certificate = GenesisCertificate {
            rules: rules.clone(),
            genesis_ms: NOW,
            signatures: signatures[..3].to_vec(),
        };
        assert!(certificate.verifies());
        for (i, node) in v.nodes.iter().enumerate() {
            let record = core::slice::from_ref(&signed[i]);
            let running =
                take_certificate(node, &v.policy(), (record, 0), &from, &certificate, NOW).unwrap();
            assert!(running.running);
            assert_eq!(running.hub(), Some(rules.id()));
        }
    }

    #[test]
    fn only_listed_sponsors_and_only_their_own_requests() {
        let v = Validators::new();
        let terms = terms(&v.sponsor);
        let node = &v.nodes[0];
        let from = v.sponsor.identity();
        let unlisted = Policy {
            sponsors: &[],
            ..v.policy()
        };
        let refused = accept(node, &unlisted, (&[], 0), &from, &terms, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::NotAllowed);
        // Someone else passing the sponsor's terms on is not the sponsor.
        let stranger = dassie_identity::TableKeygen::identity_key(&[50; 32]).identity();
        let refused = accept(node, &v.policy(), (&[], 0), &stranger, &terms, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::NotAllowed);
        let no_endpoint = Policy {
            endpoint: None,
            ..v.policy()
        };
        let refused = accept(node, &no_endpoint, (&[], 0), &from, &terms, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::NoEndpoint);
        let mut bad = terms.clone();
        bad.fees.operation = 0;
        let refused = accept(node, &v.policy(), (&[], 0), &from, &bad, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::Terms);
        // Terms on another network than the owner's are refused; on the
        // owner's, they are accepted with keys of that network.
        let mut mainnet = terms.clone();
        mainnet.network = 0;
        let refused = accept(node, &v.policy(), (&[], 0), &from, &mainnet, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::Terms);
        let on_mainnet = Policy {
            network: 0,
            ..v.policy()
        };
        let refused = accept(node, &on_mainnet, (&[], 0), &from, &terms, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::Terms);
        let (acceptance, _) = accept(node, &on_mainnet, (&[], 0), &from, &mainnet, NOW).unwrap();
        let theirs = ValidatorKeys::derive(node, 0, &mainnet.nonce).unwrap();
        assert_eq!(acceptance.consensus, theirs.consensus.public_key());
        let testnet = ValidatorKeys::derive(node, 1, &mainnet.nonce).unwrap();
        assert_ne!(acceptance.consensus, testnet.consensus.public_key());
    }

    // Hubs it runs and foundings still open hold its places; a lapsed
    // acceptance frees one.
    #[test]
    fn over_the_bound_it_refuses() {
        let v = Validators::new();
        let node = &v.nodes[0];
        let from = v.sponsor.identity();
        let policy = v.policy();
        let terms_at = |nonce: u8| Terms {
            version: crate::rules::HUB_VERSION,
            nonce: [nonce; 32],
            ..terms(&v.sponsor)
        };
        let (_, first) = accept(node, &policy, (&[], 0), &from, &terms_at(1), NOW).unwrap();
        let records = vec![first.clone()];
        let full = accept(node, &policy, (&records, 1), &from, &terms_at(2), NOW);
        assert_eq!(full.unwrap_err(), Refusal::Full);
        // The same terms again take no new place.
        let again = accept(node, &policy, (&records, 1), &from, &terms_at(1), NOW).unwrap();
        assert_eq!(again.1, first);
        let later = NOW + ACCEPTANCE_TTL_MS;
        assert!(accept(node, &policy, (&records, 1), &from, &terms_at(2), later).is_ok());
        assert!(kept(records, later).is_empty());
    }

    // A validator signs one genesis per nonce: neither other rules nor
    // another timestamp under it, and no certificate of either.
    #[test]
    fn one_genesis_per_nonce() {
        let v = Validators::new();
        let terms = terms(&v.sponsor);
        let (rules, records) = v.rules(&terms);
        let node = &v.nodes[0];
        let from = v.sponsor.identity();
        let policy = v.policy();
        let mine = core::slice::from_ref(&records[0]);
        let (_, signed) =
            sign_genesis(node, &policy, (mine, 0), &from, (&rules, NOW), NOW).unwrap();
        let signed = vec![signed];
        let later = sign_genesis(node, &policy, (&signed, 0), &from, (&rules, NOW + 1), NOW);
        assert_eq!(later.unwrap_err(), Refusal::OtherGenesis);
        // Rules with another validator in place of one.
        let mut other = rules.clone();
        let fifth = dassie_identity::TableKeygen::identity_key(&[5; 32]);
        other.validators[3] = accept(&fifth, &policy, (&[], 0), &from, &terms, NOW)
            .unwrap()
            .0;
        let refused = sign_genesis(node, &policy, (&signed, 0), &from, (&other, NOW), NOW);
        assert_eq!(refused.unwrap_err(), Refusal::OtherGenesis);
        // Other terms under the nonce are refused too, once it signed.
        let mut changed = terms.clone();
        changed.fees.operation = 11;
        let refused = accept(node, &policy, (&signed, 0), &from, &changed, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::OtherGenesis);
        let digest = crate::founding::genesis_digest(&other, NOW);
        let signatures = [1, 2, 3]
            .iter()
            .map(|&i| {
                let keys = ValidatorKeys::derive(
                    if i == 3 { &fifth } else { &v.nodes[i] },
                    terms.network,
                    &terms.nonce,
                )
                .unwrap();
                (
                    ValidatorIndex::new(i).unwrap(),
                    keys.consensus.sign(digest.as_bytes()),
                )
            })
            .collect();
        let certificate = GenesisCertificate {
            rules: other,
            genesis_ms: NOW,
            signatures,
        };
        assert!(certificate.verifies());
        let refused = take_certificate(node, &policy, (&signed, 0), &from, &certificate, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::OtherGenesis);
    }

    // Rules that do not carry its own acceptance as it makes it, or a
    // genesis stamped far from its clock, it does not sign.
    #[test]
    fn rules_must_match_its_acceptance_and_its_clock() {
        let v = Validators::new();
        let terms = terms(&v.sponsor);
        let (rules, records) = v.rules(&terms);
        let node = &v.nodes[0];
        let from = v.sponsor.identity();
        let policy = v.policy();
        let mine = core::slice::from_ref(&records[0]);
        // Its place taken by another node's acceptance.
        let mut without = rules.clone();
        let fifth = dassie_identity::TableKeygen::identity_key(&[5; 32]);
        without.validators[0] = accept(&fifth, &policy, (&[], 0), &from, &terms, NOW)
            .unwrap()
            .0;
        let refused = sign_genesis(node, &policy, (mine, 0), &from, (&without, NOW), NOW);
        assert_eq!(refused.unwrap_err(), Refusal::NotInRules);
        // An acceptance in its name whose keys are not its own does not
        // verify, so the rules do not check.
        let mut forged = rules.clone();
        forged.validators[0].consensus = fifth.public_key();
        let refused = sign_genesis(node, &policy, (mine, 0), &from, (&forged, NOW), NOW);
        assert_eq!(refused.unwrap_err(), Refusal::Terms);
        for at in [NOW + MAX_GENESIS_AHEAD_MS + 1, NOW - MAX_GENESIS_AGE_MS - 1] {
            let refused = sign_genesis(node, &policy, (mine, 0), &from, (&rules, at), NOW);
            assert_eq!(refused.unwrap_err(), Refusal::Time);
        }
        // A certificate with too few signatures founds nothing.
        let digest = crate::founding::genesis_digest(&rules, NOW);
        let keys = ValidatorKeys::derive(&v.nodes[1], terms.network, &terms.nonce).unwrap();
        let certificate = GenesisCertificate {
            rules,
            genesis_ms: NOW,
            signatures: vec![(
                ValidatorIndex::new(1).unwrap(),
                keys.consensus.sign(digest.as_bytes()),
            )],
        };
        let refused = take_certificate(node, &policy, (mine, 0), &from, &certificate, NOW);
        assert_eq!(refused.unwrap_err(), Refusal::Certificate);
    }
}
