//! The hub's golden vectors (spec/areas/hubs.md#golden-vectors), as the
//! crate's code makes them from fixed keys. `examples/hub_vectors.rs` writes
//! them to `testdata/hub/vectors.json`; `tests/vectors.rs` checks that the
//! code still makes and reads exactly those bytes. They pin what every
//! validator of a hub must agree on within its version
//! ([`HUB_VERSION`]): the rules and the ID, operations, promises,
//! statements, blocks and the state a block leaves, votes, the genesis,
//! and the escrow's transactions.

use dassie_hub::client::{SignedStatement, Statement};
use dassie_hub::consensus::wire::Message;
use dassie_hub::consensus::{Block, Vote, VoteKind};
use dassie_hub::escrow::{OPEN_MEMO_TYPE, ValidatorKeys, escrow_master, setup, withdrawal_payment};
use dassie_hub::founding::{GenesisCertificate, genesis_digest, starting_terms};
use dassie_hub::ledger::{self, Ledger, results_root};
use dassie_hub::operation::{
    AccountNumber, Deposit, Destination, MemberOperation, Operation, SignedMemberOperation,
    SignedValidatorOperation, ValidatorOperation,
};
use dassie_hub::promise::{Claim, Commitment, HoldNumber, Signed};
use dassie_hub::rules::{HUB_VERSION, Rules};
use dassie_hub::service::genesis_block;
use dassie_hub::service::payload::Payload;
use dassie_hub::{Digest, Millis, ValidatorIndex};
use dassie_identity::NodeKey;
use dassie_oer::Encode;
use dassie_xrpl::codec::{Amount, Memo, Payment, Signing, Transact};
use dassie_xrpl::{AccountId, Wallet, hex_upper};
use serde_json::{Value, json};

/// Bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    hex_upper(bytes).to_lowercase()
}

/// The node key of the seed `byte` repeated.
fn key(byte: u8) -> NodeKey {
    NodeKey::from_bytes(&[byte; 32])
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
fn sponsor() -> NodeKey {
    key(0x20)
}

/// The node keys of the four validators.
fn validator_nodes() -> Vec<NodeKey> {
    (0x21..=0x24).map(key).collect()
}

/// The keys each validator makes for the hub.
pub fn validator_keys() -> Vec<ValidatorKeys> {
    validator_nodes()
        .iter()
        .map(|node| ValidatorKeys::derive(node, NETWORK, &NONCE).unwrap())
        .collect()
}

/// The rules of the hub the vectors are of: the starting terms of the
/// sponsor of key 0x20, its escrow at the master key it derives, and four
/// validators, of keys 0x21 to 0x24, each accepting at its own host.
pub fn rules() -> Rules {
    let sponsor = sponsor();
    let escrow = escrow_master(&sponsor, NETWORK, &NONCE);
    let terms = starting_terms(NONCE, sponsor.public_key(), NETWORK, escrow.account().0);
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
        "version": HUB_VERSION,
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

/// Every vector, as `testdata/hub/vectors.json` holds them.
pub fn all() -> Value {
    let rules = rules();
    json!({
        "comment": "Generated by crates/dassie-hub/examples/hub_vectors.rs; see its header. Do not edit.",
        "open": open(),
        "rules": rules_vector(&rules),
        "genesis": genesis_vector(&rules),
        "operations": operations_vector(&rules),
        "promises": promises_vector(&rules),
        "block": block_vector(&rules),
        "escrow": escrow_vector(&rules),
    })
}
