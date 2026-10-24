//! What the fake ledger writes about a transaction it applied: the
//! transaction and its metadata in binary, as a ledger's transaction tree
//! holds them, and the JSON rendered of them, which the streams,
//! `account_tx` and `tx` give.
//!
//! The metadata follows rippled's rules (`ApplyStateTable::apply`): each
//! object the transaction created, changed or deleted, by key, with its
//! key and kind. A created one with its fields that are not at their
//! defaults; a changed one with its fields as they are and those that
//! changed as they were, and the transaction that changed it before; a
//! deleted one as it ended, and what changed of it. An object names the
//! last transaction that changed it (`PreviousTxnID`), and a trust line or
//! a pool made or deleted names it in its owners' roots too, so an owner
//! the transaction did not otherwise change is listed with that alone.

use alloc::string::ToString;
use alloc::vec::Vec;

use serde_json::{Value, json};

use super::objects::{
    self, AFFECTED_NODES, CREATED_NODE, DELETED_NODE, FINAL_FIELDS, LEDGER_ENTRY_TYPE,
    LEDGER_INDEX, MODIFIED_NODE, NEW_FIELDS, Object, PREVIOUS_FIELDS, PREVIOUS_TXN_ID,
    PREVIOUS_TXN_LGR_SEQ, TRANSACTION_INDEX, TRANSACTION_RESULT, Thread, UNTHREADED,
};
use super::pools::{Pool, pool_account};
use super::signers::signer_list_index;
use super::tokens::{LineKey, Token, line_index};
use super::{Account, FakeLedger, Outcome, Tx};
use crate::api::{SignerList, TrustLine};
use crate::binary::FieldId;
use crate::codec::{Amount, Transact, Transaction, hash};
use crate::hex_upper;
use crate::keys::AccountId;

/// A transaction in a validated ledger, with its metadata.
#[derive(Clone, Debug)]
pub(super) struct Applied {
    pub ledger: u32,
    pub hash: [u8; 32],
    /// The accounts it touched, whose streams and histories carry it.
    pub accounts: Vec<AccountId>,
    /// The transaction in binary.
    pub blob: Vec<u8>,
    /// Its metadata in binary.
    pub meta: Vec<u8>,
    pub message: Value,
}

/// What a transaction wrote, each with what it was before.
#[derive(Default)]
pub(super) struct Touched {
    accounts: Vec<(AccountId, Option<Account>)>,
    /// Each line before, and as last written: what a deleted line ended as.
    lines: Vec<(LineKey, Option<TrustLine>, TrustLine)>,
    /// Each signer list before, and as last written, by its owner.
    signer_lists: Vec<(AccountId, Option<SignerList>, SignerList)>,
    /// Each pool before, by its token.
    pools: Vec<(Token, Option<Pool>)>,
}

impl Touched {
    /// `id` is written; it was `before`, unless it was written already.
    pub fn account(&mut self, id: AccountId, before: Option<Account>) {
        if !self.accounts.iter().any(|(account, _)| *account == id) {
            self.accounts.push((id, before));
        }
    }

    /// The line `key` is written as `written`; it was `before`, unless it
    /// was written already.
    pub fn line(&mut self, key: LineKey, before: Option<TrustLine>, written: TrustLine) {
        match self.lines.iter_mut().find(|(line, _, _)| *line == key) {
            Some((_, _, last)) => *last = written,
            None => self.lines.push((key, before, written)),
        }
    }

    /// `owner`'s signer list is written as `written`, or deleted as it
    /// was; it was `before`, unless it was written already.
    pub fn signer_list(
        &mut self,
        owner: AccountId,
        before: Option<SignerList>,
        written: SignerList,
    ) {
        match self.signer_lists.iter_mut().find(|(id, _, _)| *id == owner) {
            Some((_, _, last)) => *last = written,
            None => self.signer_lists.push((owner, before, written)),
        }
    }

    /// The pool of `token` is written; it was `before`, unless it was
    /// written already.
    pub fn pool(&mut self, token: Token, before: Option<Pool>) {
        if !self.pools.iter().any(|(pool, _)| *pool == token) {
            self.pools.push((token, before));
        }
    }
}

/// An account's fields, as `account_info` writes them.
pub(super) fn account_fields(id: AccountId, account: &Account) -> Value {
    let mut fields = json!({
        "Account": id.to_address(),
        "Balance": account.balance.to_string(),
        "Flags": account.flags,
        "OwnerCount": account.owner_count,
        "Sequence": account.sequence,
    });
    if let Some(object) = fields.as_object_mut() {
        if account.transfer_rate != 0 {
            object.insert("TransferRate".into(), account.transfer_rate.into());
        }
        if let Some(key) = account.regular_key {
            object.insert("RegularKey".into(), key.to_address().into());
        }
    }
    fields
}

/// A line's fields, as `ledger_entry` and `account_objects` write them:
/// the balance from the low account's side, its flags and each side's
/// limit, issued by that side.
pub(super) fn line_fields(line: &TrustLine) -> Value {
    let limit = |issuer: AccountId, value: crate::codec::TokenValue| {
        objects::token_json(&crate::codec::TokenAmount {
            value,
            currency: line.currency,
            issuer,
        })
    };
    json!({
        "Balance": {
            "currency": line.currency.to_string(),
            "issuer": objects::ACCOUNT_ONE.to_address(),
            "value": line.balance.to_string(),
        },
        "Flags": line.flags,
        "HighLimit": limit(line.high, line.high_limit),
        "LowLimit": limit(line.low, line.low_limit),
    })
}

/// One object a transaction wrote, under its key: as it was and as it is,
/// each naming the transaction that changed it before; what a deleted one
/// ended as; the accounts whose roots a made or deleted one names it in;
/// and the accounts whose streams and histories carry a change of it.
struct Change {
    key: [u8; 32],
    before: Option<Object>,
    after: Option<Object>,
    last: Option<Object>,
    owners: Vec<AccountId>,
    accounts: Vec<AccountId>,
}

/// An affected node: the change to the object under `key`, `before` and
/// `after`, or `last` if it was deleted, with `previous`, the transaction
/// that changed it before. None if it did not change.
pub(super) fn affected_node(
    key: &[u8; 32],
    before: Option<&Object>,
    after: Option<&Object>,
    last: Option<&Object>,
    previous: Thread,
) -> Option<(FieldId, Object)> {
    let any = after.or(before)?;
    let mut node = Object::default();
    node.put(LEDGER_ENTRY_TYPE, any.get(LEDGER_ENTRY_TYPE)?);
    node.put(LEDGER_INDEX, key);
    // Fields kept on the node, or only as an object ended.
    let own = |id| id == LEDGER_ENTRY_TYPE || id == PREVIOUS_TXN_ID || id == PREVIOUS_TXN_LGR_SEQ;
    // What `from` held that `to` does not, as it was.
    let changed = |from: &Object, to: &Object| {
        let mut fields = Object::default();
        for (id, value) in from.iter().filter(|(id, _)| !own(*id)) {
            if to.get(id) != Some(value) {
                fields.put(id, value);
            }
        }
        fields
    };
    let put = |node: &mut Object, id, fields: Object| {
        if !fields.is_empty() {
            node.put(id, &fields.to_bytes());
        }
    };
    let kind = match (before, after) {
        (None, Some(after)) => {
            let mut fields = Object::default();
            for (id, value) in after.iter() {
                if !own(id) && !objects::is_default(id, value) {
                    fields.put(id, value);
                }
            }
            put(&mut node, NEW_FIELDS, fields);
            CREATED_NODE
        }
        (Some(before), Some(after)) => {
            if before == after {
                return None;
            }
            if any.entry_type().is_some_and(objects::is_threaded) && previous != UNTHREADED {
                node.put(PREVIOUS_TXN_ID, &previous.0);
                node.put_u32(PREVIOUS_TXN_LGR_SEQ, previous.1);
            }
            put(&mut node, PREVIOUS_FIELDS, changed(before, after));
            let mut fields = after.clone();
            for id in [LEDGER_ENTRY_TYPE, PREVIOUS_TXN_ID, PREVIOUS_TXN_LGR_SEQ] {
                fields.remove(id);
            }
            put(&mut node, FINAL_FIELDS, fields);
            MODIFIED_NODE
        }
        (Some(before), None) => {
            let last = last.unwrap_or(before);
            put(&mut node, PREVIOUS_FIELDS, changed(before, last));
            let mut fields = last.clone();
            fields.remove(LEDGER_ENTRY_TYPE);
            put(&mut node, FINAL_FIELDS, fields);
            DELETED_NODE
        }
        (None, None) => return None,
    };
    Some((kind, node))
}

/// The metadata of a transaction at `index` with `result`, of `nodes`,
/// each its key, its kind and itself.
pub(super) fn metadata(index: u32, result: &str, nodes: &[([u8; 32], FieldId, Object)]) -> Vec<u8> {
    let mut written = Vec::new();
    let mut sorted: Vec<&([u8; 32], FieldId, Object)> = nodes.iter().collect();
    sorted.sort_by_key(|(key, _, _)| *key);
    for (_, kind, node) in sorted {
        crate::binary::write_field(&mut written, *kind, &node.to_bytes());
    }
    let mut meta = Object::default();
    meta.put_u32(TRANSACTION_INDEX, index);
    meta.put(TRANSACTION_RESULT, &[objects::result_code(result)]);
    meta.put(AFFECTED_NODES, &written);
    meta.to_bytes()
}

/// The message the streams carry of a transaction in `ledger`, from its
/// binary forms; `delivered` is what a successful Payment delivered, which
/// rippled adds to the JSON.
pub(super) fn message(
    ledger: u32,
    tx_hash: &[u8; 32],
    blob: &[u8],
    meta: &[u8],
    result: &str,
    delivered: Option<&Amount>,
) -> Value {
    let mut transaction = objects::to_json(blob).unwrap_or(Value::Null);
    if let Some(object) = transaction.as_object_mut() {
        object.insert("hash".into(), hex_upper(tx_hash).into());
    }
    let mut meta = objects::to_json(meta).unwrap_or(Value::Null);
    if let (Some(delivered), Some(object)) = (delivered, meta.as_object_mut()) {
        let delivered = match delivered {
            Amount::Xrp(drops) => Value::from(alloc::string::ToString::to_string(drops)),
            Amount::Token(token) => objects::token_json(token),
        };
        object.insert("delivered_amount".into(), delivered);
    }
    json!({
        "type": "transaction",
        "validated": true,
        "ledger_index": ledger,
        "engine_result": result,
        "transaction": transaction,
        "meta": meta,
    })
}

impl Tx {
    /// Its binary form: as submitted, or, for the fake's own, unsigned.
    pub(super) fn blob(&self) -> Vec<u8> {
        match self {
            Self::Ledger(tx) => tx.blob(),
            Self::IssuerSet(set) => objects::issuer_set(set, self.fee()).to_bytes(),
            Self::Clawback(clawback) => objects::clawback(clawback, self.fee()).to_bytes(),
            Self::PoolSet(set) => objects::pool_set(set, self.fee()).to_bytes(),
        }
    }
}

impl FakeLedger {
    /// The record of `tx`, applied at `index` of the current ledger, whose
    /// objects now name it as the last that changed them.
    pub(super) fn applied(
        &mut self,
        tx: &Tx,
        index: u32,
        outcome: &Outcome,
        touched: &Touched,
    ) -> Applied {
        let blob = tx.blob();
        let tx_hash = hash(&blob);
        let changes = self.changes(touched);
        let mut nodes = Vec::new();
        let mut accounts = Vec::new();
        let mut owners = Vec::new();
        for change in &changes {
            let Some((kind, node)) = affected_node(
                &change.key,
                change.before.as_ref(),
                change.after.as_ref(),
                change.last.as_ref(),
                self.thread(&change.key),
            ) else {
                continue;
            };
            nodes.push((change.key, kind, node));
            if change.before.is_none() || change.after.is_none() {
                owners.extend_from_slice(&change.owners);
            }
            for account in &change.accounts {
                if !accounts.contains(account) {
                    accounts.push(*account);
                }
            }
        }
        let threaded = self.thread_owners(&owners, &mut nodes);
        self.rethread(&changes, &threaded, (tx_hash, self.ledger));
        let mut also = alloc::vec![tx.account()];
        if let Tx::Ledger(ledger_tx) = tx
            && let Transaction::Payment(payment) = ledger_tx.as_ref()
        {
            also.push(payment.destination);
        }
        for account in also {
            if !accounts.contains(&account) {
                accounts.push(account);
            }
        }
        let meta = metadata(index, outcome.code, &nodes);
        let message = message(
            self.ledger,
            &tx_hash,
            &blob,
            &meta,
            outcome.code,
            outcome.delivered.as_ref(),
        );
        Applied {
            ledger: self.ledger,
            hash: tx_hash,
            accounts,
            blob,
            meta,
            message,
        }
    }

    /// The roots of `owners` the transaction did not otherwise change,
    /// which name it too: each is listed in `nodes` with the transaction
    /// before alone, unless none changed it before.
    fn thread_owners(
        &self,
        owners: &[AccountId],
        nodes: &mut Vec<([u8; 32], FieldId, Object)>,
    ) -> Vec<[u8; 32]> {
        let mut threaded = Vec::new();
        for owner in owners {
            let key = objects::account_key(*owner);
            if nodes.iter().any(|(changed, _, _)| *changed == key)
                || threaded.contains(&key)
                || !self.accounts.contains_key(owner)
            {
                continue;
            }
            threaded.push(key);
            let previous = self.thread(&key);
            if previous != UNTHREADED {
                let mut node = Object::entry(objects::ACCOUNT_ROOT);
                node.put(LEDGER_INDEX, &key);
                node.put(PREVIOUS_TXN_ID, &previous.0);
                node.put_u32(PREVIOUS_TXN_LGR_SEQ, previous.1);
                nodes.push((key, MODIFIED_NODE, node));
            }
        }
        threaded
    }

    /// Each object `changes` changed, and each of `threaded`, now names
    /// `thread`; a deleted one names nothing.
    fn rethread(&mut self, changes: &[Change], threaded: &[[u8; 32]], thread: Thread) {
        for change in changes {
            let kind = change.after.as_ref().and_then(Object::entry_type);
            if change.after.is_none() {
                self.threads.remove(&change.key);
            } else if kind.is_some_and(objects::is_threaded) && change.before != change.after {
                self.threads.insert(change.key, thread);
            }
        }
        for key in threaded {
            self.threads.insert(*key, thread);
        }
    }

    /// The transaction that last changed the object under `key`.
    pub(super) fn thread(&self, key: &[u8; 32]) -> Thread {
        self.threads.get(key).copied().unwrap_or(UNTHREADED)
    }

    /// What `touched` came to, object by object, each as it was and as it
    /// is, both naming the transaction that changed it before.
    fn changes(&self, touched: &Touched) -> Vec<Change> {
        let mut changes = Vec::new();
        for (id, before) in &touched.accounts {
            let key = objects::account_key(*id);
            let previous = self.thread(&key);
            let amm_before = self.amm_before(*id, touched);
            changes.push(Change {
                key,
                before: before
                    .map(|account| objects::account_root(*id, &account, amm_before, previous)),
                after: self
                    .accounts
                    .get(id)
                    .map(|account| objects::account_root(*id, account, self.amm_of(*id), previous)),
                last: None,
                owners: Vec::new(),
                accounts: alloc::vec![*id],
            });
        }
        for (line, before, last) in &touched.lines {
            let key = line_index(line);
            let previous = self.thread(&key);
            changes.push(Change {
                key,
                before: before.map(|line| objects::ripple_state(&line, previous)),
                after: (self.lines.get(line)).map(|line| objects::ripple_state(line, previous)),
                last: Some(objects::ripple_state(last, previous)),
                owners: alloc::vec![line.0, line.1],
                accounts: alloc::vec![line.0, line.1],
            });
        }
        for (owner, before, last) in &touched.signer_lists {
            let key = signer_list_index(*owner);
            let previous = self.thread(&key);
            changes.push(Change {
                key,
                before: (before.as_ref()).map(|list| objects::signer_list(list, previous)),
                after: (self.signer_lists.get(owner))
                    .map(|list| objects::signer_list(list, previous)),
                last: Some(objects::signer_list(last, previous)),
                owners: Vec::new(),
                accounts: alloc::vec![*owner],
            });
        }
        for (token, before) in &touched.pools {
            let key = objects::amm_key(*token);
            let previous = self.thread(&key);
            changes.push(Change {
                key,
                before: before.map(|pool| objects::amm(*token, &pool, previous)),
                after: (self.pools.get(token)).map(|pool| objects::amm(*token, pool, previous)),
                last: None,
                owners: alloc::vec![pool_account(*token)],
                accounts: Vec::new(),
            });
        }
        changes
    }

    /// The key of the `AMM` object of the pool whose account `account` is,
    /// if it is one's.
    pub(super) fn amm_of(&self, account: AccountId) -> Option<[u8; 32]> {
        self.pools
            .iter()
            .find(|(_, pool)| pool.account == account)
            .map(|(token, _)| objects::amm_key(*token))
    }

    /// As [`amm_of`](Self::amm_of), before what `touched` wrote.
    fn amm_before(&self, account: AccountId, touched: &Touched) -> Option<[u8; 32]> {
        let pool = (touched.pools.iter()).find(|(token, _)| pool_account(*token) == account);
        match pool {
            Some((token, before)) => before.map(|_| objects::amm_key(*token)),
            None => self.amm_of(account),
        }
    }
}
