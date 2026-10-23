//! What the fake ledger writes about a transaction it applied: the
//! transaction as JSON, and its metadata, with each `AccountRoot`,
//! `RippleState` and `SignerList` it created, modified or deleted, as
//! rippled writes them.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

use super::signers::signer_list_fields;
use super::tokens::LineKey;
use super::{Account, FakeLedger, Outcome, Tx, token_json};
use crate::api::{SignerList, TrustLine};
use crate::codec::{Amount, Memo, Signing, Transact, Transaction, hash};
use crate::hex_upper;
use crate::keys::AccountId;

/// The placeholder issuer of a line's balance, `ACCOUNT_ONE`: a balance
/// is between the line's two accounts, whichever way it goes.
const ACCOUNT_ONE: AccountId =
    AccountId([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

/// A transaction in a validated ledger, with its metadata.
#[derive(Clone, Debug)]
pub(super) struct Applied {
    pub ledger: u32,
    pub hash: [u8; 32],
    /// The accounts it touched, whose streams and histories carry it.
    pub accounts: Vec<AccountId>,
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
}

/// An account's fields, as `account_info` and the metadata write them.
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

/// A line's fields: the balance from the low account's side, its flags and
/// each side's limit, issued by that side.
pub(super) fn line_fields(line: &TrustLine) -> Value {
    let limit = |issuer: AccountId, value: crate::codec::TokenValue| {
        token_json(&crate::codec::TokenAmount {
            value,
            currency: line.currency,
            issuer,
        })
    };
    json!({
        "Balance": {
            "currency": line.currency.to_string(),
            "issuer": ACCOUNT_ONE.to_address(),
            "value": line.balance.to_string(),
        },
        "Flags": line.flags,
        "HighLimit": limit(line.high, line.high_limit),
        "LowLimit": limit(line.low, line.low_limit),
    })
}

/// The fields of `before` that differ in `after`, as they were.
fn previous(before: &Value, after: &Value) -> Value {
    let mut changed = Map::new();
    if let Some(before) = before.as_object() {
        for (key, value) in before {
            if after.get(key) != Some(value) {
                changed.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(changed)
}

/// An affected node: `kind` of `entry`, with its fields.
fn node(kind: &str, entry: &str, fields: Vec<(&str, Value)>) -> Value {
    let mut inner = Map::new();
    inner.insert("LedgerEntryType".into(), entry.into());
    for (key, value) in fields {
        if value.as_object().is_none_or(|object| !object.is_empty()) {
            inner.insert(key.into(), value);
        }
    }
    let mut node = Map::new();
    node.insert(String::from(kind), Value::Object(inner));
    Value::Object(node)
}

/// The node for an entry `before` and after, if it changed.
fn changed_node(
    entry: &str,
    before: Option<Value>,
    after: Option<Value>,
    last: Value,
) -> Option<Value> {
    match (before, after) {
        (None, Some(after)) => Some(node(
            "CreatedNode",
            entry,
            alloc::vec![("NewFields", after)],
        )),
        (Some(before), Some(after)) if before != after => Some(node(
            "ModifiedNode",
            entry,
            alloc::vec![
                ("FinalFields", after.clone()),
                ("PreviousFields", previous(&before, &after))
            ],
        )),
        (Some(before), None) => Some(node(
            "DeletedNode",
            entry,
            alloc::vec![
                ("FinalFields", last.clone()),
                ("PreviousFields", previous(&before, &last))
            ],
        )),
        _ => None,
    }
}

/// A Payment's or TrustSet's memos, as the ledger writes them.
fn memos_json(memos: &[Memo]) -> Value {
    memos
        .iter()
        .map(|memo| {
            json!({ "Memo": {
                "MemoType": hex_upper(&memo.memo_type),
                "MemoData": hex_upper(&memo.memo_data),
            }})
        })
        .collect::<Vec<_>>()
        .into()
}

fn amount_json(amount: &Amount) -> Value {
    match amount {
        Amount::Xrp(drops) => drops.to_string().into(),
        Amount::Token(token) => token_json(token),
    }
}

/// The transaction's own fields, by kind.
fn transaction_json(tx: &Tx, tx_hash: &[u8; 32]) -> Value {
    let mut fields = match tx {
        Tx::PoolSet(set) => set.json().as_object().cloned().unwrap_or_default(),
        _ => Map::new(),
    };
    let mut put = |key: &str, value: Value| {
        fields.insert(key.into(), value);
    };
    put("Account", tx.account().to_address().into());
    put("Fee", tx.fee().to_string().into());
    put("Sequence", tx.sequence().into());
    if let Some(last) = tx.last_ledger_sequence() {
        put("LastLedgerSequence", last.into());
    }
    put("hash", hex_upper(tx_hash).into());
    match tx {
        Tx::Ledger(tx) => ledger_json(tx, &mut put),
        Tx::IssuerSet(set) => {
            put("TransactionType", "AccountSet".into());
            put("Flags", 0.into());
            if let Some(flag) = set.set_flag {
                put("SetFlag", flag.into());
            }
            if let Some(flag) = set.clear_flag {
                put("ClearFlag", flag.into());
            }
            if let Some(rate) = set.transfer_rate {
                put("TransferRate", rate.into());
            }
        }
        Tx::Clawback(clawback) => {
            put("TransactionType", "Clawback".into());
            put("Amount", token_json(&clawback.amount));
            put("Flags", 0.into());
        }
        // Its own fields came first.
        Tx::PoolSet(_) => {}
    }
    Value::Object(fields)
}

/// The fields of a transaction of a kind the codec reads: its kind's, its
/// flags and memos, and who signed it, if anyone did.
fn ledger_json(tx: &Transaction, put: &mut impl FnMut(&str, Value)) {
    let optional = |put: &mut dyn FnMut(&str, Value), key: &str, value: Option<Value>| {
        if let Some(value) = value {
            put(key, value);
        }
    };
    let (kind, flags, memos) = match tx {
        Transaction::Payment(payment) => {
            put("Destination", payment.destination.to_address().into());
            optional(
                put,
                "DestinationTag",
                payment.destination_tag.map(Value::from),
            );
            put("Amount", amount_json(&payment.amount));
            optional(put, "SendMax", payment.send_max.as_ref().map(amount_json));
            ("Payment", payment.flags, &payment.memos)
        }
        Transaction::TrustSet(set) => {
            put("LimitAmount", token_json(&set.limit));
            ("TrustSet", set.flags, &set.memos)
        }
        Transaction::AccountSet(set) => {
            optional(put, "SetFlag", set.set_flag.map(Value::from));
            optional(put, "ClearFlag", set.clear_flag.map(Value::from));
            ("AccountSet", set.flags, &set.memos)
        }
        Transaction::SetRegularKey(set) => {
            let key = set.regular_key.map(|key| key.to_address().into());
            optional(put, "RegularKey", key);
            ("SetRegularKey", set.flags, &set.memos)
        }
        Transaction::SignerListSet(set) => {
            put("SignerQuorum", set.signer_quorum.into());
            if !set.signer_entries.is_empty() {
                let list = SignerList {
                    quorum: set.signer_quorum,
                    entries: set.signer_entries.clone(),
                };
                let fields = signer_list_fields(&list);
                put(
                    "SignerEntries",
                    fields.get("SignerEntries").cloned().unwrap_or_default(),
                );
            }
            ("SignerListSet", set.flags, &set.memos)
        }
    };
    put("TransactionType", kind.into());
    put("Flags", flags.into());
    if !memos.is_empty() {
        put("Memos", memos_json(memos));
    }
    signing_json(tx.signing(), put);
}

/// Who signed, as the ledger writes it: one key and its signature, or an
/// empty key and the signers. Nothing for the fake's own.
fn signing_json(signing: &Signing, put: &mut impl FnMut(&str, Value)) {
    match signing {
        Signing::Single {
            public_key,
            signature: Some(signature),
        } => {
            put("SigningPubKey", hex_upper(public_key.as_bytes()).into());
            put("TxnSignature", hex_upper(signature.as_bytes()).into());
        }
        Signing::Multi(signers) if !signers.is_empty() => {
            put("SigningPubKey", "".into());
            let signers: Vec<Value> = signers
                .iter()
                .map(|signer| {
                    json!({ "Signer": {
                        "Account": signer.account.to_address(),
                        "SigningPubKey": hex_upper(signer.signing_pub_key.as_bytes()),
                        "TxnSignature": hex_upper(signer.signature.as_bytes()),
                    }})
                })
                .collect();
            put("Signers", signers.into());
        }
        Signing::Single { .. } | Signing::Multi(_) => {}
    }
}

impl FakeLedger {
    /// The record of `tx`, applied at `index` of the current ledger.
    pub(super) fn applied(
        &self,
        tx: &Tx,
        index: u32,
        outcome: &Outcome,
        touched: &Touched,
    ) -> Applied {
        // The hash of the transaction as submitted. The fake's own are not
        // signed: their ledger and index make them unique.
        let tx_hash = tx.signed_blob().map_or_else(
            || {
                let mut seed = Vec::new();
                seed.extend_from_slice(&self.ledger.to_be_bytes());
                seed.extend_from_slice(&index.to_be_bytes());
                hash(&seed)
            },
            |blob| hash(&blob),
        );
        let (nodes, mut accounts) = self.affected(touched);
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
        let mut meta = json!({
            "TransactionIndex": index,
            "TransactionResult": outcome.code,
            "AffectedNodes": nodes,
        });
        if let (Some(delivered), Some(object)) = (&outcome.delivered, meta.as_object_mut()) {
            object.insert("delivered_amount".into(), amount_json(delivered));
        }
        Applied {
            ledger: self.ledger,
            hash: tx_hash,
            accounts,
            message: json!({
                "type": "transaction",
                "validated": true,
                "ledger_index": self.ledger,
                "engine_result": outcome.code,
                "transaction": transaction_json(tx, &tx_hash),
                "meta": meta,
            }),
        }
    }

    /// The nodes a transaction changed, and the accounts they belong to:
    /// each account written, and both of each line's.
    fn affected(&self, touched: &Touched) -> (Vec<Value>, Vec<AccountId>) {
        let mut nodes = Vec::new();
        let mut accounts = Vec::new();
        for (id, before) in &touched.accounts {
            let fields = |account: &Account| account_fields(*id, account);
            let after = self.accounts.get(id).map(fields);
            let last = after.clone().unwrap_or(Value::Null);
            if let Some(node) =
                changed_node("AccountRoot", before.as_ref().map(fields), after, last)
            {
                nodes.push(node);
                accounts.push(*id);
            }
        }
        for (key, before, last) in &touched.lines {
            let after = self.lines.get(key).map(line_fields);
            if let Some(node) = changed_node(
                "RippleState",
                before.as_ref().map(line_fields),
                after,
                line_fields(last),
            ) {
                nodes.push(node);
                for account in [key.0, key.1] {
                    if !accounts.contains(&account) {
                        accounts.push(account);
                    }
                }
            }
        }
        for (owner, before, last) in &touched.signer_lists {
            let after = self.signer_lists.get(owner).map(signer_list_fields);
            if let Some(node) = changed_node(
                "SignerList",
                before.as_ref().map(signer_list_fields),
                after,
                signer_list_fields(last),
            ) {
                nodes.push(node);
                if !accounts.contains(owner) {
                    accounts.push(*owner);
                }
            }
        }
        (nodes, accounts)
    }
}
