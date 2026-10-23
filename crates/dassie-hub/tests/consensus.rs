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

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_hub::consensus::wire::{Message, Proposal};
use dassie_hub::consensus::{
    Config, Engine, Evidence, PeerId, Recipient, Vote, VoteKind, verify_certificate,
};
use dassie_hub::sim::{Faults, Network, Role, START, SimApp, genesis, hub, key, roles, set};
use dassie_hub::{Digest, ValidatorIndex};
use dassie_identity::Signature;
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
                view,
                height,
                parent: Digest([u8::try_from(at % 251).unwrap(); 32]),
                parent_view: view - 1,
                timestamp_ms: START,
                payload: at.to_be_bytes().to_vec(),
            };
            let digest = block.digest(&hub());
            let vote = Vote::sign(&hub(), &key(1), index(1), VoteKind::Notarize, view, digest);
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
        view,
        Digest::default(),
    );
    let finalize = Vote::sign(
        &hub(),
        &key(3),
        signer,
        VoteKind::Finalize,
        view,
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
        view,
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
            view,
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
        view,
        Digest::default(),
    );
    inject(&mut network, 0, &Message::Vote(elsewhere));
    inject(
        &mut network,
        0,
        &Message::Fetch(dassie_hub::consensus::wire::Fetch { height: 0, view: 0 }),
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
        1,
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
    let mut payload = vec![1];
    payload.extend_from_slice(&height.to_be_bytes());
    payload.push(9);
    let block = dassie_hub::consensus::Block {
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
        view,
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
        .map(|at| Vote::sign(&hub(), &key(at), index(at), kind, view, block))
        .collect();
    Message::Certificate(dassie_hub::consensus::Certificate {
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
        let mut block = dassie_hub::consensus::Block {
            view: 1,
            height: 1,
            parent: genesis().digest(&hub()),
            parent_view: 0,
            timestamp_ms: now,
            payload: vec![1, 0, 0, 0, 0, 0, 0, 0, 1, payload],
        };
        block.payload.truncate(10);
        let digest = block.digest(&hub());
        let vote = Vote::sign(&hub(), &key(1), index(1), VoteKind::Notarize, 1, digest);
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
