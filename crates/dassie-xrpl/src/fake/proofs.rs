//! The proof methods of a patched rippled, as the fake serves them
//! (spec/designs/xrpl-validation.md#3-the-proof-api), and rippled's
//! `manifest`:
//!
//! - `validations` `{ledger_hash}`: `{ledger_hash, validations}`, each the
//!   hex of a validation the fake holds for that ledger.
//! - `tx_proof` `{tx_hash, ledger_index?}`: `{tx_blob, meta_blob,
//!   ledger_index, ledger_header, path}`, the branch in the ledger's
//!   transaction tree leaf first, each node in hex.
//! - `state_proof` `{keys, ledger_index | ledger_hash}`, `"validated"` or
//!   none for the validated ledger: `{ledger_index, ledger_header,
//!   entries}`, each `{key, node_binary, path}`, or `{key, absent, path}`
//!   for a key the state does not hold, with the branch that proves it.
//! - `manifest` `{public_key}`: `{requested, manifest, details}`.
//!
//! A ledger beyond what the fake keeps is `lgrNotFound`. A lying
//! connection answers each as its lie has it ([`super::lies`]).

use alloc::string::String;
use alloc::vec::Vec;

use serde_json::{Value, json};

use super::chain::CLOSE_TIME_RESOLUTION_S;
use super::lies::Lie;
use super::meta::{affected_node, message, metadata};
use super::objects::{self, BALANCE, HASHES, Object};
use super::shamap::ShaMap;
use super::{
    CLOSE_INTERVAL_S, ConnectionId, FAUCET_DROPS, FEE_BASE, FakeLedger, RESTART_LEDGER, unsigned,
};
use crate::base64;
use crate::binary::{FieldId, Fields};
use crate::codec::{Amount, Payment, TokenValue, Transact, Transaction, hash};
use crate::header::LedgerHeader;
use crate::keys::{AccountId, PublicKey, sha512_half};
use crate::shamap::Tree;
use crate::unl::FRESH_FOR_MS;
use crate::{DROPS_PER_XRP, MAX_DROPS, from_hex, hex_upper};

/// The most keys one `state_proof` names: an account's root, its lines,
/// their issuers, the fee settings, the skip lists and a pool, with room.
pub const MAX_STATE_PROOF_KEYS: usize = 32;
/// How much longer before the newest ledger than its validations stay
/// fresh an old anchor closed, in seconds.
const STALE_BY_S: u32 = 60;
/// How many ledgers back an anchor older than a newer one is: its
/// validations are still fresh.
const OLDER_ANCHOR_LEDGERS: u32 = 2;

fn error(name: &str) -> Value {
    json!({ "error": name })
}

/// A 32-byte hash in hex, as `request` names it under `key`.
fn hash_param(request: &Value, key: &str) -> Option<[u8; 32]> {
    let text = request.get(key)?.as_str()?;
    from_hex(text)?.try_into().ok()
}

/// A branch as the answers write it: each node in hex.
fn path(branch: Option<Vec<Vec<u8>>>) -> Value {
    (branch.unwrap_or_default().iter())
        .map(|node| Value::from(hex_upper(node)))
        .collect::<Vec<_>>()
        .into()
}

impl FakeLedger {
    /// Whether `connection` serves the proof methods: not a stock server,
    /// nor one that refuses.
    fn serves_proofs(&mut self, connection: ConnectionId) -> Result<(), Value> {
        match self.lie(connection) {
            Some(Lie::NoProofs) => {
                self.told(Lie::NoProofs);
                Err(error("unknownCmd"))
            }
            Some(Lie::Refusal) => {
                self.told(Lie::Refusal);
                Err(error("tooBusy"))
            }
            _ => Ok(()),
        }
    }

    /// `validations`.
    pub(super) fn validations(
        &mut self,
        connection: ConnectionId,
        request: &Value,
    ) -> Result<Value, Value> {
        self.serves_proofs(connection)?;
        let hash = hash_param(request, "ledger_hash").ok_or_else(|| error("invalidParams"))?;
        let validations = match self.chain.index_of(&hash) {
            Some(index) => self.lied_validations(connection, index),
            // None held. A liar shows the newest ledger's, which sign
            // another hash.
            None if self.lie(connection).is_some() => {
                let latest = self.chain.latest().map(|sealed| sealed.header.sequence);
                latest.map_or_else(Vec::new, |index| self.validations_of(index))
            }
            None => Vec::new(),
        };
        let validations: Vec<Value> = (validations.iter())
            .map(|bytes| Value::from(hex_upper(bytes)))
            .collect();
        Ok(json!({ "ledger_hash": hex_upper(&hash), "validations": validations }))
    }

    /// The validations of the ledger of `index`, as `connection` has them.
    fn lied_validations(&mut self, connection: ConnectionId, index: u32) -> Vec<Vec<u8>> {
        let honest = self.validations_of(index);
        match self.lie(connection) {
            Some(Lie::ForgedValidations) => {
                let Some(sealed) = self.chain.get(index) else {
                    return honest;
                };
                let ledger = (sealed.hash, index, sealed.header.close_time);
                let forged = self.validators.forged(ledger, &sealed.signers);
                self.told(Lie::ForgedValidations);
                forged
            }
            Some(Lie::ReplayedValidations) => {
                let replayed = self.validations_of(index.saturating_sub(1));
                if replayed.is_empty() {
                    return honest;
                }
                self.told(Lie::ReplayedValidations);
                replayed
            }
            Some(Lie::WithheldValidations) => {
                self.told(Lie::WithheldValidations);
                honest.into_iter().take(1).collect()
            }
            _ => honest,
        }
    }

    /// `header` as `connection` serves it.
    fn served_header(&mut self, connection: ConnectionId, header: LedgerHeader) -> String {
        let mut header = header;
        if self.lies_with(connection, Lie::WrongHeader) {
            header.close_time = header.close_time.wrapping_add(1);
            self.told(Lie::WrongHeader);
        }
        hex_upper(&header.to_bytes())
    }

    /// `tx_proof`.
    pub(super) fn tx_proof(
        &mut self,
        connection: ConnectionId,
        request: &Value,
    ) -> Result<Value, Value> {
        self.serves_proofs(connection)?;
        if self.lies_with(connection, Lie::OldLedgerHint) {
            self.told(Lie::OldLedgerHint);
            return Err(error("lgrNotFound"));
        }
        let invalid = || error("invalidParams");
        let tx_hash = hash_param(request, "tx_hash").ok_or_else(invalid)?;
        let asked = match request.get("ledger_index") {
            None => None,
            Some(index) => Some(
                (index.as_u64())
                    .and_then(|index| u32::try_from(index).ok())
                    .ok_or_else(invalid)?,
            ),
        };
        let found = (self.history_index.get(&tx_hash))
            .and_then(|position| self.history.get(*position))
            .map(|applied| (applied.ledger, applied.blob.clone(), applied.meta.clone()));
        let Some((ledger, blob, meta)) = found else {
            if self.lies_with(connection, Lie::Transaction)
                && let Some(answer) = self.made_up_proof(&tx_hash)
            {
                self.told(Lie::Transaction);
                return Ok(answer);
            }
            return Err(error("txnNotFound"));
        };
        let within = |index: u32| index >= self.history_from && index <= self.ledger;
        if let Some(asked) = asked
            && asked != ledger
        {
            return Err(error(if within(asked) {
                "txnNotFound"
            } else {
                "lgrNotFound"
            }));
        }
        let header = (self.chain.get(ledger))
            .filter(|_| within(ledger))
            .map(|sealed| sealed.header)
            .ok_or_else(|| error("lgrNotFound"))?;
        let tree = self.transactions(ledger);
        let header = self.served_header(connection, header);
        Ok(json!({
            "tx_blob": hex_upper(&blob),
            "meta_blob": hex_upper(&meta),
            "ledger_index": ledger,
            "ledger_header": header,
            "path": path(tree.branch(&tx_hash)),
        }))
    }

    /// `state_proof`.
    pub(super) fn state_proof(
        &mut self,
        connection: ConnectionId,
        request: &Value,
    ) -> Result<Value, Value> {
        self.serves_proofs(connection)?;
        let invalid = || error("invalidParams");
        let keys = request
            .get("keys")
            .and_then(Value::as_array)
            .filter(|keys| !keys.is_empty() && keys.len() <= MAX_STATE_PROOF_KEYS)
            .ok_or_else(invalid)?;
        let keys: Vec<[u8; 32]> = (keys.iter())
            .map(|key| {
                let bytes = key.as_str().and_then(from_hex).ok_or_else(invalid)?;
                bytes.try_into().map_err(|_| invalid())
            })
            .collect::<Result<_, _>>()?;
        let (header, tree) = self.proven_ledger(connection, request)?;
        let mut entries = Vec::new();
        let mut told = Vec::new();
        for key in &keys {
            let (entry, lie) = self.entry(connection, &tree, key);
            entries.push(entry);
            told.extend(lie);
        }
        for lie in told {
            self.told(lie);
        }
        let ledger_index = header.sequence;
        let header = self.served_header(connection, header);
        Ok(json!({
            "ledger_index": ledger_index,
            "ledger_header": header,
            "entries": entries,
        }))
    }

    /// The ledger a `state_proof` asks for, as `connection` has it: its
    /// header and its state.
    fn proven_ledger(
        &mut self,
        connection: ConnectionId,
        request: &Value,
    ) -> Result<(LedgerHeader, ShaMap), Value> {
        let not_found = || error("lgrNotFound");
        let invalid = || error("invalidParams");
        let validated = match request.get("ledger_index") {
            None => request.get("ledger_hash").is_none(),
            Some(Value::String(text)) => text == "validated",
            Some(_) => false,
        };
        let index = if validated {
            match self.validated_lie(connection) {
                Some((lie, Some(index))) => {
                    self.told(lie);
                    index
                }
                Some((lie, None)) => {
                    self.told(lie);
                    return Ok(self.made_up_chain());
                }
                None => (self.chain.latest())
                    .map(|sealed| sealed.header.sequence)
                    .ok_or_else(not_found)?,
            }
        } else if request.get("ledger_hash").is_some() {
            let hash = hash_param(request, "ledger_hash").ok_or_else(invalid)?;
            self.chain.index_of(&hash).ok_or_else(not_found)?
        } else {
            (request.get("ledger_index").and_then(Value::as_u64))
                .and_then(|index| u32::try_from(index).ok())
                .ok_or_else(invalid)?
        };
        let sealed = self.chain.get(index).ok_or_else(not_found)?;
        let state = sealed.state.clone().ok_or_else(not_found)?;
        Ok((sealed.header, state))
    }

    /// The validated ledger as `connection` has it, if it lies about it:
    /// with the lie, an older ledger's index, or none for a made-up
    /// reset's.
    pub(super) fn validated_lie(&self, connection: ConnectionId) -> Option<(Lie, Option<u32>)> {
        let lie = self.lie(connection)?;
        let latest = self.chain.latest()?.header;
        let older = match lie {
            Lie::MadeUpReset => return Some((lie, None)),
            Lie::OldAnchor => {
                let fresh_s = u32::try_from(FRESH_FOR_MS / 1_000).unwrap_or(u32::MAX);
                let stale = latest
                    .close_time
                    .checked_sub(fresh_s.saturating_add(STALE_BY_S))?;
                self.chain.closed_before(stale)?
            }
            Lie::OlderAnchor => {
                let older = latest.sequence.checked_sub(OLDER_ANCHOR_LEDGERS)?;
                self.chain.get(older)?.state.as_ref()?;
                older
            }
            _ => return None,
        };
        Some((lie, Some(older)))
    }

    /// A chain made up as a reset's: a lower ledger that closed later,
    /// holding genesis and the fees alone.
    pub(super) fn made_up_chain(&self) -> (LedgerHeader, ShaMap) {
        let mut tree = ShaMap::new(Tree::State);
        tree.insert_object(
            objects::fees_key(),
            &objects::fee_settings(FEE_BASE, self.reserve).to_bytes(),
        );
        if let Some(genesis) = self.accounts.get(&self.genesis) {
            let root = objects::account_root(self.genesis, genesis, None, objects::UNTHREADED);
            tree.insert_object(objects::account_key(self.genesis), &root.to_bytes());
        }
        let latest = self.chain.latest().map(|sealed| sealed.header);
        let close_time = latest.map_or(0, |header| header.close_time);
        let interval = u32::try_from(CLOSE_INTERVAL_S).unwrap_or(u32::MAX);
        let header = LedgerHeader {
            sequence: RESTART_LEDGER.min(self.ledger.saturating_sub(1)),
            total_drops: MAX_DROPS,
            parent_hash: [0; 32],
            transaction_hash: [0; 32],
            account_hash: tree.root(),
            parent_close_time: 0,
            close_time: close_time.saturating_add(interval),
            close_time_resolution: CLOSE_TIME_RESOLUTION_S,
            close_flags: 0,
        };
        (header, tree)
    }

    /// The entry of `key` in `tree`, as `connection` gives it, and the lie
    /// it told, if it told one.
    fn entry(
        &self,
        connection: ConnectionId,
        tree: &ShaMap,
        key: &[u8; 32],
    ) -> (Value, Option<Lie>) {
        let key_hex = hex_upper(key);
        // `object` with the branch of `of`.
        let held = |object: &[u8], of: &[u8; 32]| {
            let path = path(tree.branch(of));
            json!({ "key": key_hex, "node_binary": hex_upper(object), "path": path })
        };
        let absent = || json!({ "key": key_hex, "absent": true, "path": path(tree.branch(key)) });
        let Some(object) = tree.object(key) else {
            return (absent(), None);
        };
        let lie = self.lie(connection);
        let lied = match lie {
            Some(Lie::AlteredObject) => altered(object).map(|altered| held(&altered, key)),
            Some(Lie::FalseAbsence) => Some(absent()),
            Some(Lie::OtherBranch) => (tree.keys().find(|other| *other > key))
                .or_else(|| tree.keys().find(|other| *other != key))
                .and_then(|other| Some(held(tree.object(other)?, other))),
            Some(Lie::OtherChainSkipList) => other_chain(object).map(|altered| held(&altered, key)),
            _ => None,
        };
        match lied {
            Some(entry) => (entry, lie),
            None => (held(object, key), None),
        }
    }

    /// A made-up proof of the transaction of `tx_hash`: a Payment of test
    /// XRP to an account, or a transaction that never applied, said to have
    /// succeeded, in the newest ledger with a header made up for it.
    fn made_up_proof(&self, tx_hash: &[u8; 32]) -> Option<Value> {
        let made_up = (self.accounts.keys())
            .filter_map(|account| self.made_up_payment(*account))
            .find(|(blob, _)| hash(blob) == *tx_hash);
        let (blob, meta) = match made_up {
            Some(found) => found,
            None => {
                let (_, blob) = (self.never_applied.iter()).find(|(hash, _)| hash == tx_hash)?;
                (blob.clone(), self.made_up_success(blob)?)
            }
        };
        let latest = self.chain.latest()?.header;
        let mut tree = self.transactions(latest.sequence);
        tree.insert_transaction(&blob, &meta);
        let header = LedgerHeader {
            transaction_hash: tree.root(),
            ..latest
        };
        Some(json!({
            "tx_blob": hex_upper(&blob),
            "meta_blob": hex_upper(&meta),
            "ledger_index": latest.sequence,
            "ledger_header": hex_upper(&header.to_bytes()),
            "path": path(tree.branch(tx_hash)),
        }))
    }

    /// The made-up Payment of test XRP from genesis to `account`, with its
    /// metadata: the same each time it is asked for, so its hash stays.
    pub(super) fn made_up_payment(&self, account: AccountId) -> Option<(Vec<u8>, Vec<u8>)> {
        if account == self.genesis {
            return None;
        }
        let target = *self.accounts.get(&account)?;
        let genesis = *self.accounts.get(&self.genesis)?;
        let sequence = account
            .0
            .first_chunk::<4>()
            .map_or(1, |bytes| u32::from_be_bytes(*bytes));
        let payment = Payment {
            account: self.genesis,
            destination: account,
            destination_tag: None,
            amount: Amount::Xrp(FAUCET_DROPS),
            send_max: None,
            fee: FEE_BASE,
            sequence,
            last_ledger_sequence: None,
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing: unsigned(),
        };
        let blob = payment.blob();
        let paid = super::Account {
            balance: target.balance.saturating_add(FAUCET_DROPS),
            ..target
        };
        let paying = super::Account {
            balance: genesis
                .balance
                .saturating_sub(FAUCET_DROPS.saturating_add(FEE_BASE)),
            sequence: genesis.sequence.saturating_add(1),
            ..genesis
        };
        let nodes = [
            self.made_up_node(account, &target, &paid),
            self.made_up_node(self.genesis, &genesis, &paying),
        ];
        Some((blob, self.made_up_meta(&nodes)))
    }

    /// Made-up metadata of a transaction that never applied, said to have
    /// succeeded: its fee taken and its sequence used.
    fn made_up_success(&self, blob: &[u8]) -> Option<Vec<u8>> {
        let tx = Transaction::decode(blob).ok()?;
        let sender = *self.accounts.get(&tx.sender())?;
        let after = super::Account {
            balance: sender.balance.saturating_sub(tx.fee()),
            sequence: tx.sequence().saturating_add(1),
            ..sender
        };
        Some(self.made_up_meta(&[self.made_up_node(tx.sender(), &sender, &after)]))
    }

    /// An affected node of `id`'s root, made up from `before` to `after`.
    fn made_up_node(
        &self,
        id: AccountId,
        before: &super::Account,
        after: &super::Account,
    ) -> Option<([u8; 32], FieldId, Object)> {
        let key = objects::account_key(id);
        let previous = self.thread(&key);
        let amm = self.amm_of(id);
        let before = objects::account_root(id, before, amm, previous);
        let after = objects::account_root(id, after, amm, previous);
        let (kind, node) = affected_node(&key, Some(&before), Some(&after), None, previous)?;
        Some((key, kind, node))
    }

    /// Successful metadata of `nodes`, last in the newest ledger.
    fn made_up_meta(&self, nodes: &[Option<([u8; 32], FieldId, Object)>]) -> Vec<u8> {
        let index = self.transactions(self.ledger).len();
        let nodes: Vec<_> = nodes.iter().flatten().cloned().collect();
        metadata(
            u32::try_from(index).unwrap_or(u32::MAX),
            "tesSUCCESS",
            &nodes,
        )
    }

    /// The made-up Payment to `account` as `account_tx` lists it, if there
    /// is one.
    pub(super) fn made_up_entry(&self, account: AccountId) -> Option<Value> {
        let (blob, meta) = self.made_up_payment(account)?;
        let delivered = Amount::Xrp(FAUCET_DROPS);
        let message = message(
            self.ledger,
            &hash(&blob),
            &blob,
            &meta,
            "tesSUCCESS",
            Some(&delivered),
        );
        let mut tx = message.get("transaction").cloned().unwrap_or(Value::Null);
        if let Some(object) = tx.as_object_mut() {
            object.insert("ledger_index".into(), self.ledger.into());
        }
        Some(json!({ "tx": tx, "meta": message.get("meta"), "validated": true }))
    }

    /// `manifest`: the newest manifest of a validator, by its master key or
    /// its current signing key, as a node public key (`n…`).
    pub(super) fn manifest(&self, request: &Value) -> Result<Value, Value> {
        let requested = request
            .get("public_key")
            .and_then(Value::as_str)
            .ok_or_else(|| error("invalidParams"))?;
        let key = PublicKey::from_node_public(requested).ok_or_else(|| error("invalidParams"))?;
        let Some((manifest, master, signing, sequence)) = self.validators.manifest(&key) else {
            return Ok(json!({
                "requested": requested,
                "details": { "master_key": requested, "ephemeral_key": requested },
            }));
        };
        let mut details = json!({ "master_key": master.to_node_public(), "seq": sequence });
        if let (Some(signing), Some(object)) = (signing, details.as_object_mut()) {
            object.insert("ephemeral_key".into(), signing.to_node_public().into());
        }
        Ok(json!({
            "requested": requested,
            "manifest": base64::encode(manifest),
            "details": details,
        }))
    }
}

/// `object` holding more than it does: a root's XRP or a line's balance
/// raised by one; `None` for an object without a balance.
fn altered(object: &[u8]) -> Option<Vec<u8>> {
    let balance = (Fields::new(object).filter_map(Result::ok)).find(|field| field.id == BALANCE)?;
    let more = match balance.amount().ok()? {
        Amount::Xrp(drops) => Amount::Xrp(drops.saturating_add(DROPS_PER_XRP).min(MAX_DROPS)),
        Amount::Token(mut token) => {
            token.value = token.value.checked_add(TokenValue::new(false, 1, 0)?)?;
            Amount::Token(token)
        }
    };
    let mut fields = Object::from_bytes(object)?;
    fields.put_amount(BALANCE, &more);
    Some(fields.to_bytes())
}

/// A skip list of another chain: each hash it holds made up.
fn other_chain(object: &[u8]) -> Option<Vec<u8>> {
    let mut fields = Object::from_bytes(object)?;
    if fields.entry_type() != Some(objects::LEDGER_HASHES) {
        return None;
    }
    let hashes: Vec<u8> = (fields.get(HASHES)?.as_chunks::<32>().0.iter())
        .flat_map(|hash| sha512_half(&[hash.as_slice(), b"another chain"].concat()))
        .collect();
    fields.put(HASHES, &hashes);
    Some(fields.to_bytes())
}
