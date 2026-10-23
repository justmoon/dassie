//! Simplex scenarios, after Commonware's `consensus::simplex` tests: all
//! online, one offline, unclean shutdowns, backfill, a slow validator, all
//! recovering, partitions, slow and lossy links, determinism, equivocators,
//! conflicting and invalid votes, impersonators, outdated messages, and
//! reconfiguration; and what Dassie adds: finality within the regional
//! bound, safety across a restart, stalls, loading a later state, and the
//! evidence that colluders leave.
//!
//! Each runs on the simulated network (`dassie_hub::sim`) at production
//! timings, checking every engine's invariants after each step.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_hub::consensus::wire::{Message, Proposal};
use dassie_hub::consensus::{
    Config, Engine, Evidence, PeerId, Recipient, Vote, VoteKind, verify_certificate,
};
use dassie_hub::sim::{Faults, Network, Role, START, SimApp, genesis, hub, key, roles, set};
use dassie_hub::{Digest, ValidatorIndex};
use dassie_identity::{NodeKey, Signature};
use dassie_oer::Encode;

const CONFIG: Config = Config::DEFAULT;

fn honest(count: usize) -> Network {
    Network::new(1, &roles(count, &[]), CONFIG)
}

fn run_for(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn assert_safe(network: &Network) {
    assert_eq!(network.disagreement(), None);
    assert_eq!(network.blamed_honest(), None);
}

fn index(at: usize) -> ValidatorIndex {
    ValidatorIndex::new(at).unwrap()
}

/// Sends `message` to validator `to` as if from a peer.
fn inject(network: &mut Network, to: usize, message: &Message) {
    let bytes = message.to_vec();
    network.act(to, |engine, app, now| {
        engine.handle_message(now, app, PeerId(99), &bytes);
    });
}

#[test]
fn all_online_finalize_every_interval_within_the_regional_bound() {
    for count in [4, 7] {
        let mut network = honest(count);
        run_for(&mut network, 10_000);
        assert_safe(&network);
        for index in 0..count {
            // A block every interval and a few milliseconds.
            assert!(network.height(index) >= 80, "{}", network.height(index));
            // Final within three message delays and the precision of its
            // stamp, by this validator's clock.
            let bound = 3 * CONFIG.max_delay_ms + CONFIG.precision_ms;
            assert!(network.validators[index].slowest_ms < bound);
        }
    }
}

#[test]
fn one_offline_is_skipped() {
    let mut network = honest(4);
    network.crash(3);
    run_for(&mut network, 20_000);
    assert_safe(&network);
    // Three of every four views make a block; the offline leader's view is
    // nullified at once once it has been silent for `skip_views`, instead
    // of after the advance timeout (which would allow some 100).
    for index in 0..3 {
        assert!(network.height(index) >= 140, "{}", network.height(index));
    }
}

#[test]
fn unclean_shutdowns_never_make_a_validator_sign_twice() {
    let mut network = honest(4);
    for round in 0..30u64 {
        let victim = usize::try_from(round * 7 % 4).unwrap();
        network.crash(victim);
        run_for(&mut network, 50 + round * 13 % 400);
        network.start(victim);
        run_for(&mut network, 100 + round * 31 % 300);
    }
    let before = network.honest_floor();
    run_for(&mut network, 5_000);
    assert_safe(&network);
    assert!(network.honest_floor() > before + 10);
    assert!(
        network
            .honest()
            .all(|i| network.validators[i].evidence.is_empty())
    );
}

#[test]
fn a_validator_cut_off_backfills_what_it_missed() {
    let mut network = honest(4);
    run_for(&mut network, 1_000);
    network.partition(&[3]);
    run_for(&mut network, 3_000);
    let missed = network.height(0) - network.height(3);
    assert!(missed > 10, "{missed}");
    network.heal();
    let target = network.height(0);
    run_for(&mut network, 3_000);
    assert_safe(&network);
    assert!(network.height(3) >= target);
    // Within what peers keep: no state had to be loaded.
    assert_eq!(network.state_loads, 0);
}

#[test]
fn a_validator_cut_off_for_long_loads_a_later_state() {
    let mut network = honest(4);
    network.partition(&[3]);
    run_for(&mut network, 15_000);
    network.heal();
    let target = network.height(0);
    run_for(&mut network, 3_000);
    assert_safe(&network);
    assert!(network.state_loads > 0);
    assert!(network.height(3) >= target);
}

#[test]
fn a_slow_validator_does_not_stop_the_others() {
    let mut network = honest(4);
    network.slow.insert(0, 80);
    run_for(&mut network, 10_000);
    assert_safe(&network);
    assert!(network.honest_floor() >= 50);
}

#[test]
fn all_recover_after_crashing_together() {
    let mut network = honest(4);
    run_for(&mut network, 2_000);
    for index in 0..4 {
        network.crash(index);
    }
    run_for(&mut network, 2_000);
    for index in 0..4 {
        network.start(index);
    }
    let before = network.honest_floor();
    run_for(&mut network, 5_000);
    assert_safe(&network);
    assert!(network.honest_floor() > before + 20);
}

#[test]
fn a_partition_without_quorum_stops_and_heals() {
    let mut network = honest(7);
    run_for(&mut network, 1_000);
    network.partition(&[0, 1, 2]);
    run_for(&mut network, 500);
    let stopped = (0..7).map(|i| network.height(i)).max().unwrap();
    run_for(&mut network, 5_000);
    // Neither side has q = 5.
    assert!((0..7).all(|i| network.height(i) <= stopped));
    network.heal();
    run_for(&mut network, 2 * CONFIG.max_retry_ms + 2_000);
    assert_safe(&network);
    assert!(network.honest_floor() > stopped + 5);
}

#[test]
fn slow_and_lossy_links() {
    let mut network = honest(7);
    network.faults = Faults::ROUGH;
    run_for(&mut network, 20_000);
    assert_safe(&network);
    assert!(network.honest_floor() >= 30, "{}", network.honest_floor());
}

#[test]
fn a_seed_determines_the_run() {
    let run = |seed| {
        let mut network = Network::new(seed, &roles(5, &[Role::Equivocate]), CONFIG);
        network.faults = Faults::ROUGH;
        run_for(&mut network, 3_000);
        network.trace
    };
    assert_eq!(run(7), run(7));
    assert_ne!(run(7), run(8));
}

#[test]
fn an_equivocating_leader_is_named_and_skipped() {
    let mut network = Network::new(3, &roles(4, &[Role::Equivocate]), CONFIG);
    run_for(&mut network, 10_000);
    assert_safe(&network);
    let named = network.honest().any(|i| {
        network.validators[i]
            .evidence
            .iter()
            .any(|evidence| evidence.culprits().contains(index(3)))
    });
    assert!(named);
    assert!(network.honest_floor() >= 50);
}

#[test]
fn byzantine_leaders_cost_only_their_views() {
    for role in [Role::Silent, Role::Withhold, Role::Invalid] {
        let mut network = Network::new(4, &roles(7, &[role, role]), CONFIG);
        run_for(&mut network, 10_000);
        assert_safe(&network);
        assert!(
            network.honest_floor() >= 40,
            "{role:?}: {}",
            network.honest_floor()
        );
    }
}

#[test]
fn a_leader_flooding_proposals_crowds_out_no_honest_block() {
    for count in [4, 7] {
        let faulty = (count - 1) / 3;
        let mut network = Network::new(9, &roles(count, &vec![Role::Flood; faulty]), CONFIG);
        run_for(&mut network, 10_000);
        assert_safe(&network);
        assert!(network.honest_floor() >= 50, "{}", network.honest_floor());
        assert!(network.trace.iter().any(|line| line.contains("floods")));
        // Its second proposals of a view are evidence against it.
        let named = network.honest().any(|i| {
            network.validators[i]
                .evidence
                .iter()
                .any(|evidence| evidence.culprits().contains(index(count - 1)))
        });
        assert!(named);
    }
}

#[test]
fn flooded_proposals_take_only_their_views_room() {
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let mut app = SimApp::default();
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    // Validator 1 leads views 1, 5, 9, 13 and 17, all within reach: a
    // thousand proposals each, plausible and not.
    for view in [1u64, 5, 9, 13, 17] {
        for at in 0..1_000u64 {
            let height = if at % 2 == 0 {
                u64::MAX - at
            } else {
                1 + at % view
            };
            let block = dassie_hub::consensus::Block {
                epoch: 0,
                view,
                height,
                parent: Digest([u8::try_from(at % 251).unwrap(); 32]),
                parent_view: view - 1,
                timestamp_ms: START,
                payload: at.to_be_bytes().to_vec(),
            };
            let digest = block.digest(&hub());
            let vote = Vote::sign(
                &hub(),
                &key(1),
                index(1),
                VoteKind::Notarize,
                (0, view),
                digest,
            );
            let bytes = Message::Proposal(Proposal {
                block,
                signature: vote.signature,
            })
            .to_vec();
            engine.handle_message(START + 1, &mut app, PeerId(1), &bytes);
        }
    }
    engine.check_invariants();
    let _ = notarized(&mut engine);
    // View 1 is nullified; view 2's leader proposes on genesis, and the
    // block is held and notarized.
    let nullification = certificate_of(VoteKind::Nullify, 1, Digest::default());
    engine.handle_message(START + 3, &mut app, PeerId(3), &nullification);
    assert_eq!(engine.view(), 2);
    let honest = proposal_on(2, &genesis(), START + 5);
    engine.handle_message(START + 6, &mut app, PeerId(2), &honest);
    engine.check_invariants();
    assert_eq!(notarized(&mut engine).len(), 1);
}

#[test]
fn colluders_that_fork_are_named() {
    // n = 4, f = 1: two colluders sign two blocks of one view, each with
    // one honest validator.
    let mut network = Network::new(5, &roles(4, &[Role::Collude, Role::Collude]), CONFIG);
    run_for(&mut network, 3_000);
    assert!(
        network.disagreement().is_some(),
        "the colluders forked the chain"
    );
    assert_eq!(network.blamed_honest(), None);
    let mut named = dassie_hub::Signers::NONE;
    for index in network.honest() {
        for evidence in &network.validators[index].evidence {
            named = dassie_hub::Signers::from_bits(named.bits() | evidence.culprits().bits());
        }
    }
    // Every validator that signed both is named, and they are f + 1.
    assert_eq!(named, network.faulty());
}

#[test]
fn conflicting_votes_are_evidence() {
    let mut network = honest(4);
    run_for(&mut network, 1_000);
    let view = network.engine(0).view() + 1;
    let signer = index(3);
    let nullify = Vote::sign(
        &hub(),
        &key(3),
        signer,
        VoteKind::Nullify,
        (0, view),
        Digest::default(),
    );
    let finalize = Vote::sign(
        &hub(),
        &key(3),
        signer,
        VoteKind::Finalize,
        (0, view),
        Digest([7; 32]),
    );
    inject(&mut network, 0, &Message::Vote(nullify));
    inject(&mut network, 0, &Message::Vote(finalize));
    let evidence = &network.validators[0].evidence;
    assert!(evidence.iter().any(|evidence| matches!(
        evidence,
        Evidence::Votes { first, second } if *first == nullify && *second == finalize
    )));
}

#[test]
fn invalid_and_impersonated_votes_are_refused() {
    let mut network = honest(4);
    run_for(&mut network, 1_000);
    let view = network.engine(0).view();
    let refused = network.engine(0).stats().refused;
    let mut forged = Vote::sign(
        &hub(),
        &key(2),
        index(2),
        VoteKind::Nullify,
        (0, view),
        Digest::default(),
    );
    forged.signature = Signature::from_bytes([1; 64]);
    inject(&mut network, 0, &Message::Vote(forged));
    // Signed by 2, claiming to be 1.
    let impersonated = Vote {
        signer: index(1),
        ..Vote::sign(
            &hub(),
            &key(2),
            index(2),
            VoteKind::Nullify,
            (0, view),
            Digest::default(),
        )
    };
    inject(&mut network, 0, &Message::Vote(impersonated));
    // Another hub's vote.
    let elsewhere = Vote::sign(
        &Digest([9; 32]),
        &key(2),
        index(2),
        VoteKind::Nullify,
        (0, view),
        Digest::default(),
    );
    inject(&mut network, 0, &Message::Vote(elsewhere));
    inject(
        &mut network,
        0,
        &Message::Fetch(dassie_hub::consensus::wire::Fetch {
            height: 0,
            epoch: 0,
            view: 0,
        }),
    );
    assert_eq!(network.engine(0).stats().refused, refused + 3);
    run_for(&mut network, 1_000);
    assert_safe(&network);
}

#[test]
fn outdated_messages_are_ignored() {
    let mut network = honest(4);
    run_for(&mut network, 2_000);
    let old = Vote::sign(
        &hub(),
        &key(1),
        index(1),
        VoteKind::Nullify,
        (0, 1),
        Digest::default(),
    );
    let proposal = Proposal {
        block: genesis(),
        signature: Signature::from_bytes([0; 64]),
    };
    let height = network.height(0);
    inject(&mut network, 0, &Message::Vote(old));
    inject(&mut network, 0, &Message::Proposal(proposal));
    run_for(&mut network, 500);
    assert_safe(&network);
    assert!(network.height(0) > height);
}

/// A block of `view` by its leader on `parent`, as a proposal.
fn proposal_on(view: u64, parent: &dassie_hub::consensus::Block, stamp: u64) -> Vec<u8> {
    let height = parent.height + 1;
    let payload = dassie_hub::sim::payload(height, 9, None);
    let block = dassie_hub::consensus::Block {
        epoch: 0,
        view,
        height,
        parent: parent.digest(&hub()),
        parent_view: parent.view,
        timestamp_ms: stamp,
        payload,
    };
    let leader = usize::try_from(view % 4).unwrap();
    let vote = Vote::sign(
        &hub(),
        &key(leader),
        index(leader),
        VoteKind::Notarize,
        (0, view),
        block.digest(&hub()),
    );
    Message::Proposal(Proposal {
        block,
        signature: vote.signature,
    })
    .to_vec()
}

/// The certificate of validators 1 to 3 of `kind` for `view` and `block`.
fn certificate_of(kind: VoteKind, view: u64, block: Digest) -> Vec<u8> {
    let votes: Vec<Vote> = (1..4)
        .map(|at| Vote::sign(&hub(), &key(at), index(at), kind, (0, view), block))
        .collect();
    Message::Certificate(dassie_hub::consensus::Certificate {
        epoch: 0,
        kind,
        view,
        block: votes[0].block,
        signers: votes
            .iter()
            .fold(dassie_hub::Signers::NONE, |signers, vote| {
                signers.with(vote.signer)
            }),
        signatures: votes.iter().map(|vote| vote.signature).collect(),
    })
    .to_vec()
}

/// Hands `engine` a message from validator `from`.
fn hand(engine: &mut Engine, app: &mut SimApp, at: u64, from: u64, message: &Message) {
    engine.handle_message(at, app, PeerId(from), &message.to_vec());
    engine.check_invariants();
}

/// `block`, proposed by validator `leader` of its epoch.
fn proposed(block: &dassie_hub::consensus::Block, leader: usize) -> Message {
    let digest = block.digest(&hub());
    let at = (block.epoch, block.view);
    let vote = Vote::sign(
        &hub(),
        &key(leader),
        index(leader),
        VoteKind::Notarize,
        at,
        digest,
    );
    Message::Proposal(Proposal {
        block: block.clone(),
        signature: vote.signature,
    })
}

/// The block of a proposal's bytes.
fn block_of(bytes: &[u8]) -> dassie_hub::consensus::Block {
    let Ok(Message::Proposal(Proposal { block, .. })) = Message::decode_message(bytes) else {
        panic!("a proposal");
    };
    block
}

/// Validators 0 to 3, a change that takes over at height 3, so block 2 is
/// the last of epoch 0, and block 2 notarized with its votes split: two to
/// finalize it (we and 1), two to nullify its view (2 and 3), as seed
/// 5786467219792507 drew it across a partition. It can be neither
/// finalized nor skipped, and nothing may build on it in epoch 0.
fn split_at_the_boundary() -> (
    Engine,
    SimApp,
    dassie_hub::consensus::Block,
    dassie_hub::ValidatorSet,
) {
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let mut app = SimApp::default();
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    let keys = [0, 1, 2, 5]
        .iter()
        .map(|at| key(*at).public_key())
        .collect();
    let next = dassie_hub::ValidatorSet::new(keys).unwrap();
    engine.reconfigure(START, 3, next.clone()).unwrap();
    let b1 = block_of(&proposal_on(1, &genesis(), START));
    hand(&mut engine, &mut app, START + 1, 1, &proposed(&b1, 1));
    for kind in [VoteKind::Notarize, VoteKind::Finalize] {
        let certificate = certificate_of(kind, 1, b1.digest(&hub()));
        let message = Message::decode_message(&certificate).unwrap();
        hand(&mut engine, &mut app, START + 3, 3, &message);
    }
    let b2 = block_of(&proposal_on(2, &b1, START + 100));
    let b2_digest = b2.digest(&hub());
    hand(&mut engine, &mut app, START + 101, 2, &proposed(&b2, 2));
    let notarization = certificate_of(VoteKind::Notarize, 2, b2_digest);
    let message = Message::decode_message(&notarization).unwrap();
    hand(&mut engine, &mut app, START + 103, 3, &message);
    for (at, kind) in [
        (1, VoteKind::Finalize),
        (2, VoteKind::Nullify),
        (3, VoteKind::Nullify),
    ] {
        let vote = Vote::sign(&hub(), &key(at), index(at), kind, (0, 2), b2_digest);
        hand(
            &mut engine,
            &mut app,
            START + 104,
            at as u64,
            &Message::Vote(vote),
        );
    }
    assert_eq!(engine.tip().block.height, 1, "block 2 is not final");
    assert_eq!(engine.view(), 3);
    let _ = std::iter::from_fn(|| engine.poll_output()).count();
    (engine, app, b2, next)
}

#[test]
fn a_repeat_finalizes_an_epochs_last_block_whose_votes_split() {
    let (mut engine, mut app, b2, next) = split_at_the_boundary();
    let b2_digest = b2.digest(&hub());
    // View 3's leader repeats block 2: same height, no payload.
    let repeat = dassie_hub::consensus::Block {
        epoch: 0,
        view: 3,
        height: 2,
        parent: b2_digest,
        parent_view: 2,
        timestamp_ms: START + 200,
        payload: Vec::new(),
    };
    let repeat_digest = repeat.digest(&hub());
    hand(&mut engine, &mut app, START + 201, 3, &proposed(&repeat, 3));
    assert_eq!(
        notarized(&mut engine),
        vec![repeat_digest],
        "we notarize the repeat"
    );
    for kind in [VoteKind::Notarize, VoteKind::Finalize] {
        let certificate = certificate_of(kind, 3, repeat_digest);
        let message = Message::decode_message(&certificate).unwrap();
        hand(&mut engine, &mut app, START + 203, 3, &message);
    }
    // Block 2 is final through the repeat, which the application never
    // saw; the new set runs epoch 1 from view 1, on block 2.
    assert_eq!(engine.tip().block, b2);
    assert_eq!(engine.tip().repeats, vec![repeat]);
    assert_eq!(
        app.finalized.keys().copied().collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!((engine.epoch(), engine.view()), (1, 1));
    assert_eq!(engine.set(), &next);
    // The new set's first block builds on block 2 at view 0, and we
    // notarize it.
    let third = dassie_hub::consensus::Block {
        epoch: 1,
        view: 1,
        height: 3,
        parent: b2_digest,
        parent_view: 0,
        timestamp_ms: START + 300,
        payload: dassie_hub::sim::payload(3, 9, None),
    };
    hand(&mut engine, &mut app, START + 301, 1, &proposed(&third, 1));
    assert_eq!(notarized(&mut engine), vec![third.digest(&hub())]);
}

/// Validator 0 of 0 to 3, with an application whose chain schedules, at
/// block 1, the set `next` to take over at height 3: the least notice, so
/// block 2 is the last of epoch 0. No one has finalized block 1.
fn scheduled_at_one() -> (Engine, SimApp, dassie_hub::sim::Schedule) {
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let keys = [0, 1, 2, 5]
        .iter()
        .map(|at| key(*at).public_key())
        .collect();
    let schedule = dassie_hub::sim::Schedule {
        at: 1,
        from: 3,
        set: dassie_hub::ValidatorSet::new(keys).unwrap(),
    };
    let mut app = SimApp::default();
    app.schedule = Some(schedule.clone());
    let engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    (engine, app, schedule)
}

/// The block of `view` on `parent`, with the payload the schedule asks for.
fn scheduled_block(
    view: u64,
    parent: &dassie_hub::consensus::Block,
    schedule: &dassie_hub::sim::Schedule,
) -> dassie_hub::consensus::Block {
    let height = parent.height + 1;
    dassie_hub::consensus::Block {
        epoch: 0,
        view,
        height,
        parent: parent.digest(&hub()),
        parent_view: parent.view,
        timestamp_ms: START + view * 100,
        payload: dassie_hub::sim::payload(height, 9, Some(schedule)),
    }
}

/// An empty repeat of `parent` in view `view` of epoch 0.
fn repeat_of(view: u64, parent: &dassie_hub::consensus::Block) -> dassie_hub::consensus::Block {
    dassie_hub::consensus::Block {
        epoch: 0,
        view,
        height: parent.height,
        parent: parent.digest(&hub()),
        parent_view: parent.view,
        timestamp_ms: START + view * 100,
        payload: Vec::new(),
    }
}

/// Hands `engine` the notarization of `block`, and its finalization too if
/// `finalized`.
fn certify(
    engine: &mut Engine,
    app: &mut SimApp,
    block: &dassie_hub::consensus::Block,
    finalized: bool,
) {
    let kinds: &[VoteKind] = if finalized {
        &[VoteKind::Notarize, VoteKind::Finalize]
    } else {
        &[VoteKind::Notarize]
    };
    for kind in kinds {
        let certificate = certificate_of(*kind, block.view, block.digest(&hub()));
        let message = Message::decode_message(&certificate).unwrap();
        hand(engine, app, START + block.view * 100 + 5, 3, &message);
    }
}

#[test]
fn a_lagging_validator_finalizes_through_a_repeat_before_it_knows_the_change() {
    // It saw blocks 1 and 2 proposed and notarized, never final, and knows
    // the change only from the chain: it judges the repeat on that chain,
    // and finalizes blocks 1 and 2 when the repeat's finalization comes.
    let (mut engine, mut app, schedule) = scheduled_at_one();
    let b1 = scheduled_block(1, &genesis(), &schedule);
    let b2 = scheduled_block(2, &b1, &schedule);
    for (block, leader) in [(&b1, 1), (&b2, 2)] {
        hand(
            &mut engine,
            &mut app,
            START + block.view * 100 + 1,
            leader,
            &proposed(block, usize::try_from(leader).unwrap()),
        );
        certify(&mut engine, &mut app, block, false);
    }
    assert_eq!(
        (engine.tip().block.height, engine.set_for_height(3)),
        (0, &set(0..4))
    );
    let repeat = repeat_of(3, &b2);
    let _ = notarized(&mut engine);
    hand(&mut engine, &mut app, START + 301, 3, &proposed(&repeat, 3));
    assert_eq!(notarized(&mut engine), vec![repeat.digest(&hub())]);
    certify(&mut engine, &mut app, &repeat, true);
    assert_eq!(engine.tip().block, b2);
    assert_eq!(engine.tip().repeats, vec![repeat]);
    assert_eq!((engine.epoch(), engine.set()), (1, &schedule.set));
    // One that never saw them proposed gets the blocks with their
    // certificates only, and finalizes them the same.
    let (mut engine, mut app, schedule) = scheduled_at_one();
    let certified =
        |block: &dassie_hub::consensus::Block, kind| dassie_hub::consensus::wire::Certified {
            certificate: match Message::decode_message(&certificate_of(
                kind,
                block.view,
                block.digest(&hub()),
            ))
            .unwrap()
            {
                Message::Certificate(certificate) => certificate,
                _ => panic!("a certificate"),
            },
            block: Some(block.clone()),
        };
    let delivery = Message::Delivery(dassie_hub::consensus::wire::Delivery {
        finalized: Vec::new(),
        finalization: None,
        certified: vec![
            certified(&b1, VoteKind::Notarize),
            certified(&b2, VoteKind::Notarize),
            certified(&repeat_of(3, &b2), VoteKind::Notarize),
            certified(&repeat_of(3, &b2), VoteKind::Finalize),
        ],
    });
    hand(&mut engine, &mut app, START + 400, 1, &delivery);
    assert_eq!((engine.tip().block.height, engine.epoch()), (2, 1));
    assert_eq!(engine.set(), &schedule.set);
}

#[test]
fn no_block_of_the_old_set_passes_the_boundary_before_the_change_is_final() {
    // Blocks 1 (which schedules the change for height 3) and 2 are
    // notarized, and their votes split: neither is final. View 3's leader
    // proposes block 3 in epoch 0, which the chain says the next set must
    // build: we nullify it, and lead view 4 with a repeat of block 2.
    let (mut engine, mut app, schedule) = scheduled_at_one();
    let b1 = scheduled_block(1, &genesis(), &schedule);
    let b2 = scheduled_block(2, &b1, &schedule);
    for (block, leader) in [(&b1, 1), (&b2, 2)] {
        hand(
            &mut engine,
            &mut app,
            START + block.view * 100 + 1,
            leader,
            &proposed(block, usize::try_from(leader).unwrap()),
        );
        certify(&mut engine, &mut app, block, false);
    }
    let _ = std::iter::from_fn(|| engine.poll_output()).count();
    let b3 = scheduled_block(3, &b2, &schedule);
    hand(&mut engine, &mut app, START + 301, 3, &proposed(&b3, 3));
    let votes: Vec<VoteKind> = std::iter::from_fn(|| engine.poll_output())
        .filter_map(|output| match Message::decode_message(&output.message) {
            Ok(Message::Vote(vote)) if vote.view == 3 => Some(vote.kind),
            _ => None,
        })
        .collect();
    assert_eq!(votes, vec![VoteKind::Nullify]);
    let nullification = certificate_of(VoteKind::Nullify, 3, Digest::default());
    hand(
        &mut engine,
        &mut app,
        START + 310,
        3,
        &Message::decode_message(&nullification).unwrap(),
    );
    assert_eq!(engine.view(), 4);
    engine.handle_timeout(START + 500, &mut app);
    engine.check_invariants();
    let ours: Vec<_> = std::iter::from_fn(|| engine.poll_output())
        .filter_map(|output| match Message::decode_message(&output.message) {
            Ok(Message::Proposal(proposal)) => Some(proposal.block),
            _ => None,
        })
        .collect();
    assert_eq!(ours.len(), 1);
    assert!(ours[0].repeats(&b2) && ours[0].parent == b2.digest(&hub()));
}

#[test]
fn catch_up_crosses_a_handover_that_ended_with_repeats() {
    let (_, _, schedule) = scheduled_at_one();
    let b1 = scheduled_block(1, &genesis(), &schedule);
    let b2 = scheduled_block(2, &b1, &schedule);
    let repeat = repeat_of(3, &b2);
    let finalization = match Message::decode_message(&certificate_of(
        VoteKind::Finalize,
        3,
        repeat.digest(&hub()),
    ))
    .unwrap()
    {
        Message::Certificate(certificate) => certificate,
        _ => panic!("a certificate"),
    };
    // Behind the scheduling block: blocks 1 and 2 and the repeat, in one
    // delivery; it learns of the change from block 1 as it applies it.
    let (mut engine, mut app, _) = scheduled_at_one();
    let delivery = |finalized: Vec<_>| {
        Message::Delivery(dassie_hub::consensus::wire::Delivery {
            finalized,
            finalization: Some(finalization.clone()),
            certified: Vec::new(),
        })
    };
    hand(
        &mut engine,
        &mut app,
        START + 400,
        1,
        &delivery(vec![b1.clone(), b2.clone(), repeat.clone()]),
    );
    assert_eq!((engine.tip().block.height, engine.epoch()), (2, 1));
    assert_eq!(engine.tip().repeats, vec![repeat.clone()]);
    // Beyond what peers keep: block 2 and its repeat, checked with the set
    // it knows, then the state after block 2, which names the change.
    let (mut engine, mut app, schedule) = scheduled_at_one();
    hand(
        &mut engine,
        &mut app,
        START + 400,
        1,
        &delivery(vec![b2.clone(), repeat.clone()]),
    );
    let behind = std::iter::from_fn(|| engine.poll_event())
        .find_map(|event| match event {
            dassie_hub::consensus::Event::Behind(finalized) => Some(*finalized),
            _ => None,
        })
        .expect("behind");
    assert_eq!(behind.repeats, vec![repeat]);
    let change = schedule.pending(behind.block.height);
    engine.jump(START + 401, behind, change).unwrap();
    engine.check_invariants();
    assert_eq!((engine.tip().block.height, engine.epoch()), (2, 1));
    assert_eq!(engine.set(), &schedule.set);
}

#[test]
fn a_long_stall_keeps_the_chain_and_lets_go_of_the_rest() {
    // For more views than the engine holds rounds, every other view
    // notarizes a block on the last and the others are nullified; nothing
    // is final. The engine keeps only the views from the last notarized one
    // and the chain to it (the invariants check it after every step, what
    // the application verified among it), and once the last block is
    // final, so is the whole chain.
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let mut app = SimApp::default();
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    let views = dassie_hub::consensus::MAX_ROUNDS as u64 + 50;
    let mut parent = genesis();
    for view in 1..views {
        let at = START + view * 100;
        if view % 2 == 0 {
            let nullification = certificate_of(VoteKind::Nullify, view, Digest::default());
            hand(
                &mut engine,
                &mut app,
                at,
                3,
                &Message::decode_message(&nullification).unwrap(),
            );
            continue;
        }
        let mut block = block_of(&proposal_on(view, &parent, at));
        block.parent_view = parent.view;
        let leader = usize::try_from(view % 4).unwrap();
        hand(&mut engine, &mut app, at + 1, 2, &proposed(&block, leader));
        certify(&mut engine, &mut app, &block, false);
        parent = block;
    }
    assert!(engine.view() >= views);
    assert_eq!(engine.tip().block.height, 0);
    certify(&mut engine, &mut app, &parent, true);
    assert_eq!(engine.tip().block, parent);
    assert_eq!(app.finalized.len() as u64, parent.height);
}

/// An application that breaks its contract: its chain schedules a change,
/// which `scheduled_change` names, but `reconfiguration` never reports it.
struct Misreporting(SimApp);

impl dassie_hub::consensus::Application for Misreporting {
    fn propose(
        &mut self,
        context: &dassie_hub::consensus::ProposeContext<'_>,
        payload: &mut Vec<u8>,
    ) -> Result<(), dassie_hub::consensus::Declined> {
        self.0.propose(context, payload)
    }

    fn verify(&mut self, context: &dassie_hub::consensus::VerifyContext, payload: &[u8]) -> bool {
        self.0.verify(context, payload)
    }

    fn finalized(&mut self, block: &dassie_hub::consensus::Block, digest: &Digest) {
        self.0.finalized(block, digest);
        let _ = self.0.reconfiguration();
    }

    fn scheduled_change(&mut self, after: &Digest) -> Option<(u64, dassie_hub::ValidatorSet)> {
        self.0.scheduled_change(after)
    }
}

#[test]
fn a_broken_application_or_quorum_stalls_the_engine_without_a_panic() {
    // Blocks 1 and 2 are final, but the application never reported the
    // change block 1 scheduled, so the epoch goes on past its last block;
    // the chain says block 2 ends it, and view 3 repeats it. The repeat's
    // finalization finalizes nothing new: the engine stalls, as it must
    // with an application that breaks its word, and does not panic.
    let (mut engine, app, schedule) = scheduled_at_one();
    let mut app = Misreporting(app);
    let b1 = scheduled_block(1, &genesis(), &schedule);
    let b2 = scheduled_block(2, &b1, &schedule);
    for (block, leader) in [(&b1, 1u64), (&b2, 2)] {
        let message = proposed(block, usize::try_from(leader).unwrap());
        engine.handle_message(
            START + block.view * 100 + 1,
            &mut app,
            PeerId(leader),
            &message.to_vec(),
        );
        for kind in [VoteKind::Notarize, VoteKind::Finalize] {
            let certificate = certificate_of(kind, block.view, block.digest(&hub()));
            engine.handle_message(
                START + block.view * 100 + 5,
                &mut app,
                PeerId(3),
                &certificate,
            );
            engine.check_invariants();
        }
    }
    assert_eq!((engine.tip().block.height, engine.epoch()), (2, 0));
    let repeat = repeat_of(3, &b2);
    engine.handle_message(
        START + 301,
        &mut app,
        PeerId(3),
        &proposed(&repeat, 3).to_vec(),
    );
    for kind in [VoteKind::Notarize, VoteKind::Finalize] {
        let certificate = certificate_of(kind, 3, repeat.digest(&hub()));
        engine.handle_message(START + 305, &mut app, PeerId(3), &certificate);
        engine.check_invariants();
    }
    engine.handle_timeout(START + 2_000, &mut app);
    engine.check_invariants();
    assert_eq!((engine.tip().block.height, engine.epoch()), (2, 0));
    // A quorum that breaks the rules too finalizes a block on block 2 that
    // skips a height: there is nothing it can finalize, and the engine
    // stalls on it as well.
    let mut skipping = scheduled_block(5, &b2, &schedule);
    skipping.height = 4;
    for kind in [VoteKind::Notarize, VoteKind::Finalize] {
        let certificate = certificate_of(kind, 5, skipping.digest(&hub()));
        let mut message = Message::decode_message(&certificate).unwrap();
        if let Message::Certificate(certificate) = message {
            message = Message::Delivery(dassie_hub::consensus::wire::Delivery {
                finalized: Vec::new(),
                finalization: None,
                certified: vec![dassie_hub::consensus::wire::Certified {
                    certificate,
                    block: Some(skipping.clone()),
                }],
            });
        }
        engine.handle_message(START + 2_100, &mut app, PeerId(3), &message.to_vec());
        engine.check_invariants();
    }
    engine.handle_timeout(START + 3_000, &mut app);
    engine.check_invariants();
    assert_eq!(engine.tip().block.height, 2);
}

#[test]
fn a_block_whose_state_was_let_go_of_is_verified_again() {
    // Block 1 is notarized, its finalize votes split. Views 2 to 4 each
    // bring a block on it that we verify and that is then nullified; the
    // application keeps two states, so it lets go of block 1's. View 5's
    // block on block 1 is still verified, on block 1 verified again, and
    // its finalization finalizes block 1 too.
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let mut app = SimApp::default();
    app.keep_states = Some(2);
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    let b1 = block_of(&proposal_on(1, &genesis(), START));
    hand(&mut engine, &mut app, START + 1, 1, &proposed(&b1, 1));
    certify(&mut engine, &mut app, &b1, false);
    for view in 2..5 {
        let mut other = block_of(&proposal_on(view, &b1, START + view * 100));
        other.parent_view = 1;
        let leader = usize::try_from(view % 4).unwrap();
        hand(
            &mut engine,
            &mut app,
            START + view * 100 + 1,
            2,
            &proposed(&other, leader),
        );
        let nullification = certificate_of(VoteKind::Nullify, view, Digest::default());
        let message = Message::decode_message(&nullification).unwrap();
        hand(&mut engine, &mut app, START + view * 100 + 5, 3, &message);
    }
    let _ = notarized(&mut engine);
    let mut b5 = block_of(&proposal_on(5, &b1, START + 500));
    b5.parent_view = 1;
    hand(&mut engine, &mut app, START + 501, 1, &proposed(&b5, 1));
    assert_eq!(notarized(&mut engine), vec![b5.digest(&hub())]);
    certify(&mut engine, &mut app, &b5, true);
    assert_eq!(engine.tip().block, b5);
    assert_eq!(
        app.finalized.keys().copied().collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn applications_that_keep_few_states_finalize_after_a_long_run_unfinalized() {
    let mut network = honest(7);
    for sim in &mut network.validators {
        sim.app.keep_states = Some(2);
    }
    run_for(&mut network, 1_000);
    let before = network.honest_floor();
    // Every finalize vote is lost, on a rough network that parts now and
    // then: blocks are notarized, views nullified, none is final.
    network.lose_every_finalize = true;
    network.faults = Faults::ROUGH;
    for round in 0..6 {
        network.partition(&[round % 7, (round + 3) % 7]);
        run_for(&mut network, 400);
        network.heal();
        run_for(&mut network, 400);
    }
    assert!(network.honest_floor() <= before + 1);
    network.lose_every_finalize = false;
    network.faults = Faults::CALM;
    run_for(&mut network, 5_000);
    assert_safe(&network);
    assert!(
        network.honest_floor() > before + 20,
        "{}",
        network.honest_floor()
    );
}

#[test]
fn finality_returns_after_finalize_was_lost_longer_than_the_engine_holds() {
    // Every finalize vote is lost for far longer than the views and blocks
    // an engine holds: blocks keep being notarized, none is final. Once
    // the votes go through again, blocks are final again at once.
    for count in [4, 7] {
        let mut network = honest(count);
        run_for(&mut network, 1_000);
        let before = network.honest_floor();
        network.lose_every_finalize = true;
        run_for(&mut network, 60_000);
        assert!(network.honest_floor() <= before + 1);
        network.lose_every_finalize = false;
        run_for(&mut network, 3_000);
        assert_safe(&network);
        assert!(
            network.honest_floor() > before + 10,
            "{count} validators: {} after the outage, {before} before",
            network.honest_floor()
        );
    }
}

#[test]
fn a_validator_down_through_a_long_outage_catches_up_by_blocks() {
    // While one validator is down, every finalize vote is lost long enough
    // for the chain to reach the most heights that wait to be final; then
    // a repeat finalizes it all at once, and blocks build on it through the
    // repeat's view. The validator that comes back takes all of it in
    // deliveries, without loading a state.
    for count in [4, 7] {
        let mut network = honest(count);
        run_for(&mut network, 1_000);
        let down = count - 1;
        network.crash(down);
        network.lose_every_finalize = true;
        run_for(&mut network, 20_000);
        network.lose_every_finalize = false;
        run_for(&mut network, 2_000);
        let top = network.honest_floor();
        network.start(down);
        run_for(&mut network, 5_000);
        assert_safe(&network);
        assert!(
            network.height(down) >= top,
            "{count} validators: {} after coming back, {top} at the others",
            network.height(down)
        );
        assert_eq!(network.state_loads, 0);
    }
}

#[test]
fn a_validator_away_while_nothing_is_final_takes_the_chain_from_others() {
    // While every finalize vote is lost, one validator is cut off as the
    // chain grows, and comes back while it is still not final: it takes
    // what it lacks from the others, who let go of the views below the
    // highest notarized one. Then another crashes, so that its vote is
    // needed for every quorum, and blocks are final again once the votes
    // go through.
    let mut network = honest(4);
    run_for(&mut network, 1_000);
    let before = network.honest_floor();
    network.lose_every_finalize = true;
    network.partition(&[3]);
    run_for(&mut network, 6_000);
    network.heal();
    run_for(&mut network, 3_000);
    network.crash(0);
    network.lose_every_finalize = false;
    run_for(&mut network, 5_000);
    assert_safe(&network);
    for index in 1..4 {
        assert!(
            network.height(index) > before + 31,
            "validator {index}: {} after, {before} before",
            network.height(index)
        );
    }
}

#[test]
fn a_repeat_is_valid_only_of_an_epochs_last_block() {
    let (mut engine, mut app, b2, _) = split_at_the_boundary();
    // With a payload, or below the epoch's last height, it is no repeat:
    // view 3 is nullified, not notarized.
    let mut repeat = dassie_hub::consensus::Block {
        epoch: 0,
        view: 3,
        height: 2,
        parent: b2.digest(&hub()),
        parent_view: 2,
        timestamp_ms: START + 200,
        payload: vec![1],
    };
    hand(&mut engine, &mut app, START + 201, 3, &proposed(&repeat, 3));
    assert!(notarized(&mut engine).is_empty());
    // A repeat in epoch 0 of a block that is not its last.
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    let b1 = block_of(&proposal_on(1, &genesis(), START));
    hand(&mut engine, &mut app, START + 1, 1, &proposed(&b1, 1));
    let notarization = certificate_of(VoteKind::Notarize, 1, b1.digest(&hub()));
    let message = Message::decode_message(&notarization).unwrap();
    hand(&mut engine, &mut app, START + 3, 3, &message);
    let _ = notarized(&mut engine);
    repeat.view = 2;
    repeat.height = 1;
    repeat.parent = b1.digest(&hub());
    repeat.parent_view = 1;
    repeat.payload = Vec::new();
    hand(&mut engine, &mut app, START + 201, 2, &proposed(&repeat, 2));
    assert!(notarized(&mut engine).is_empty());
}

/// The notarize votes of validator 0 among `engine`'s outputs.
fn notarized(engine: &mut Engine) -> Vec<Digest> {
    std::iter::from_fn(|| engine.poll_output())
        .filter_map(|output| match Message::decode_message(&output.message) {
            Ok(Message::Vote(vote)) if vote.kind == VoteKind::Notarize => Some(vote.block),
            _ => None,
        })
        .collect()
}

#[test]
fn a_block_skips_a_notarized_view_only_once_it_is_nullified() {
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let mut app = SimApp::default();
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    let first = proposal_on(1, &genesis(), START);
    engine.handle_message(START + 2, &mut app, PeerId(1), &first);
    let Ok(Message::Proposal(Proposal { block: b1, .. })) = Message::decode_message(&first) else {
        panic!("a proposal");
    };
    assert_eq!(notarized(&mut engine), vec![b1.digest(&hub())]);
    // View 1 is notarized, never finalized; view 2's leader builds on
    // genesis, skipping it.
    let notarization = certificate_of(VoteKind::Notarize, 1, b1.digest(&hub()));
    engine.handle_message(START + 4, &mut app, PeerId(1), &notarization);
    assert_eq!(engine.view(), 2);
    let skipping = proposal_on(2, &genesis(), START + 110);
    engine.handle_message(START + 112, &mut app, PeerId(2), &skipping);
    engine.check_invariants();
    assert!(notarized(&mut engine).is_empty());
    // Once view 1 is nullified too, the block may skip it.
    let nullification = certificate_of(VoteKind::Nullify, 1, Digest::default());
    engine.handle_message(START + 114, &mut app, PeerId(3), &nullification);
    engine.check_invariants();
    assert_eq!(notarized(&mut engine).len(), 1);
    // It voted to finalize the first block at height 1 when view 1 was
    // notarized: it never votes to finalize another there, so that two
    // finalizations of one height name only faulty validators.
    let Ok(Message::Proposal(Proposal { block: b2, .. })) = Message::decode_message(&skipping)
    else {
        panic!("a proposal");
    };
    let notarization = certificate_of(VoteKind::Notarize, 2, b2.digest(&hub()));
    engine.handle_message(START + 116, &mut app, PeerId(3), &notarization);
    engine.check_invariants();
    let finalized: Vec<Vote> = std::iter::from_fn(|| engine.poll_output())
        .filter_map(|output| match Message::decode_message(&output.message) {
            Ok(Message::Vote(vote)) if vote.kind == VoteKind::Finalize => Some(vote),
            _ => None,
        })
        .collect();
    assert!(finalized.is_empty(), "{finalized:?}");
    assert_eq!(engine.view(), 3);
}

#[test]
fn a_block_is_notarized_only_if_its_stamp_is_timely() {
    let precision = CONFIG.precision_ms;
    let delay = CONFIG.max_delay_ms;
    // Stamped ahead of our clock by more than the precision, or behind it
    // by more than the precision and a message's delay.
    for (stamp, timely) in [
        (START + precision + 1, false),
        (START + precision, true),
        (START - precision - delay, true),
        (START - precision - delay - 1, false),
    ] {
        let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
        let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
        let proposal = proposal_on(1, &genesis(), stamp);
        engine.handle_message(START, &mut SimApp::default(), PeerId(1), &proposal);
        engine.check_invariants();
        let votes: Vec<VoteKind> = std::iter::from_fn(|| engine.poll_output())
            .filter_map(|output| match Message::decode_message(&output.message) {
                Ok(Message::Vote(vote)) => Some(vote.kind),
                _ => None,
            })
            .collect();
        let expected = if timely {
            VoteKind::Notarize
        } else {
            VoteKind::Nullify
        };
        assert_eq!(votes, vec![expected], "stamped {stamp}");
    }
}

#[test]
fn a_restart_never_signs_a_second_notarize_in_a_view() {
    let validators = set(0..4);
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), validators);
    let mut app = SimApp::default();
    let now = START;
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored.clone(), now);
    let mut stored = stored;
    // View 1 is led by validator 1.
    let proposal = |payload: u8| {
        let block = dassie_hub::consensus::Block {
            epoch: 0,
            view: 1,
            height: 1,
            parent: genesis().digest(&hub()),
            parent_view: 0,
            timestamp_ms: now,
            payload: dassie_hub::sim::payload(1, usize::from(payload), None),
        };
        let digest = block.digest(&hub());
        let vote = Vote::sign(
            &hub(),
            &key(1),
            index(1),
            VoteKind::Notarize,
            (0, 1),
            digest,
        );
        Message::Proposal(Proposal {
            block,
            signature: vote.signature,
        })
        .to_vec()
    };
    engine.handle_message(now + 2, &mut app, PeerId(1), &proposal(1));
    engine.check_invariants();
    stored.apply(engine.take_batch());
    let notarized = std::iter::from_fn(|| engine.poll_output())
        .filter(|output| output.to == Recipient::All)
        .filter_map(|output| match Message::decode_message(&output.message) {
            Ok(Message::Vote(vote)) if vote.kind == VoteKind::Notarize => Some(vote),
            _ => None,
        })
        .count();
    assert_eq!(notarized, 1);
    assert_eq!(stored.votes.len(), 1);
    // A crash, and the leader's second block for the same view.
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored.clone(), now + 5);
    engine.handle_message(now + 6, &mut app, PeerId(1), &proposal(2));
    engine.check_invariants();
    stored.apply(engine.take_batch());
    let second = std::iter::from_fn(|| engine.poll_output()).any(|output| {
        matches!(
            Message::decode_message(&output.message),
            Ok(Message::Vote(vote)) if vote.kind == VoteKind::Notarize && vote.block != stored.votes[0].block
        )
    });
    assert!(!second);
}

#[test]
fn a_stalled_validator_never_votes_again() {
    let mut network = honest(4);
    run_for(&mut network, 1_000);
    network.act(0, |engine, _, now| engine.declare_stalled(now));
    assert!(network.validators[0].stored.stalled);
    let votes = network.validators[0].stored.votes.len();
    run_for(&mut network, 2_000);
    network.crash(0);
    network.start(0);
    run_for(&mut network, 2_000);
    assert!(network.engine(0).is_stalled());
    // Its own votes stopped; the others go on, three being a quorum, and
    // it still learns of what they finalize.
    assert!(network.validators[0].stored.votes.len() <= votes);
    assert!(network.height(0) > 30);
    assert_safe(&network);
    let now = network.local_now(1);
    assert!(network.engine(1).since_finalized_ms(now) < 1_000);
}

#[test]
fn a_new_validator_set_takes_over_at_a_height() {
    // Validators 0 to 3 found the hub; 4 follows it, and replaces 0.
    let mut network = Network::with_genesis(6, &roles(5, &[]), CONFIG, 4);
    run_for(&mut network, 2_000);
    let from_height = network.height(1) + 20;
    let next = set(1..5);
    for index in 0..5 {
        network.act(index, |engine, _, now| {
            engine.reconfigure(now, from_height, next.clone()).unwrap();
        });
    }
    run_for(&mut network, 5_000);
    assert_safe(&network);
    for index in 0..5 {
        assert!(
            network.height(index) > from_height + 10,
            "{index}: {}",
            network.height(index)
        );
        assert_eq!(network.engine(index).set_for_height(from_height), &next);
    }
    // Blocks from the boundary are certified by the new set alone.
    let tip = network.engine(4).tip().clone();
    let finalization = tip.finalization.unwrap();
    assert_eq!(verify_certificate(&hub(), &next, &finalization), Ok(()));
    assert!(verify_certificate(&hub(), &set(0..4), &finalization).is_err());
    // The validator that left signs nothing in the new set's views.
    let first_view = network.engine(1).tip().block.view;
    assert!(
        network.validators[0]
            .stored
            .votes
            .iter()
            .all(|vote| vote.view < first_view)
    );
}

#[test]
fn a_change_the_chain_schedules_is_taken_even_by_one_far_behind() {
    // The block at height 20 schedules validator 4 to replace 0, from a
    // height further than a delivery of blocks, and from one closer;
    // validator 3 is cut off from before 20 until past both: within what
    // peers keep, it catches up block by block; beyond it, epoch by epoch,
    // loading the state after the last block of its epoch, then after the
    // tip. An attacker meanwhile sends it a history of its own.
    for (from, cut_ms) in [(40, 5_000), (25, 5_000), (40, 15_000)] {
        let mut network = Network::with_genesis(8, &roles(5, &[]), CONFIG, 4);
        let next = dassie_hub::ValidatorSet::new(
            [4, 1, 2, 3]
                .iter()
                .map(|at| key(*at).public_key())
                .collect(),
        )
        .unwrap();
        for sim in &mut network.validators {
            sim.app.schedule = Some(dassie_hub::sim::Schedule {
                at: 20,
                from,
                set: next.clone(),
            });
        }
        run_for(&mut network, 1_000);
        assert!(network.height(3) < 20);
        network.partition(&[3]);
        run_for(&mut network, cut_ms);
        assert!(network.height(1) > 45);
        network.heal();
        for at in 0..10 {
            let (finalized, _) = forged(network.height(1) + 10 + at, 1_000 + at);
            let delivery = Message::Delivery(dassie_hub::consensus::wire::Delivery {
                finalized: vec![finalized.block],
                finalization: finalized.finalization,
                certified: Vec::new(),
            });
            inject(&mut network, 3, &delivery);
            run_for(&mut network, 300);
        }
        assert_safe(&network);
        let top = network.height(1);
        for index in 0..5 {
            let height = network.height(index);
            assert!(
                height + 5 >= top,
                "from {from}, validator {index}: {height}"
            );
            assert_eq!(network.engine(index).set_for_height(from), &next);
        }
        assert_eq!(network.state_loads > 0, cut_ms > 10_000, "from {from}");
    }
}

/// An attacker's keys, which no hub named.
fn attacker(at: usize) -> NodeKey {
    NodeKey::from_bytes(&[u8::try_from(100 + at).unwrap(); 32])
}

/// A block far ahead, finalized by the attacker's own keys, and the set
/// of them that a state the attacker made up would name.
fn forged(height: u64, view: u64) -> (dassie_hub::consensus::Finalized, dassie_hub::ValidatorSet) {
    let keys = dassie_hub::ValidatorSet::new((0..4).map(|at| attacker(at).public_key()).collect())
        .unwrap();
    let change = (2, keys.clone());
    let mut payload = dassie_hub::sim::payload(height, 0, None);
    payload[10..42].copy_from_slice(dassie_hub::sim::state_root(Some(&change)).as_bytes());
    let block = dassie_hub::consensus::Block {
        epoch: 1,
        view,
        height,
        parent: Digest([7; 32]),
        parent_view: view - 1,
        timestamp_ms: START,
        payload,
    };
    let digest = block.digest(&hub());
    let votes: Vec<Vote> = (0..4)
        .map(|at| {
            Vote::sign(
                &hub(),
                &attacker(at),
                index(at),
                VoteKind::Finalize,
                (1, view),
                digest,
            )
        })
        .collect();
    let finalization = dassie_hub::consensus::Certificate {
        epoch: 1,
        kind: VoteKind::Finalize,
        view,
        block: digest,
        signers: votes
            .iter()
            .fold(dassie_hub::Signers::NONE, |signers, vote| {
                signers.with(vote.signer)
            }),
        signatures: votes.iter().map(|vote| vote.signature).collect(),
    };
    let finalized = dassie_hub::consensus::Finalized::new(block, Some(finalization));
    (finalized, keys)
}

#[test]
fn a_set_named_by_data_no_known_set_signed_is_never_taken_in() {
    let stored = dassie_hub::consensus::Stored::genesis(genesis(), set(0..4));
    let mut app = SimApp::default();
    let mut engine = Engine::new(CONFIG, hub(), key(0), stored, START);
    let _ = std::iter::from_fn(|| engine.poll_output()).count();
    let (finalized, keys) = forged(500, 600);
    // Delivered as the end of our epoch: refused, and no Behind.
    let delivery = Message::Delivery(dassie_hub::consensus::wire::Delivery {
        finalized: vec![finalized.block.clone()],
        finalization: finalized.finalization.clone(),
        certified: Vec::new(),
    });
    let refused = engine.stats().refused;
    engine.handle_message(START + 1, &mut app, PeerId(99), &delivery.to_vec());
    engine.check_invariants();
    assert_eq!(engine.stats().refused, refused + 1);
    let events: Vec<_> = std::iter::from_fn(|| engine.poll_event()).collect();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, dassie_hub::consensus::Event::Behind(_)))
    );
    // Loaded as if a host trusted it, with the change its state names:
    // the finalization is checked with the set we know, first.
    let jumped = engine.jump(START + 2, finalized.clone(), Some((2, keys.clone())));
    assert_eq!(jumped, Err(dassie_hub::consensus::Invalid::Unproven));
    assert_eq!(engine.tip().block.height, 0);
    assert_eq!(engine.set_for_height(2), &set(0..4));
    assert_eq!(engine.set_for_height(1_000), &set(0..4));
    // A certificate of the attacker's for a view ahead steers no request
    // to its sender: we ask a validator we know.
    let certificate = Message::Certificate(finalized.finalization.clone().unwrap());
    engine.handle_message(START + 200, &mut app, PeerId(99), &certificate.to_vec());
    engine.handle_timeout(START + 400, &mut app);
    let known: Vec<_> = set(0..4).keys().to_vec();
    let mut fetches = 0;
    for output in std::iter::from_fn(|| engine.poll_output()) {
        if let Ok(Message::Fetch(_)) = Message::decode_message(&output.message) {
            assert!(matches!(&output.to, Recipient::Validator(key) if known.contains(key)));
            fetches += 1;
        }
        assert_ne!(output.to, Recipient::Peer(PeerId(99)));
    }
    assert!(fetches > 0);
}

#[test]
fn members_check_certificates_alone() {
    let mut network = honest(4);
    run_for(&mut network, 1_000);
    let tip = network.engine(2).tip().clone();
    let finalization = tip.finalization.unwrap();
    assert_eq!(finalization.block, tip.block.digest(&hub()));
    assert_eq!(
        verify_certificate(&hub(), &set(0..4), &finalization),
        Ok(())
    );
    assert!(verify_certificate(&Digest([1; 32]), &set(0..4), &finalization).is_err());
    let mut fewer = finalization.clone();
    fewer.signatures.truncate(2);
    assert!(verify_certificate(&hub(), &set(0..4), &fewer).is_err());
}

#[test]
fn proposals_offer_the_votes_received() {
    let mut network = honest(4);
    run_for(&mut network, 2_000);
    for index in 0..4 {
        assert!(network.validators[index].app.votes_offered >= 8);
    }
}
