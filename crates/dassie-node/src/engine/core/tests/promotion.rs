//! What an uplink reads of a downlink that asks to be promoted: what its
//! own account in the hub can spend (spec/decisions/promotion-to-core.md).
//! Only a registered candidate's funds are read, the hub's answer counts
//! only for the account the downlink named and a key its link state proves
//! its own, and its URL is checked only once its funds are enough. On one
//! engine core in the XRP hub, with a downlink peered over it.

use std::sync::Arc;

use dassie_hub::client::{AccountView, Statement};
use dassie_hub::member::SCHEME_ID;
use dassie_hub::operation::AccountNumber;
use dassie_identity::{IdentityKey, NodeId};
use dassie_oer::{Decode, Encode};
use dassie_peer::{Promotion, PromotionCheck};
use dassie_settlement::policy;
use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::fake::FakeLedger;

use super::super::{Core, Output};
use super::hold_allotment::peer_over_hub;
use super::{
    Policies, SHARED_CONFIG, Validators, add_xrpl, core_with_bootstrap, deposits, follow_xrpl,
    funded_ledger, hub_config, wait_and_close,
};
use crate::store::Store;

/// The downlink's account in the hub, and the seed of its key there.
const DOWNLINK_ACCOUNT: u32 = 10;
const DOWNLINK_SEED: u8 = 12;
const URL: &str = "https://d.example:4443";

/// What the downlink's account must spend, in drops: one request at the
/// default credit for the one edge a network of one bootstrap node asks.
fn enough() -> u64 {
    let units = policy::promotion_funds(
        Policies::DEFAULT.connector.max_packet_amount,
        Policies::DEFAULT.peer_credit,
        1,
    );
    units / UNITS_PER_DROP
}

/// How the downlink's link state shows its key in the hub is its own.
#[derive(Clone, Copy)]
enum Proof {
    /// Signed by its key, over its own node ID.
    Own,
    /// None: it names a key another node holds.
    Missing,
    /// Copied from the node whose key it names: signed over that node's ID.
    Copied,
}

/// The account key of the seed `seed` in `hub`'s signature saying it is the
/// key of node `node`.
fn key_proof(hub: &Validators, seed: u8, node: NodeId) -> [u8; 64] {
    let key = dassie_hub::member::member_key(
        &dassie_identity::TableKeygen::identity_key(&[seed; 32]),
        &hub.rules.id(),
    );
    let digest = dassie_hub::member::key_proof_digest(&hub.rules.id(), node.as_str());
    *key.sign(digest.as_bytes()).as_bytes()
}

/// `node` sends `core` its link state at `at`, with [`URL`], offering the
/// hub with account [`DOWNLINK_ACCOUNT`], and `proof` of its key there.
fn send_link_state(core: &mut Core, hub: &Validators, node: &Arc<IdentityKey>, proof: Proof) {
    use dassie_peer::wire::{
        Content, EXTENSION_HUB_KEYS, Entry, HubDetails, NodeInfo, SchemeId, SettlementOffer,
        SignedNodeInfo, sign_node_info,
    };

    let hub_id = *hub.rules.id().0.as_bytes();
    let details = HubDetails {
        hub_id,
        account: DOWNLINK_ACCOUNT,
    }
    .to_vec();
    let signature = match proof {
        Proof::Own => Some(key_proof(hub, DOWNLINK_SEED, node.node_id())),
        Proof::Missing => None,
        Proof::Copied => {
            let holder = dassie_identity::TableKeygen::identity_key(&[DOWNLINK_SEED; 32]);
            Some(key_proof(hub, DOWNLINK_SEED, holder.node_id()))
        }
    };
    let value: Vec<u8> = signature
        .map(|signature| hub_id.into_iter().chain(signature).collect())
        .unwrap_or_default();
    let entries = if value.is_empty() {
        vec![]
    } else {
        vec![Entry::Extension {
            kind: EXTENSION_HUB_KEYS,
            value: &value,
        }]
    };
    let at = core.now_ms() + 1;
    let info = NodeInfo {
        sequence: at,
        url: URL,
        alias: "",
        public_key: *node.public_key().as_bytes(),
        slh_dsa_public_key: *node.identity().slh_dsa().as_bytes(),
        settlement: vec![SettlementOffer {
            scheme: SchemeId::parse(SCHEME_ID).unwrap(),
            details: &details,
        }],
        entries,
    };
    let state = sign_node_info(node, &info);
    let update = Content::LinkStateUpdate(SignedNodeInfo::decode_exact(&state).unwrap()).to_vec();
    core.begin_step(at);
    core.peer_inbound(dassie_peer::InboundId(2), node.identity(), &update, None);
    core.finish_step(&mut Vec::new());
}

/// A core node in the XRP hub, with a downlink peered over it as account
/// [`DOWNLINK_ACCOUNT`] whose link state has [`URL`] and `proof` of its key
/// in the hub: the core, the hub and the downlink.
fn candidate(ledger: &mut FakeLedger, proof: Proof) -> (Core, Validators, Arc<IdentityKey>) {
    let mut core = core_with_bootstrap(Arc::new(Store::open_in_memory().unwrap()));
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(SCHEME_ID, hub_config(600, 300)).unwrap();
    follow_xrpl(&mut core, ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    wait_and_close(&mut core, ledger, 1_000);
    let balance = 50_000_000;
    let deposited = deposits(ledger, hub.escrow())[0].amount + balance;
    hub.account(&mut core, balance, deposited);
    let downlink = Arc::new(dassie_identity::TableKeygen::identity_key(&[11; 32]));
    let at = core.now_ms() + 1_000;
    let numbered = (DOWNLINK_ACCOUNT, DOWNLINK_SEED);
    peer_over_hub(&mut core, &hub, &downlink, numbered, at);
    send_link_state(&mut core, &hub, &downlink, proof);
    (core, hub, downlink)
}

/// The hub says the downlink's key has account `number`, which can spend
/// `balance` drops, or no account.
fn says(core: &mut Core, hub: &mut Validators, account: Option<(u32, u64)>) {
    let key = dassie_hub::member::member_key(
        &dassie_identity::TableKeygen::identity_key(&[DOWNLINK_SEED; 32]),
        &hub.rules.id(),
    )
    .public_key();
    hub.height += 1;
    let account = account.map(|(number, balance)| {
        let view = AccountView {
            key,
            balance,
            reserve: hub.rules.terms.fees.account_reserve,
            sequence: 1,
            disbursal: true,
            closing: false,
            deposited: balance,
            withdrawn: 0,
            returned: 0,
        };
        (AccountNumber(number), view)
    });
    core.begin_step(core.now_ms());
    hub.say(
        core,
        &[Statement::Account {
            height: hub.height,
            key,
            account,
        }],
    );
    core.finish_step(&mut Vec::new());
}

/// Where the downlink stands toward its promotion, as the owner sees it,
/// and the checks it does not pass yet.
fn promotion(core: &Core, id: NodeId) -> (Promotion, Vec<PromotionCheck>) {
    let network = core.network();
    let peer = network.peers.into_iter().find(|peer| peer.id == id);
    peer.and_then(|peer| peer.promotion).expect("a candidate")
}

/// Whether the downlink's funds were enough, as last read.
fn funded(core: &Core, id: NodeId) -> Option<bool> {
    promotion(core, id).0.funded
}

/// Steps `core` a second on, answering its requests for its bootstrap
/// node's list with one naming `listed`, as `step_with_list` does; returns
/// whether it asked to check the downlink's URL over HTTPS.
fn step(core: &mut Core, listed: &[NodeId]) -> bool {
    use dassie_peer::wire::{Kind, NodeListResponse};

    let at = core.now_ms() + 1_000;
    let mut asked = false;
    let mut outputs = Vec::new();
    core.begin_step(at);
    core.finish_step(&mut outputs);
    while !outputs.is_empty() {
        let mut next = Vec::new();
        for output in outputs {
            let request = match output {
                Output::CheckHttps { url, .. } if url == URL => {
                    asked = true;
                    continue;
                }
                Output::PeerRequest(request) => request,
                _ => continue,
            };
            let body = match request.kind {
                Kind::NodeListHashRequest => vec![9; 32],
                Kind::NodeListRequest => NodeListResponse(listed.to_vec()).to_vec(),
                _ => continue,
            };
            core.begin_step(at);
            core.peer_response(request.id, Ok(dassie_peer::Response { status: 200, body }));
            core.finish_step(&mut next);
        }
        outputs = next;
    }
    asked
}

/// Steps `core` until the bootstrap node's list names `id`: it is
/// registered, and asked to read its funds; their URL is not checked yet.
fn register(core: &mut Core, id: NodeId) {
    let start = core.now_ms();
    loop {
        assert!(!step(core, &[id]), "its URL waits for its funds");
        if promotion(core, id).0.registered {
            // Its funds are asked about at the next step.
            assert!(!step(core, &[id]), "its URL waits for its funds");
            break;
        }
        assert!(core.now_ms() < start + 600_000, "it registers");
    }
}

// A registered candidate's funds are read; short of what promotion asks,
// its URL waits; with enough, it is checked over HTTPS too.
#[test]
fn a_registered_candidates_funds_are_read_and_its_url_checked_once_funded() {
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub, downlink) = candidate(&mut ledger, Proof::Own);
    let id = downlink.node_id();
    register(&mut core, id);
    says(&mut core, &mut hub, Some((DOWNLINK_ACCOUNT, enough() - 1)));
    assert_eq!(funded(&core, id), Some(false), "short by a drop");
    assert!(!step(&mut core, &[id]), "its URL waits");

    // A bucket of its standing later, read anew: enough.
    let bucket = core.peer.config().promotion_standing_ms / dassie_peer::STANDING_BUCKETS;
    let mut asked = false;
    let until = core.now_ms() + bucket + 2_000;
    while core.now_ms() < until && funded(&core, id) != Some(true) {
        step(&mut core, &[id]);
        says(&mut core, &mut hub, Some((DOWNLINK_ACCOUNT, enough())));
    }
    assert_eq!(funded(&core, id), Some(true));
    for _ in 0..3 {
        asked |= step(&mut core, &[id]);
    }
    assert!(asked, "the node checks its URL over HTTPS");
    // What comes back for that URL is what the owner sees.
    let pending = |core: &Core| promotion(core, id).1;
    assert!(pending(&core).contains(&PromotionCheck::Https));
    core.begin_step(core.now_ms());
    core.https_checked(&id, "https://other.example", true);
    core.finish_step(&mut Vec::new());
    assert!(
        pending(&core).contains(&PromotionCheck::Https),
        "another URL's"
    );
    core.begin_step(core.now_ms());
    core.https_checked(&id, URL, true);
    core.finish_step(&mut Vec::new());
    assert!(!pending(&core).contains(&PromotionCheck::Https));
    assert!(!pending(&core).contains(&PromotionCheck::Funded));
}

// A downlink may name another member's account and key as its own, as
// that member gives them to anyone who asks: their funds count for it only
// once its link state proves the key its own, a proof over its own node ID
// that a copy from the member does not give (review of #756).
#[test]
fn funds_count_only_for_a_key_the_candidate_proves_its_own() {
    for proof in [Proof::Missing, Proof::Copied] {
        let (mut ledger, _) = funded_ledger();
        let (mut core, mut hub, downlink) = candidate(&mut ledger, proof);
        let id = downlink.node_id();
        register(&mut core, id);
        says(
            &mut core,
            &mut hub,
            Some((DOWNLINK_ACCOUNT, 100 * enough())),
        );
        assert_eq!(funded(&core, id), Some(false), "read, and not its own");
        assert!(!step(&mut core, &[id]), "its URL waits");
    }
}

// The hub's answer counts only for the account the downlink named: a key
// that holds another is not funded by it, and a key without one has none.
#[test]
fn a_key_with_another_account_is_not_funded() {
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub, downlink) = candidate(&mut ledger, Proof::Own);
    let id = downlink.node_id();
    register(&mut core, id);
    says(
        &mut core,
        &mut hub,
        Some((DOWNLINK_ACCOUNT + 1, 100 * enough())),
    );
    assert_eq!(funded(&core, id), Some(false));
}

// A candidate that is not registered costs the node no reads of its funds.
#[test]
fn an_unregistered_candidates_funds_are_not_read() {
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub, downlink) = candidate(&mut ledger, Proof::Own);
    let id = downlink.node_id();
    let start = core.now_ms();
    while core.now_ms() < start + 600_000 {
        step(&mut core, &[]);
    }
    says(
        &mut core,
        &mut hub,
        Some((DOWNLINK_ACCOUNT, 100 * enough())),
    );
    assert_eq!(funded(&core, id), None, "never asked about");
}
