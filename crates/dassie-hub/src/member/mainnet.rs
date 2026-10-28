//! The default mainnet hubs and the default list of mainnet validators
//! they are checked against (spec/designs/settlement-hubs.md#11-choosing-a-hub,
//! spec/areas/hubs.md#in-the-node).
//!
//! A built-in mainnet hub holds real money, so its rules are checked before
//! a release carries them: they decode and check, their terms are on
//! mainnet and hold the asset its scheme names, and every validator of it
//! is on the list ([`check_default_hub`]). A test runs the check on each
//! one built in, and on a hub it builds itself while none is.

use dassie_identity::{NodeId, NodeIdentity, PublicKey, SlhDsaPublicKey};

use super::scheme::{MAINNET_SCHEME_ID, RLUSD_SCHEME_ID, scheme_of};
use crate::rules::{Asset, Rules, RulesError};

/// A validator on Dassie's default list of mainnet validators: a node, the
/// key its acceptances are signed with and its SLH-DSA key, its operator
/// and the operator's jurisdiction. An operator has one entry, and an
/// operator's testnet entry vouches for nothing on mainnet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListedValidator {
    /// The node's ID, as its text.
    pub node: &'static str,
    /// The node's public key, as 64 hex digits.
    pub key: &'static str,
    /// The node's SLH-DSA public key, as 64 hex digits: with [`Self::key`],
    /// the keys whose ID [`Self::node`] is.
    pub slh_dsa: &'static str,
    /// Who runs it.
    pub operator: &'static str,
    /// Where its operator answers for it.
    pub jurisdiction: &'static str,
}

/// Dassie's default list of mainnet validators: those a default mainnet hub
/// may name. It belongs with the mainnet bootstrap node list
/// (spec/decisions/real-money.md), and moves there once that list exists;
/// until then it is kept here, beside the default hubs it vouches for.
///
/// Empty until the project's mainnet validators run: an operator's step,
/// which adds an entry for each, by its node ID, its two keys, operator and
/// jurisdiction.
pub const DEFAULT_MAINNET_VALIDATORS: &[ListedValidator] = &[];

/// Why a list of validators is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListError {
    /// Entry `i`'s keys are no node's, or its ID is not the keys'.
    Entry(usize),
    /// Entry `i` names a node or an operator an earlier entry names.
    Repeated(usize),
}

/// Checks a list of validators: each pair of keys is a node's whose ID the
/// entry names, and no node or operator is named twice.
///
/// # Errors
///
/// See [`ListError`].
pub fn check_list(list: &[ListedValidator]) -> Result<(), ListError> {
    for (index, entry) in list.iter().enumerate() {
        let node = listed_node(entry).ok_or(ListError::Entry(index))?;
        let id = NodeId::parse(entry.node).map_err(|_| ListError::Entry(index))?;
        if node.node_id() != id {
            return Err(ListError::Entry(index));
        }
        let repeated = list.iter().take(index).any(|earlier| {
            earlier.operator == entry.operator
                || NodeId::parse(earlier.node).is_ok_and(|earlier| earlier == id)
        });
        if repeated {
            return Err(ListError::Repeated(index));
        }
    }
    Ok(())
}

/// The identity of a listed validator, if its hex is a pair of keys.
fn listed_node(entry: &ListedValidator) -> Option<NodeIdentity> {
    let bytes: [u8; 32] = dassie_xrpl::from_hex(entry.key)?.try_into().ok()?;
    let slh_dsa: [u8; 32] = dassie_xrpl::from_hex(entry.slh_dsa)?.try_into().ok()?;
    let key = PublicKey::from_bytes(&bytes).ok()?;
    Some(NodeIdentity::new(key, SlhDsaPublicKey::from_bytes(slh_dsa)))
}

/// Why a default mainnet hub is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultHubError {
    /// The hex does not decode as rules, or leaves bytes over.
    Encoding,
    /// The rules do not check ([`Rules::check`]).
    Rules(RulesError),
    /// Its terms are not on mainnet.
    Network,
    /// It holds another asset than its scheme's: XRP for
    /// [`MAINNET_SCHEME_ID`], RLUSD of its issuer for [`RLUSD_SCHEME_ID`].
    Asset,
    /// Validator `i` of its rules is not on the list.
    Unlisted(usize),
}

/// Checks the rules `hex` built in as the default hub of mainnet scheme
/// `scheme` against the list `validators`: they decode and check, their
/// terms are on mainnet and hold the scheme's asset, XRP for
/// [`MAINNET_SCHEME_ID`] and RLUSD of [`dassie_xrpl::rlusd::ISSUER`] for
/// [`RLUSD_SCHEME_ID`], and each of their validators is on the list by its
/// identity, both its keys. The rules, if so.
///
/// # Errors
///
/// See [`DefaultHubError`].
pub fn check_default_hub(
    scheme: &str,
    hex: &str,
    validators: &[ListedValidator],
) -> Result<Rules, DefaultHubError> {
    let bytes = dassie_xrpl::from_hex(hex.trim()).ok_or(DefaultHubError::Encoding)?;
    let mut reader = dassie_oer::Reader::new(&bytes);
    let rules = Rules::decode(&mut reader).map_err(|_| DefaultHubError::Encoding)?;
    reader.finish().map_err(|_| DefaultHubError::Encoding)?;
    rules.check().map_err(DefaultHubError::Rules)?;
    if rules.terms.network != dassie_xrpl::network::MAINNET.id {
        return Err(DefaultHubError::Network);
    }
    let asset = match scheme {
        MAINNET_SCHEME_ID => Asset::Xrp,
        RLUSD_SCHEME_ID => Asset::Token {
            currency: dassie_xrpl::rlusd::CURRENCY,
            issuer: dassie_xrpl::rlusd::ISSUER,
        },
        _ => return Err(DefaultHubError::Asset),
    };
    let named = scheme_of(&rules.terms).map(|(named, _)| named);
    if rules.terms.asset != asset || named != Some(scheme) {
        return Err(DefaultHubError::Asset);
    }
    for (index, acceptance) in rules.validators.iter().enumerate() {
        let listed = validators
            .iter()
            .any(|entry| listed_node(entry).is_some_and(|node| node == acceptance.node));
        if !listed {
            return Err(DefaultHubError::Unlisted(index));
        }
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec::Vec;

    use dassie_identity::IdentityKey;
    use dassie_oer::Encode;

    use super::*;
    use crate::member::{DEFAULT_MAINNET_HUB, DEFAULT_MAINNET_RLUSD_HUB};
    use crate::rules::tests::{rules, token_terms};

    fn hex(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| alloc::format!("{byte:02x}"))
            .collect()
    }

    /// The tests' hub of four validators, on network `network` holding
    /// `asset`, as its validators accept it again: its hex, and its
    /// validators listed.
    fn hub_on(network: u32, asset: Asset) -> (String, Vec<ListedValidator>) {
        let (mut rules, _) = rules(4);
        rules.terms.network = network;
        if asset != Asset::Xrp {
            rules.terms = token_terms(rules.terms);
        }
        rules.terms.asset = asset;
        let terms = rules.terms.clone();
        let keys: Vec<IdentityKey> = (1..=4).map(crate::rules::tests::node_key).collect();
        for (acceptance, key) in rules.validators.iter_mut().zip(&keys) {
            *acceptance = acceptance.clone().sign(key, &terms);
        }
        rules.check().unwrap();
        let listed = keys
            .iter()
            .map(|key| {
                let node: &'static str = String::from(key.node_id().as_str()).leak();
                let slh_dsa: &'static str = hex(key.identity().slh_dsa().as_bytes()).leak();
                let key: &'static str = hex(key.public_key().as_bytes()).leak();
                ListedValidator {
                    node,
                    key,
                    slh_dsa,
                    operator: node,
                    jurisdiction: "CH",
                }
            })
            .collect();
        (hex(&rules.to_vec()), listed)
    }

    fn rlusd() -> Asset {
        Asset::Token {
            currency: dassie_xrpl::rlusd::CURRENCY,
            issuer: dassie_xrpl::rlusd::ISSUER,
        }
    }

    // spec/designs/settlement-hubs.md#11-choosing-a-hub: each default
    // mainnet hub built in holds its scheme's asset on mainnet, and only
    // validators on the default list run it; the list names each node by
    // its key, once, and each operator once.
    #[test]
    fn the_built_in_mainnet_hubs_are_on_mainnet_with_listed_validators() {
        check_list(DEFAULT_MAINNET_VALIDATORS).unwrap();
        for (scheme, built_in) in [
            (MAINNET_SCHEME_ID, DEFAULT_MAINNET_HUB),
            (RLUSD_SCHEME_ID, DEFAULT_MAINNET_RLUSD_HUB),
        ] {
            if let Some(hex) = built_in {
                let checked = check_default_hub(scheme, hex, DEFAULT_MAINNET_VALIDATORS);
                assert!(checked.is_ok(), "{scheme}: {checked:?}");
            }
        }
    }

    // The check passes a mainnet hub of XRP or of RLUSD whose validators
    // are all listed, and refuses one on the testnet, of another asset or
    // token issuer, with one validator off the list, or that does not
    // decode or check.
    #[test]
    fn a_default_mainnet_hub_is_checked() {
        let mainnet = dassie_xrpl::network::MAINNET.id;
        let testnet = crate::ledger::TESTNET_NETWORK;
        let (xrp, listed) = hub_on(mainnet, Asset::Xrp);
        check_list(&listed).unwrap();
        let rules = check_default_hub(MAINNET_SCHEME_ID, &xrp, &listed).unwrap();
        assert_eq!(rules.terms.network, mainnet);
        let (token, token_listed) = hub_on(mainnet, rlusd());
        assert!(check_default_hub(RLUSD_SCHEME_ID, &token, &token_listed).is_ok());

        // Each scheme takes only its own asset.
        let wrong = |scheme, hex: &str| check_default_hub(scheme, hex, &listed);
        assert_eq!(wrong(RLUSD_SCHEME_ID, &xrp), Err(DefaultHubError::Asset));
        assert_eq!(
            wrong(MAINNET_SCHEME_ID, &token),
            Err(DefaultHubError::Asset)
        );
        assert_eq!(
            wrong("hub-xrpl-testnet-xrp", &xrp),
            Err(DefaultHubError::Asset)
        );
        let dsusd = Asset::Token {
            currency: dassie_xrpl::dsusd::CURRENCY,
            issuer: dassie_xrpl::dsusd::ISSUER,
        };
        let (other_issuer, _) = hub_on(mainnet, dsusd);
        assert_eq!(
            wrong(RLUSD_SCHEME_ID, &other_issuer),
            Err(DefaultHubError::Asset)
        );
        // Only mainnet.
        let (on_testnet, _) = hub_on(testnet, Asset::Xrp);
        assert_eq!(
            wrong(MAINNET_SCHEME_ID, &on_testnet),
            Err(DefaultHubError::Network)
        );
        // Every validator listed.
        assert_eq!(
            check_default_hub(MAINNET_SCHEME_ID, &xrp, &listed[1..]),
            Err(DefaultHubError::Unlisted(0))
        );
        assert_eq!(
            check_default_hub(MAINNET_SCHEME_ID, &xrp, &[]),
            Err(DefaultHubError::Unlisted(0))
        );
        // Rules that do not decode, or do not check.
        assert_eq!(
            wrong(MAINNET_SCHEME_ID, "zz"),
            Err(DefaultHubError::Encoding)
        );
        assert_eq!(
            wrong(MAINNET_SCHEME_ID, &alloc::format!("{xrp}00")),
            Err(DefaultHubError::Encoding)
        );
        let (mut broken, _) = crate::rules::tests::rules(4);
        broken.terms.network = mainnet;
        assert!(matches!(
            wrong(MAINNET_SCHEME_ID, &hex(&broken.to_vec())),
            Err(DefaultHubError::Rules(RulesError::Acceptance(0)))
        ));
    }

    // The list names each node by a key whose ID it is, once, and each
    // operator once.
    #[test]
    fn the_list_is_checked() {
        let (_, listed) = hub_on(dassie_xrpl::network::MAINNET.id, Asset::Xrp);
        check_list(&listed).unwrap();
        let mut wrong_id = listed.clone();
        wrong_id[1].node = listed[2].node;
        assert_eq!(check_list(&wrong_id), Err(ListError::Entry(1)));
        let mut not_a_key = listed.clone();
        not_a_key[0].key = "00";
        assert_eq!(check_list(&not_a_key), Err(ListError::Entry(0)));
        let mut twice = listed.clone();
        twice[3] = listed[0];
        twice[3].operator = "another";
        assert_eq!(check_list(&twice), Err(ListError::Repeated(3)));
        let mut one_operator = listed.clone();
        one_operator[2].operator = listed[0].operator;
        assert_eq!(check_list(&one_operator), Err(ListError::Repeated(2)));
    }
}
