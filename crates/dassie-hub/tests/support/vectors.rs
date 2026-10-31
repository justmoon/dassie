//! The hub's golden vectors (spec/areas/hubs.md#golden-vectors), as the
//! crate's code makes them from fixed keys, one file per version of the
//! hub's protocol it runs. `examples/hub_vectors.rs` writes them to
//! `testdata/hub/vectors-v<version>.json`; `tests/vectors.rs` checks that
//! the code still makes and reads exactly those bytes. They pin what every
//! validator of a hub must agree on within its version: the rules and the
//! ID, operations, promises, statements, blocks and the state a block
//! leaves, votes, the genesis, and the escrow's transactions; from version
//! 2 on, a payee's release, a payer's dispute, what a claim that waits
//! leaves, and a hub of version 1 moved to version 2.

use dassie_hub::client::{SignedStatement, Statement};
use dassie_hub::consensus::wire::Message;
use dassie_hub::consensus::{Block, Vote, VoteKind};
use dassie_hub::escrow::{OPEN_MEMO_TYPE, ValidatorKeys, escrow_master, setup, withdrawal_payment};
use dassie_hub::founding::{GenesisCertificate, genesis_digest, starting_terms};
use dassie_hub::ledger::{self, Ledger, results_root};
use dassie_hub::operation::{
    AccountNumber, Clawback, Deposit, Destination, Issuer, MemberOperation, Operation,
    SignedMemberOperation, SignedValidatorOperation, ValidatorOperation,
};
use dassie_hub::promise::{Claim, Commitment, Fulfilled, HoldNumber, Release, Signed};
use dassie_hub::rules::Rules;
use dassie_hub::service::genesis_block;
use dassie_hub::service::payload::Payload;
use dassie_hub::{Digest, Millis, ValidatorIndex};
use dassie_identity::{IdentityKey, NodeKey};
use dassie_oer::Encode;
use dassie_xrpl::codec::{Amount, Memo, Payment, Signing, Transact};
use dassie_xrpl::{AccountId, Wallet, hex_upper};
use serde_json::{Value, json};

/// Bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    hex_upper(bytes).to_lowercase()
}

/// The node key of the seed `byte` repeated, with its identity.
fn key(byte: u8) -> IdentityKey {
    dassie_identity::TableKeygen::identity_key(&[byte; 32])
}

/// The key of the account a deposit opens: the node key of the seed 0 to
/// 31.
pub fn member_key() -> NodeKey {
    NodeKey::from_bytes(&core::array::from_fn(|index| {
        u8::try_from(index).unwrap_or(u8::MAX)
    }))
}

/// Who pays the deposit, from the XRPL seed 0x11 repeated.
pub fn depositor() -> Wallet {
    Wallet::from_seed([0x11; 16])
}

/// The escrow it pays.
pub const ESCROW: [u8; 20] = [0xe5; 20];

/// The drops it pays.
pub const OPEN_DROPS: u64 = 20_000_000;

/// The hub's nonce.
pub const NONCE: [u8; 32] = [0x42; 32];

/// The XRP Ledger testnet's network ID.
const NETWORK: u32 = 1;

/// The genesis block's timestamp.
pub const GENESIS_MS: Millis = 1_800_000_000_000;

/// The sponsor's node key.
fn sponsor() -> IdentityKey {
    key(0x20)
}

/// The node keys of the four validators.
fn validator_nodes() -> Vec<IdentityKey> {
    (0x21..=0x24).map(key).collect()
}

/// The keys each validator makes for the hub.
pub fn validator_keys() -> Vec<ValidatorKeys> {
    validator_nodes()
        .iter()
        .map(|node| ValidatorKeys::derive(node, NETWORK, &NONCE).unwrap())
        .collect()
}

/// The rules of the hub the vectors of `version` are of: the starting
/// terms of the sponsor of key 0x20, of that version, its escrow at the
/// master key it derives, and four validators, of keys 0x21 to 0x24, each
/// accepting at its own host.
pub fn rules(version: u16) -> Rules {
    let sponsor = sponsor();
    let escrow = escrow_master(&sponsor, NETWORK, &NONCE);
    let mut terms = starting_terms(NONCE, sponsor.identity(), NETWORK, escrow.account().0);
    terms.version = version;
    let validators = validator_nodes()
        .iter()
        .zip(validator_keys())
        .enumerate()
        .map(|(index, (node, keys))| {
            keys.accept(node, &terms, format!("v{}.example:443", index + 1))
                .unwrap()
        })
        .collect();
    Rules { terms, validators }
}

/// The Payment that opens an account in a hub: to the escrow, tag 0, and
/// one memo of [`OPEN_MEMO_TYPE`] whose data is the new account's key.
pub fn open_payment() -> Payment {
    let depositor = depositor();
    Payment {
        account: depositor.account(),
        destination: AccountId(ESCROW),
        destination_tag: Some(0),
        amount: Amount::Xrp(OPEN_DROPS),
        send_max: None,
        fee: 12,
        sequence: 7,
        last_ledger_sequence: Some(1_000),
        flags: 0,
        network_id: None,
        memos: vec![Memo {
            memo_type: OPEN_MEMO_TYPE.to_vec(),
            memo_data: member_key().public_key().as_bytes().to_vec(),
        }],
        signing: Signing::by(depositor.public_key()),
    }
}

/// The opening deposit: what a wallet outside Dassie writes, byte for
/// byte, and what the validators attest of it.
pub fn open() -> Value {
    let signed = open_payment().sign(&depositor());
    json!({
        "memoType": hex(OPEN_MEMO_TYPE),
        "memoTypeText": String::from_utf8_lossy(OPEN_MEMO_TYPE),
        "key": hex(member_key().public_key().as_bytes()),
        "account": depositor().account().to_address(),
        "escrow": AccountId(ESCROW).to_address(),
        "destinationTag": 0,
        "drops": OPEN_DROPS.to_string(),
        "txBlob": hex(&signed.blob),
        "hash": hex(&signed.hash),
    })
}

/// The rules, the terms' digest each acceptance signs, and the hub ID.
fn rules_vector(rules: &Rules) -> Value {
    json!({
        "version": rules.terms.version,
        "terms": hex(&rules.terms.to_vec()),
        "termsDigest": hex(rules.terms.digest().as_bytes()),
        "rules": hex(&rules.to_vec()),
        "hubId": hex(rules.id().0.as_bytes()),
    })
}

/// The genesis block, the digest each validator signs, and the certificate
/// of the first three's signatures.
fn genesis_vector(rules: &Rules) -> Value {
    let block = genesis_block(rules, GENESIS_MS);
    let digest = genesis_digest(rules, GENESIS_MS);
    let signatures = validator_keys()
        .iter()
        .take(3)
        .enumerate()
        .map(|(index, keys)| {
            (
                ValidatorIndex::new(index).unwrap(),
                keys.consensus.sign(digest.as_bytes()),
            )
        })
        .collect();
    let certificate = GenesisCertificate {
        rules: rules.clone(),
        genesis_ms: GENESIS_MS,
        signatures,
    };
    assert!(certificate.verifies());
    json!({
        "genesisMs": GENESIS_MS,
        "block": hex(&block.to_vec()),
        "blockDigest": hex(block.digest(&rules.id().0).as_bytes()),
        "digest": hex(digest.as_bytes()),
        "certificate": hex(&certificate.to_vec()),
    })
}

/// Each validator's attestation that the escrow is set up, which the first
/// block holds.
fn attestations(rules: &Rules) -> Vec<Operation> {
    let hub = rules.id();
    validator_keys()
        .iter()
        .enumerate()
        .map(|(index, keys)| {
            Operation::Validator(SignedValidatorOperation::sign(
                &hub,
                ValidatorIndex::new(index).unwrap(),
                1,
                ValidatorOperation::AttestEscrow { sequence: 5 },
                &keys.consensus,
            ))
        })
        .collect()
}

/// A member's operation and a validator's, as signed and encoded, with
/// what their signatures cover and the hash a block's statement names.
fn operations_vector(rules: &Rules) -> Value {
    let hub = rules.id();
    let transfer = SignedMemberOperation::sign(
        &hub,
        AccountNumber(5),
        3,
        10,
        MemberOperation::Transfer {
            to: AccountNumber(6),
            amount: 1_000,
        },
        &member_key(),
    );
    let deposit = SignedValidatorOperation::sign(
        &hub,
        ValidatorIndex::new(1).unwrap(),
        2,
        ValidatorOperation::AttestDeposit(Deposit {
            transaction: Digest([0xd0; 32]),
            ledger: 1_000,
            amount: OPEN_DROPS,
            sender: depositor().account().0,
            tag: 0,
            key: Some(member_key().public_key()),
        }),
        &validator_keys()[1].consensus,
    );
    let member = Operation::Member(transfer.clone());
    let validator = Operation::Validator(deposit.clone());
    let signed = |sequence, operation| {
        Operation::Validator(SignedValidatorOperation::sign(
            &hub,
            ValidatorIndex::new(2).unwrap(),
            sequence,
            operation,
            &validator_keys()[2].consensus,
        ))
    };
    let issuer = signed(3, ValidatorOperation::AttestIssuer(FREEZE));
    let clawback = signed(4, ValidatorOperation::AttestClawback(CLAWBACK));
    json!({
        "member": {
            "operation": hex(&member.to_vec()),
            "digest": hex(transfer.digest(&hub).as_bytes()),
            "hash": hex(member.hash().as_bytes()),
        },
        "validator": {
            "operation": hex(&validator.to_vec()),
            "digest": hex(deposit.digest(&hub).as_bytes()),
            "hash": hex(validator.hash().as_bytes()),
        },
        "issuer": {
            "operation": hex(&issuer.to_vec()),
            "hash": hex(issuer.hash().as_bytes()),
        },
        "clawback": {
            "operation": hex(&clawback.to_vec()),
            "hash": hex(clawback.hash().as_bytes()),
        },
    })
}

/// The issuer's freeze of the escrow's line, as validators attest it.
const FREEZE: Issuer = Issuer {
    ledger: 2_000,
    frozen: true,
    fee: false,
};

/// A clawback from the escrow, as validators attest it.
const CLAWBACK: Clawback = Clawback {
    transaction: Digest([0xcb; 32]),
    ledger: 2_001,
    amount: 1_000_000,
};

/// The rules of a DSUSD hub of the same sponsor and validators, of
/// `version`: a token hub, whose issuer may freeze and claw back.
pub fn token_rules(version: u16) -> Rules {
    let sponsor = sponsor();
    let escrow = escrow_master(&sponsor, NETWORK, &NONCE);
    let mut terms = starting_terms(NONCE, sponsor.identity(), NETWORK, escrow.account().0);
    terms.version = version;
    terms.asset = dassie_hub::rules::Asset::Token {
        currency: dassie_xrpl::dsusd::CURRENCY,
        issuer: dassie_xrpl::dsusd::ISSUER,
    };
    terms.fees.gas = dassie_hub::rules::LEAST_TOKEN_GAS_DROPS;
    terms.fees.ledger = 0;
    terms.fees.least_payout = 1;
    let validators = validator_nodes()
        .iter()
        .zip(validator_keys())
        .enumerate()
        .map(|(index, (node, keys))| {
            keys.accept(node, &terms, format!("v{}.example:443", index + 1))
                .unwrap()
        })
        .collect();
    Rules { terms, validators }
}

/// The states a token hub's ledger leaves after its escrow is checked,
/// after a quorum attested a freeze, and after one attested a clawback
/// during it: they pin how the frozen state and what was clawed back are
/// in the state root.
fn token_vector(version: u16) -> Value {
    let rules = token_rules(version);
    let hub = rules.id();
    let mut state = Ledger::genesis(rules.clone(), GENESIS_MS).unwrap();
    let mut roots = Vec::new();
    for (height, operation) in [
        ValidatorOperation::AttestEscrow { sequence: 5 },
        ValidatorOperation::AttestIssuer(FREEZE),
        ValidatorOperation::AttestClawback(CLAWBACK),
    ]
    .into_iter()
    .enumerate()
    {
        let height = u64::try_from(height).unwrap() + 1;
        let operations: Vec<Operation> = validator_keys()
            .iter()
            .take(3)
            .enumerate()
            .map(|(index, keys)| {
                Operation::Validator(SignedValidatorOperation::sign(
                    &hub,
                    ValidatorIndex::new(index).unwrap(),
                    height,
                    operation.clone(),
                    &keys.consensus,
                ))
            })
            .collect();
        state
            .apply(&ledger::Block {
                height,
                timestamp: GENESIS_MS + 100 * height,
                operations: &operations,
                votes: &[],
            })
            .unwrap();
        roots.push(hex(state.state_root().as_bytes()));
    }
    assert!(state.frozen().is_some(), "frozen");
    assert_eq!(state.escrow().clawed_back, CLAWBACK.amount);
    json!({
        "rules": hex(&rules.to_vec()),
        "hubId": hex(hub.0.as_bytes()),
        "checked": roots[0],
        "frozen": roots[1],
        "clawedBack": roots[2],
    })
}

/// A payer's commitment and claim, signed with its account key.
fn promises_vector(rules: &Rules) -> Value {
    let hub = rules.id();
    let commitment = Signed::sign(
        Commitment {
            hold: HoldNumber(9),
            sequence: 4,
            amount: 250,
            condition: [0xc0; 32],
            expiry: GENESIS_MS + 60_000,
        },
        &hub,
        &member_key(),
    );
    let claim = Signed::sign(
        Claim {
            hold: HoldNumber(9),
            total: 1_250,
            through: 4,
        },
        &hub,
        &member_key(),
    );
    json!({
        "commitment": hex(&commitment.to_vec()),
        "commitmentDigest": hex(commitment.value.digest(&hub).as_bytes()),
        "claim": hex(&claim.to_vec()),
        "claimDigest": hex(claim.value.digest(&hub).as_bytes()),
    })
}

/// The first block after genesis, holding each validator's attestation of
/// the escrow: its payload, with the root of the results and of the state
/// the ledger leaves after it, the block, and validator 0's vote to
/// finalize it. The state root pins what the ledger does with a block.
fn block_vector(rules: &Rules) -> Value {
    let hub = rules.id();
    let operations = attestations(rules);
    let mut state = Ledger::genesis(rules.clone(), GENESIS_MS).unwrap();
    let timestamp = GENESIS_MS + 100;
    let outcomes = state
        .apply(&ledger::Block {
            height: 1,
            timestamp,
            operations: &operations,
            votes: &[],
        })
        .unwrap();
    assert!(state.escrow().checked, "a quorum attested the escrow");
    let payload = Payload {
        results: results_root(&operations, &outcomes),
        state: state.state_root(),
        operations,
        votes: Vec::new(),
    };
    let genesis = genesis_block(rules, GENESIS_MS);
    let block = Block {
        epoch: 0,
        view: 1,
        height: 1,
        parent: genesis.digest(&hub.0),
        parent_view: 0,
        timestamp_ms: timestamp,
        payload: payload.to_vec(),
    };
    let digest = block.digest(&hub.0);
    let vote = Vote::sign(
        &hub.0,
        &validator_keys()[0].consensus,
        ValidatorIndex::new(0).unwrap(),
        VoteKind::Finalize,
        (0, 1),
        digest,
    );
    let statements = [
        Statement::Final {
            height: 1,
            timestamp,
        },
        Statement::account(&state, validator_keys()[2].consensus.public_key()),
    ]
    .map(|statement| {
        SignedStatement::sign(
            &hub,
            ValidatorIndex::new(2).unwrap(),
            statement,
            &validator_keys()[2].consensus,
        )
    });
    json!({
        "payload": hex(&payload.to_vec()),
        "state": hex(payload.state.as_bytes()),
        "results": hex(payload.results.as_bytes()),
        "block": hex(&block.to_vec()),
        "blockDigest": hex(digest.as_bytes()),
        "vote": hex(&Message::Vote(vote).to_vec()),
        "statements": hex(&SignedStatement::encode_list(&statements)),
    })
}

/// The escrow's setup as the sponsor signs it, and a withdrawal as
/// validator 0 multi-signs it.
fn escrow_vector(rules: &Rules) -> Value {
    let master = escrow_master(&sponsor(), NETWORK, &NONCE);
    let setup: Vec<String> = setup(rules, &master, 3, 12)
        .iter()
        .map(|transaction| hex(transaction.blob()))
        .collect();
    let withdrawal = ledger::Withdrawal {
        account: Some(AccountNumber(5)),
        amount: 3_000_000,
        ledger_fee: rules.terms.fees.ledger,
        to: Destination {
            account: depositor().account().0,
            tag: Some(77),
        },
        closes: false,
        delivered: dassie_hub::Signers::NONE,
        failed: dassie_hub::Signers::NONE,
    };
    let payment = withdrawal_payment(rules, 9, &withdrawal);
    let escrow_key = &validator_keys()[0].escrow;
    let signer = payment.multi_sign(escrow_key.account(), escrow_key);
    json!({
        "account": master.account().to_address(),
        "setup": setup,
        "withdrawal": {
            "signer": escrow_key.account().to_address(),
            "signingPayload": hex(&payment.multi_signing_payload(escrow_key.account())),
            "signature": hex(signer.signature.as_bytes()),
        },
    })
}

/// Signs `operation` as validator `index`, with `sequence`, in the hub of
/// `rules`.
fn by_validator(
    rules: &Rules,
    index: usize,
    sequence: u64,
    operation: ValidatorOperation,
) -> Operation {
    Operation::Validator(SignedValidatorOperation::sign(
        &rules.id(),
        ValidatorIndex::new(index).unwrap(),
        sequence,
        operation,
        &validator_keys()[index].consensus,
    ))
}

/// Applies a block of `operations` at `height`, stamped `timestamp`, to
/// `state`, and returns its outcomes.
fn apply(
    state: &mut Ledger,
    height: u64,
    timestamp: Millis,
    operations: &[Operation],
) -> Vec<ledger::Outcome> {
    state
        .apply(&ledger::Block {
            height,
            timestamp,
            operations,
            votes: &[],
        })
        .unwrap()
}

/// Applies a block of `operations` to `state`, 100 ms after the last.
fn next(state: &mut Ledger, operations: &[Operation]) -> Vec<ledger::Outcome> {
    let (height, timestamp) = (state.height() + 1, state.timestamp() + 100);
    apply(state, height, timestamp, operations)
}

/// A quorum's attestation of `operation`, signed at the next height.
fn attested(rules: &Rules, state: &Ledger, operation: &ValidatorOperation) -> Vec<Operation> {
    (0..3)
        .map(|index| by_validator(rules, index, state.height() + 1, operation.clone()))
        .collect()
}

/// `operation` signed with `key` for `account`, at its next sequence.
fn by_member(
    state: &Ledger,
    account: AccountNumber,
    key: &NodeKey,
    operation: MemberOperation,
) -> Operation {
    let sequence = state.account(account).unwrap().sequence;
    let fee = state.terms().fees.operation;
    Operation::Member(SignedMemberOperation::sign(
        state.hub(),
        account,
        sequence,
        fee,
        operation,
        key,
    ))
}

/// A hub of `rules` with its escrow checked and two members, of keys 0x30
/// and 0x31, each joined with a deposit, the first holding funds for the
/// second: the ledger, the two keys, and the hold.
fn two_members(rules: &Rules) -> (Ledger, NodeKey, NodeKey, HoldNumber) {
    let payer = NodeKey::from_bytes(&[0x30; 32]);
    let payee = NodeKey::from_bytes(&[0x31; 32]);
    let mut state = Ledger::genesis(rules.clone(), GENESIS_MS).unwrap();
    let checked = attested(
        rules,
        &state,
        &ValidatorOperation::AttestEscrow { sequence: 5 },
    );
    next(&mut state, &checked);
    for (byte, key) in [(0xa1, &payer), (0xa2, &payee)] {
        let deposit = Deposit {
            transaction: Digest([byte; 32]),
            ledger: 1_000 + u32::from(byte),
            amount: OPEN_DROPS,
            sender: depositor().account().0,
            tag: 0,
            key: Some(key.public_key()),
        };
        let attested = attested(rules, &state, &ValidatorOperation::AttestDeposit(deposit));
        next(&mut state, &attested);
    }
    let number = |key: &NodeKey| state.account_of(&key.public_key()).unwrap();
    let (from, to) = (number(&payer), number(&payee));
    let disbursal = |key: &NodeKey| {
        MemberOperation::SetDisbursal(Destination {
            account: [key.public_key().as_bytes()[0]; 20],
            tag: None,
        })
    };
    let set = [
        by_member(&state, from, &payer, disbursal(&payer)),
        by_member(&state, to, &payee, disbursal(&payee)),
    ];
    next(&mut state, &set);
    let open = MemberOperation::HoldOpen {
        payee: to,
        amount: 1_000_000,
    };
    let open = by_member(&state, from, &payer, open);
    next(&mut state, &[open]);
    let hold = state.holds_of(from).next().unwrap().0;
    (state, payer, payee, hold)
}

/// `payer`'s commitment on `hold`, sequence 1, with its fulfillment.
fn fulfilled(hub: &dassie_hub::rules::HubId, payer: &NodeKey, hold: HoldNumber) -> Fulfilled {
    let fulfillment = [0xf1; 32];
    let commitment = Commitment {
        hold,
        sequence: 1,
        amount: 250,
        condition: sha256(&fulfillment),
        expiry: GENESIS_MS + 60_000,
    };
    Fulfilled {
        commitment: Signed::sign(commitment, hub, payer),
        fulfillment,
    }
}

/// From version 2 on (spec/decisions/signed-rejects.md): a payee's release
/// of a commitment and what it signs; a payer's dispute with it; and a
/// hub's states as a claim takes from a hold to wait, as the payer's
/// dispute puts it back, and as a claim that waited is paid, with the
/// holds a statement lists with what waits and the version a statement
/// says.
fn releases_vector(rules: &Rules) -> Value {
    let hub = rules.id();
    let (mut state, payer, payee, hold) = two_members(rules);
    let number = |key: &NodeKey| state.account_of(&key.public_key()).unwrap();
    let (from, to) = (number(&payer), number(&payee));
    let fee = rules.terms.fees.operation;
    let claim = MemberOperation::Claim {
        hold,
        fulfilled: vec![fulfilled(&hub, &payer, hold)],
    };
    let claim = by_member(&state, to, &payee, claim);
    let pending = ledger::Detail::Pending(250);
    assert_eq!(
        next(&mut state, core::slice::from_ref(&claim)),
        [ledger::Outcome::Applied {
            fee,
            detail: pending
        }]
    );
    let waiting = state.state_root();
    let holds = Statement::holds(&state, to);
    let mut paid = state.clone();
    let release = Signed::sign(
        Release::of(&fulfilled(&hub, &payer, hold).commitment.value),
        &hub,
        &payee,
    );
    let dispute = MemberOperation::Dispute {
        hold,
        releases: vec![release],
    };
    let dispute = by_member(&state, from, &payer, dispute);
    let returned = ledger::Detail::Returned(250);
    assert_eq!(
        next(&mut state, core::slice::from_ref(&dispute)),
        [ledger::Outcome::Applied {
            fee,
            detail: returned
        }]
    );
    // Undisputed, what waited is paid once the hold's notice passed.
    let due = paid.hold(hold).unwrap().pending_due;
    let height = paid.height() + 1;
    apply(&mut paid, height, due, &[]);
    assert!(paid.hold(hold).unwrap().pending.is_empty());
    let statements = holds
        .into_iter()
        .chain(Statement::protocol(&state))
        .map(|statement| {
            SignedStatement::sign(
                &hub,
                ValidatorIndex::new(1).unwrap(),
                statement,
                &validator_keys()[1].consensus,
            )
        })
        .collect::<Vec<_>>();
    json!({
        "release": hex(&release.to_vec()),
        "releaseDigest": hex(release.value.digest(&hub).as_bytes()),
        "claim": hex(&claim.to_vec()),
        "dispute": hex(&dispute.to_vec()),
        "disputeHash": hex(dispute.hash().as_bytes()),
        "waiting": hex(waiting.as_bytes()),
        "snapshot": hex(&state.snapshot()),
        "returned": hex(state.state_root().as_bytes()),
        "paid": hex(paid.state_root().as_bytes()),
        "statements": hex(&SignedStatement::encode_list(&statements)),
    })
}

/// A hub of version `from` moved to version `to`
/// (spec/decisions/hub-protocol-version.md): a validator's vote, and the
/// hub's states after a quorum's votes and once their notice passed, which
/// pin how the version and the move waiting are in the state root.
fn upgrade_vector(from: u16, to: u16) -> Value {
    let rules = rules(from);
    let vote = ValidatorOperation::Upgrade { version: to };
    let mut state = Ledger::genesis(rules.clone(), GENESIS_MS).unwrap();
    let votes: Vec<Operation> = (0..3)
        .map(|index| by_validator(&rules, index, 1, vote.clone()))
        .collect();
    apply(&mut state, 1, GENESIS_MS + 100, &votes);
    let voted = state.state_root();
    let at = state.pending_upgrade().unwrap().at;
    apply(&mut state, 2, at, &[]);
    assert_eq!(state.version(), to);
    json!({
        "rules": hex(&rules.to_vec()),
        "vote": hex(&votes[0].to_vec()),
        "voteHash": hex(votes[0].hash().as_bytes()),
        "voted": hex(voted.as_bytes()),
        "moved": hex(state.state_root().as_bytes()),
        "snapshot": hex(&state.snapshot()),
    })
}

/// The SHA-256 of `bytes`.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(bytes).into()
}

/// Every vector of `version` of the hub's protocol, as
/// `testdata/hub/vectors-v<version>.json` holds them.
pub fn all(version: u16) -> Value {
    let rules = rules(version);
    let mut vectors = json!({
        "comment": "Generated by crates/dassie-hub/examples/hub_vectors.rs; see its header. Do not edit.",
        "open": open(),
        "rules": rules_vector(&rules),
        "genesis": genesis_vector(&rules),
        "operations": operations_vector(&rules),
        "promises": promises_vector(&rules),
        "block": block_vector(&rules),
        "escrow": escrow_vector(&rules),
        "token": token_vector(version),
    });
    if version >= 2 {
        vectors["releases"] = releases_vector(&rules);
        // Each version's vectors move a hub of the version before to it.
        vectors["upgrade"] = upgrade_vector(version - 1, version);
    }
    vectors
}
