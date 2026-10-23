//! Who signs for an account in the fake ledger
//! (spec/areas/xrpl.md#keys-in-the-fake-ledger): its master key, unless it
//! is disabled; its regular key; or signers of its signer list together,
//! each alone, once their weights reach its quorum. With the settings an
//! escrow that several keys control is set up with: `AccountSet` that
//! requires destination tags or disables the master key, `SetRegularKey`,
//! and `SignerListSet`, each with the ledger's refusals.
//!
//! Who signed is checked when a transaction is submitted, and again when
//! it applies, against the account as it is then: one whose keys may no
//! longer sign for it applies nothing and uses no sequence (`tef`).

use alloc::vec::Vec;

use serde_json::{Value, json};
use sha2::{Digest, Sha512};

use super::{Account, FakeLedger, TF_FULLY_CANONICAL_SIG, Touched, tokens};
use crate::api::{SignerList, account_flags};
use crate::codec::{
    ASF_DISABLE_MASTER, ASF_REQUIRE_DEST, AccountSet, SetRegularKey, Signer, SignerListSet,
    Signing, TF_OPTIONAL_DEST_TAG, TF_REQUIRE_DEST_TAG, check_signer_list,
};
use crate::hex_upper;
use crate::keys::AccountId;

/// A signer list's flag: it counts as one object toward its owner's
/// reserve, however many entries it has, as every list made since the
/// ledger's `MultiSignReserve` amendment does.
const LSF_ONE_OWNER_COUNT: u32 = 0x0001_0000;

/// The most objects `account_objects` returns at once, as rippled's limit.
const MAX_OBJECTS_PAGE: usize = 400;

/// A signer list's ledger index: SHA-512Half of its space (`S`), its
/// owner, and its ID, always 0.
pub(super) fn signer_list_index(owner: AccountId) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update([0x00, b'S']);
    hasher.update(owner.0);
    hasher.update(0_u32.to_be_bytes());
    let digest = hasher.finalize();
    let mut out = [0; 32];
    out.copy_from_slice(digest.get(..32).unwrap_or(&[0; 32]));
    out
}

/// A signer list's fields, as `account_objects` and the metadata write
/// them.
pub(super) fn signer_list_fields(list: &SignerList) -> Value {
    let entries: Vec<Value> = list
        .entries
        .iter()
        .map(|entry| {
            json!({ "SignerEntry": {
                "Account": entry.account.to_address(),
                "SignerWeight": entry.weight,
            }})
        })
        .collect();
    json!({
        "Flags": LSF_ONE_OWNER_COUNT,
        "OwnerNode": "0",
        "SignerEntries": entries,
        "SignerListID": 0,
        "SignerQuorum": list.quorum,
    })
}

/// The settings the fake models: whether an AccountSet requires
/// destination tags, and whether it lifts that. Only after
/// [`preflight_account_set`].
fn destination_tag_change(set: &AccountSet) -> (bool, bool) {
    (
        set.flags & TF_REQUIRE_DEST_TAG != 0 || set.set_flag == Some(ASF_REQUIRE_DEST),
        set.flags & TF_OPTIONAL_DEST_TAG != 0 || set.clear_flag == Some(ASF_REQUIRE_DEST),
    )
}

/// What an AccountSet alone may be refused for. The fake models only
/// [`ASF_REQUIRE_DEST`] and [`ASF_DISABLE_MASTER`], and refuses any other
/// setting rather than ignore it.
pub(super) fn preflight_account_set(set: &AccountSet) -> Result<(), &'static str> {
    let known = TF_REQUIRE_DEST_TAG | TF_OPTIONAL_DEST_TAG | TF_FULLY_CANONICAL_SIG;
    let modelled =
        |flag: Option<u32>| flag.is_none_or(|f| f == ASF_REQUIRE_DEST || f == ASF_DISABLE_MASTER);
    if set.flags & !known != 0 || !modelled(set.set_flag) || !modelled(set.clear_flag) {
        return Err("temINVALID_FLAG");
    }
    let (require, optional) = destination_tag_change(set);
    if (set.set_flag.is_some() && set.set_flag == set.clear_flag) || (require && optional) {
        return Err("temINVALID_FLAG");
    }
    Ok(())
}

/// What a SetRegularKey alone may be refused for.
pub(super) fn preflight_set_regular_key(set: &SetRegularKey) -> Result<(), &'static str> {
    if set.flags & !TF_FULLY_CANONICAL_SIG != 0 {
        return Err("temINVALID_FLAG");
    }
    if set.regular_key == Some(set.account) {
        return Err("temBAD_REGKEY");
    }
    Ok(())
}

/// What a SignerListSet alone may be refused for: a quorum of zero with no
/// entries deletes the list; anything else is a list the ledger takes.
pub(super) fn preflight_signer_list_set(set: &SignerListSet) -> Result<(), &'static str> {
    if set.flags & !TF_FULLY_CANONICAL_SIG != 0 {
        return Err("temINVALID_FLAG");
    }
    match (set.signer_quorum, set.signer_entries.is_empty()) {
        (0, true) => Ok(()),
        (0, false) | (_, true) => Err("temMALFORMED"),
        _ => check_signer_list(set.account, set.signer_quorum, &set.signer_entries)
            .map_err(|error| error.code()),
    }
}

/// Whether `signer`, a key's account, may sign alone for `sender`, whose
/// root is `account`.
fn single_authorized(
    account: &Account,
    sender: AccountId,
    signer: AccountId,
) -> Result<(), &'static str> {
    if signer == sender {
        if account.flags & account_flags::DISABLE_MASTER != 0 {
            return Err("tefMASTER_DISABLED");
        }
        Ok(())
    } else if account.regular_key == Some(signer) {
        Ok(())
    } else if account.regular_key.is_some() {
        Err("tefBAD_AUTH")
    } else {
        Err("tefBAD_AUTH_MASTER")
    }
}

impl FakeLedger {
    /// `account`'s signer list, if it has one.
    #[must_use]
    pub fn signer_list(&self, account: AccountId) -> Option<&SignerList> {
        self.signer_lists.get(&account)
    }

    /// `account`'s regular key, as the account of the key, if it has one.
    #[must_use]
    pub fn regular_key(&self, account: AccountId) -> Option<AccountId> {
        self.accounts.get(&account)?.regular_key
    }

    /// Whether `signing` may sign for `sender` as the ledger is now: its
    /// master key unless disabled, its regular key, or signers on its list
    /// whose weights reach its quorum, each signing with its account's
    /// master key (an account that does not exist has only that) or its
    /// regular key. The signatures themselves were checked at submission.
    pub(super) fn authorized(
        &self,
        sender: AccountId,
        signing: &Signing,
    ) -> Result<(), &'static str> {
        let account = self.accounts.get(&sender).ok_or("terNO_ACCOUNT")?;
        match signing {
            Signing::Single { public_key, .. } => {
                single_authorized(account, sender, public_key.account())
            }
            Signing::Multi(signers) => self.multi_authorized(sender, signers),
        }
    }

    fn multi_authorized(&self, sender: AccountId, signers: &[Signer]) -> Result<(), &'static str> {
        let list = self
            .signer_lists
            .get(&sender)
            .ok_or("tefNOT_MULTI_SIGNING")?;
        let mut weight: u32 = 0;
        for signer in signers {
            let entry = list
                .entries
                .iter()
                .find(|entry| entry.account == signer.account)
                .ok_or("tefBAD_SIGNATURE")?;
            let key = signer.signing_pub_key.account();
            let root = self.accounts.get(&signer.account);
            if key == signer.account {
                if root.is_some_and(|root| root.flags & account_flags::DISABLE_MASTER != 0) {
                    return Err("tefMASTER_DISABLED");
                }
            } else if root.and_then(|root| root.regular_key) != Some(key) {
                return Err("tefBAD_SIGNATURE");
            }
            weight = weight.saturating_add(u32::from(entry.weight));
        }
        if weight < list.quorum {
            return Err("tefBAD_QUORUM");
        }
        Ok(())
    }

    /// An AccountSet, after its fee. Disabling the master key needs the
    /// master key's own signature, and another way to sign.
    pub(super) fn account_set(&mut self, touched: &mut Touched, set: &AccountSet) -> &'static str {
        let Some(mut account) = self.accounts.get(&set.account).copied() else {
            return "tefINTERNAL";
        };
        if set.set_flag == Some(ASF_DISABLE_MASTER) {
            let by_master = matches!(
                &set.signing,
                Signing::Single { public_key, .. } if public_key.account() == set.account
            );
            if !by_master {
                return "tecNEED_MASTER_KEY";
            }
            if account.regular_key.is_none() && !self.signer_lists.contains_key(&set.account) {
                return "tecNO_ALTERNATIVE_KEY";
            }
            account.flags |= account_flags::DISABLE_MASTER;
        }
        if set.clear_flag == Some(ASF_DISABLE_MASTER) {
            account.flags &= !account_flags::DISABLE_MASTER;
        }
        let (require, optional) = destination_tag_change(set);
        if require {
            account.flags |= account_flags::REQUIRE_DEST_TAG;
        }
        if optional {
            account.flags &= !account_flags::REQUIRE_DEST_TAG;
        }
        self.put_account(touched, set.account, account);
        "tesSUCCESS"
    }

    /// A SetRegularKey, after its fee: removing the key needs another way
    /// to sign.
    pub(super) fn set_regular_key(
        &mut self,
        touched: &mut Touched,
        set: &SetRegularKey,
    ) -> &'static str {
        let Some(mut account) = self.accounts.get(&set.account).copied() else {
            return "tefINTERNAL";
        };
        if set.regular_key.is_none()
            && account.flags & account_flags::DISABLE_MASTER != 0
            && !self.signer_lists.contains_key(&set.account)
        {
            return "tecNO_ALTERNATIVE_KEY";
        }
        account.regular_key = set.regular_key;
        self.put_account(touched, set.account, account);
        "tesSUCCESS"
    }

    /// A SignerListSet, after its fee: replaces the list, sorted by
    /// account, or deletes it. A new list holds one owner reserve, which
    /// `prior`, the sender's XRP before the fee, must cover; deleting the
    /// list needs another way to sign.
    pub(super) fn signer_list_set(
        &mut self,
        touched: &mut Touched,
        set: &SignerListSet,
        prior: u64,
    ) -> &'static str {
        let Some(account) = self.accounts.get(&set.account).copied() else {
            return "tefINTERNAL";
        };
        let had = self.signer_lists.contains_key(&set.account);
        if set.signer_quorum == 0 {
            if account.flags & account_flags::DISABLE_MASTER != 0 && account.regular_key.is_none() {
                return "tecNO_ALTERNATIVE_KEY";
            }
            self.store_signer_list(touched, set.account, None);
            return "tesSUCCESS";
        }
        let owned = account.owner_count.saturating_sub(u32::from(had));
        let reserve = self.reserve.0.saturating_add(
            self.reserve
                .1
                .saturating_mul(u64::from(owned).saturating_add(1)),
        );
        if prior < reserve {
            return "tecINSUFFICIENT_RESERVE";
        }
        let mut entries = set.signer_entries.clone();
        entries.sort_unstable_by_key(|entry| entry.account);
        let list = SignerList {
            quorum: set.signer_quorum,
            entries,
        };
        self.store_signer_list(touched, set.account, Some(list));
        "tesSUCCESS"
    }

    /// Writes `owner`'s signer list, or deletes it, with its owner count.
    fn store_signer_list(
        &mut self,
        touched: &mut Touched,
        owner: AccountId,
        list: Option<SignerList>,
    ) {
        let before = self.signer_lists.get(&owner).cloned();
        if let Some(mut account) = self.accounts.get(&owner).copied() {
            match (&before, &list) {
                (None, Some(_)) => account.owner_count = account.owner_count.saturating_add(1),
                (Some(_), None) => account.owner_count = account.owner_count.saturating_sub(1),
                _ => {}
            }
            self.put_account(touched, owner, account);
        }
        if let Some(last) = list.clone().or_else(|| before.clone()) {
            touched.signer_list(owner, before, last);
        }
        match list {
            Some(list) => self.signer_lists.insert(owner, list),
            None => self.signer_lists.remove(&owner),
        };
    }

    /// `account_objects`: the account's signer list, then its trust lines,
    /// as of the last validated ledger, a page at a time.
    pub(super) fn account_objects(&self, request: &Value) -> Result<Value, Value> {
        let id = Self::account_of(request)?;
        if !self.accounts.contains_key(&id) {
            return Err(
                json!({ "error": "actNotFound", "ledger_index": self.ledger, "validated": true }),
            );
        }
        let mut objects = Vec::new();
        if let Some(list) = self.signer_lists.get(&id) {
            objects.push(entry(
                signer_list_fields(list),
                "SignerList",
                &signer_list_index(id),
            ));
        }
        for (key, line) in &self.lines {
            if key.0 == id || key.1 == id {
                let fields = super::meta::line_fields(line);
                objects.push(entry(fields, "RippleState", &tokens::line_index(key)));
            }
        }
        let limit = request
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(200)
            .clamp(1, MAX_OBJECTS_PAGE);
        let skip = request
            .get("marker")
            .and_then(Value::as_u64)
            .and_then(|marker| usize::try_from(marker).ok())
            .unwrap_or(0);
        let total = objects.len();
        let page: Vec<Value> = objects.into_iter().skip(skip).take(limit).collect();
        let next = skip.saturating_add(page.len());
        let mut result = json!({
            "account": id.to_address(),
            "account_objects": page,
            "ledger_index": self.ledger,
            "validated": true,
        });
        if next < total
            && let Some(object) = result.as_object_mut()
        {
            object.insert("marker".into(), next.into());
        }
        Ok(result)
    }
}

/// A ledger entry's fields with its type and index, as `account_objects`
/// writes it.
fn entry(mut fields: Value, kind: &str, index: &[u8; 32]) -> Value {
    if let Some(object) = fields.as_object_mut() {
        object.insert("LedgerEntryType".into(), kind.into());
        object.insert("index".into(), hex_upper(index).into());
    }
    fields
}
