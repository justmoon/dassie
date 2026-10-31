//! What an uplink reads of a downlink that asks to be promoted: what its
//! own account in the hub can spend (spec/decisions/promotion-to-core.md).
//! Only a registered candidate's funds are read, the hub's answer counts
//! only for the account the downlink named, and its URL is checked only
//! once its funds are enough. On one engine core in the XRP hub, with a
//! downlink peered over it.

use std::sync::Arc;

use dassie_hub::client::{AccountView, Statement};
use dassie_hub::member::SCHEME_ID;
use dassie_hub::operation::AccountNumber;
use dassie_identity::{IdentityKey, NodeId};
use dassie_oer::Encode;
use dassie_peer::{Promotion, PromotionCheck};
use dassie_settlement::policy;
use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::fake::FakeLedger;

use super::super::{Core, Output};
use super::hold_allotment::peer_over_hub;
use super::{
    Policies, SHARED_CONFIG, Validators, add_xrpl, core_with_bootstrap, deposits, follow_xrpl,
    funded_ledger, hub_config, send_link_state, wait_and_close,
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

/// A core node in the XRP hub, with a downlink peered over it as account
/// [`DOWNLINK_ACCOUNT`] whose link state has [`URL`]: the core, the hub and
/// the downlink.
fn candidate(ledger: &mut FakeLedger) -> (Core, Validators, Arc<IdentityKey>) {
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
    send_link_state(&mut core, &downlink, at + 1, Some(URL));
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
    let (mut core, mut hub, downlink) = candidate(&mut ledger);
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

// The hub's answer counts only for the account the downlink named: a key
// that holds another is not funded by it, and a key without one has none.
#[test]
fn a_key_with_another_account_is_not_funded() {
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub, downlink) = candidate(&mut ledger);
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
    let (mut core, mut hub, downlink) = candidate(&mut ledger);
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
