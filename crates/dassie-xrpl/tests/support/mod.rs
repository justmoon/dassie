//! The node's first XRPL testnet account and the `xrpl-testnet` scheme on
//! it, wired together as the node wires them: after each input to the
//! account, the scheme is told what it learnt, and what both ask is handed
//! back in order. It speaks the interface the scheme had when it owned its
//! account, so the tests of what the scheme books read as they always did.
//!
//! Its state is the one the scheme wrote then: a single JSON object of the
//! account's and the scheme's fields, which it encodes into what each
//! writes (spec/areas/xrpl.md#state-and-restarts), and decodes again.

#![allow(dead_code, clippy::unwrap_used, clippy::panic)]

use dassie_kit::sim::SeededRng;
use dassie_ledger::Name;
use dassie_settlement::{
    AssetEffect, ExternalAsset, LedgerInput, LedgerStatus, PreparedSettlement, ReconcileCause,
    SchemeError, SettlementScheme,
};
use dassie_xrpl::AccountId;
use dassie_xrpl::account::{self, Config, XrplAccount};
use serde_json::{Map, Value};

/// What the scheme and its account ask, in the order they ask it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    IncomingSettlement {
        peer: Name,
        amount: u64,
        id: Option<Name>,
    },
    Finalize {
        settlement: Name,
    },
    Cancel {
        settlement: Name,
    },
    Deposit {
        amount: u64,
    },
    Withdrawal {
        amount: u64,
    },
    OwnerPaid {
        id: Name,
        applied: bool,
    },
    SendMessage {
        peer: Name,
        message: Vec<u8>,
    },
    Fee {
        amount: u64,
        id: Name,
    },
    Reconcile {
        amount: u64,
        gain: bool,
        cause: ReconcileCause,
    },
    LedgerSend {
        message: Vec<u8>,
    },
    Reconnect,
    Fund {
        url: String,
        body: Vec<u8>,
    },
    Fetch {
        url: String,
    },
}

impl From<dassie_settlement::Effect> for Effect {
    fn from(effect: dassie_settlement::Effect) -> Self {
        use dassie_settlement::Effect as E;
        match effect {
            E::IncomingSettlement { peer, amount, id } => {
                Self::IncomingSettlement { peer, amount, id }
            }
            E::Finalize { settlement } => Self::Finalize { settlement },
            E::Cancel { settlement } => Self::Cancel { settlement },
            E::Deposit { amount } => Self::Deposit { amount },
            E::Withdrawal { amount } => Self::Withdrawal { amount },
            E::OwnerPaid { id, applied } => Self::OwnerPaid { id, applied },
            E::SendMessage { peer, message } => Self::SendMessage { peer, message },
            E::Fee { amount, id } => Self::Fee { amount, id },
            E::Reconcile {
                amount,
                gain,
                cause,
            } => Self::Reconcile {
                amount,
                gain,
                cause,
            },
            E::PeerLoss { .. } => panic!("an XRPL scheme books no peer losses"),
        }
    }
}

impl From<AssetEffect> for Effect {
    fn from(effect: AssetEffect) -> Self {
        match effect {
            AssetEffect::Send { message } => Self::LedgerSend { message },
            AssetEffect::Reconnect => Self::Reconnect,
            AssetEffect::Fund { url, body } => Self::Fund { url, body },
            AssetEffect::Fetch { url, .. } => Self::Fetch { url },
            AssetEffect::SendTo { .. } => panic!("an XRPL account names no endpoints"),
        }
    }
}

/// The account and its scheme.
pub struct XrplScheme {
    pub account: XrplAccount,
    pub scheme: dassie_xrpl::XrplScheme,
}

impl XrplScheme {
    /// The scheme and its account, from the joined state.
    pub fn new(
        config: &str,
        entropy: &[u8; 32],
        node: Name,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let (account_state, scheme_state) = split(state).ok_or(SchemeError::InvalidData)?;
        let schemes = [dassie_ledger::name!(dassie_xrpl::scheme::SCHEME_ID)];
        // It trusts the fake ledger's validators.
        let config = dassie_xrpl::fake::with_publishers(config, &dassie_xrpl::network::TESTNET);
        let account = XrplAccount::new(account::FIRST, &schemes, &config, entropy, &account_state)?;
        let scheme = dassie_xrpl::XrplScheme::new(&account, node, checkpoint, &scheme_state)?;
        let mut both = Self { account, scheme };
        let mut effects = Vec::new();
        both.tell(0, &mut effects);
        assert!(effects.is_empty(), "{effects:?}");
        Ok(both)
    }

    fn tell(&mut self, now_ms: u64, effects: &mut Vec<Effect>) {
        let mut booked = Vec::new();
        self.scheme.on_asset(now_ms, &mut self.account, &mut booked);
        self.account.told();
        effects.extend(booked.into_iter().map(Effect::from));
        self.drain(effects);
    }

    fn drain(&mut self, effects: &mut Vec<Effect>) {
        let mut asked = Vec::new();
        self.account.take_effects(&mut asked);
        effects.extend(asked.into_iter().map(Effect::from));
    }

    pub fn account(&self) -> AccountId {
        self.account.account()
    }

    pub fn config(&self) -> &Config {
        self.account.config()
    }

    pub fn on_ledger(&mut self, now_ms: u64, input: LedgerInput<'_>, effects: &mut Vec<Effect>) {
        self.account.on_ledger(now_ms, input);
        self.tell(now_ms, effects);
    }

    pub fn deadline(&self) -> Option<u64> {
        self.account.deadline()
    }

    pub fn on_timer(&mut self, now_ms: u64, effects: &mut Vec<Effect>) {
        self.account.on_timer(now_ms);
        self.tell(now_ms, effects);
    }

    /// As the node shows it: the account's, until it is synced.
    pub fn status(&self) -> LedgerStatus {
        match self.account.status() {
            LedgerStatus::Synced => self.scheme.status(),
            other => other,
        }
    }

    pub fn balance(&self) -> u64 {
        self.scheme.balance()
    }

    pub fn reserve(&self) -> u64 {
        self.scheme.reserve()
    }

    pub fn has_synced(&self) -> bool {
        self.scheme.has_synced()
    }

    pub fn deposit_address(&self) -> Option<String> {
        self.account.address()
    }

    pub fn has_faucet(&self) -> bool {
        self.account.has_faucet()
    }

    pub fn ask_faucet(&mut self, effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        let asked = self.account.ask_faucet();
        self.drain(effects);
        asked
    }

    pub fn peering_info(&self, out: &mut Vec<u8>) {
        self.scheme.peering_info(out);
    }

    pub fn create_peering_request(
        &mut self,
        peer: Name,
        info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        self.scheme.create_peering_request(peer, info, out)
    }

    pub fn accept_peering_request(
        &mut self,
        peer: Name,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError> {
        self.scheme.accept_peering_request(peer, data, response)
    }

    pub fn finalize_peering_request(
        &mut self,
        peer: Name,
        info: &[u8],
        data: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        self.scheme.finalize_peering_request(peer, info, data)
    }

    pub fn on_peered(&mut self, peer: Name, state: &[u8]) {
        self.scheme.on_peered(peer, state);
    }

    pub fn on_closed(&mut self, peer: Name) {
        self.scheme.on_closed(peer);
    }

    pub fn take_entries_from(&mut self, minimum: u64) {
        self.scheme.take_entries_from(minimum);
    }

    pub fn entries_held(&self) -> u64 {
        self.scheme.entries_held()
    }

    pub fn entry(&self, peer: Name, data: &[u8]) -> dassie_settlement::EntryPayment {
        self.scheme.entry(peer, data)
    }

    pub fn take_entry(&mut self, peer: Name, effects: &mut Vec<Effect>) {
        let mut booked = Vec::new();
        self.scheme.take_entry(peer, &mut booked);
        effects.extend(booked.into_iter().map(Effect::from));
    }

    pub fn prepare_settlement(
        &mut self,
        rng: &mut SeededRng,
        peer: Name,
        amount: u64,
        state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        self.scheme
            .prepare_settlement(rng, Some(&mut self.account), peer, amount, state)
    }

    pub fn execute_settlement(&mut self, settlement: Name, effects: &mut Vec<Effect>) {
        let mut booked = Vec::new();
        self.scheme
            .execute_settlement(Some(&mut self.account), settlement, &mut booked);
        effects.extend(booked.into_iter().map(Effect::from));
        self.drain(effects);
    }

    pub fn pay_owner(
        &mut self,
        payment: &dassie_settlement::OwnerPayment,
        effects: &mut Vec<Effect>,
    ) -> Result<Name, SchemeError> {
        let paid = self.scheme.pay_owner(Some(&mut self.account), payment);
        self.drain(effects);
        paid
    }

    pub fn owner_payment(&self, id: Name) -> dassie_settlement::OwnerPaymentState {
        self.scheme.owner_payment(id)
    }

    pub fn discard_settlement(&mut self, settlement: Name) {
        self.scheme
            .discard_settlement(Some(&mut self.account), settlement);
    }

    pub fn reconcile(&mut self, pending: &[(Name, u64)], effects: &mut Vec<Effect>) {
        let mut booked = Vec::new();
        self.scheme
            .reconcile(Some(&mut self.account), pending, &mut booked);
        effects.extend(booked.into_iter().map(Effect::from));
        self.drain(effects);
    }

    pub fn handle_settlement(
        &mut self,
        peer: Name,
        amount: u64,
        data: &[u8],
        peer_state: &[u8],
        effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        let mut booked = Vec::new();
        let handled = self
            .scheme
            .handle_settlement(peer, amount, data, peer_state, &mut booked);
        effects.extend(booked.into_iter().map(Effect::from));
        handled
    }

    pub fn handle_message(&mut self, peer: Name, message: &[u8], effects: &mut Vec<Effect>) {
        let mut booked = Vec::new();
        self.scheme.handle_message(peer, message, &mut booked);
        effects.extend(booked.into_iter().map(Effect::from));
    }

    /// The joined state.
    pub fn state(&self) -> Vec<u8> {
        join(&self.account.state(), &self.scheme.state())
    }

    /// The scheme's state version: the node writes its state when it
    /// changes, or the balance does.
    pub fn state_version(&self) -> u64 {
        self.scheme.state_version()
    }

    pub fn check_invariants(&self) {
        self.account.check_invariants();
        self.scheme.check_invariants();
    }
}

/// The joined state, split into what the account and the scheme write
/// (version 1, spec/areas/xrpl.md#state-and-restarts). A JSON object of
/// their fields, as the tests write and change it, is encoded as each
/// writes its own, with what a test leaves out as a fresh account or
/// scheme has it; anything else is taken as the two states joined
/// ([`join_bytes`]), so that any bytes reach the account's and the
/// scheme's own readers. What neither could have written, they refuse
/// themselves.
fn split(state: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if state.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    let Ok(Value::Object(joined)) = serde_json::from_slice::<Value>(state) else {
        let (length, rest) = state.split_first_chunk::<4>()?;
        let length = usize::try_from(u32::from_be_bytes(*length)).ok()?;
        let (account, scheme) = rest.split_at_checked(length)?;
        return Some((account.to_vec(), scheme.to_vec()));
    };
    Some((encode_account(&joined)?, encode_scheme(&joined)?))
}

/// The two states as raw bytes, joined: the account's length, the
/// account's and the scheme's.
pub fn join_bytes(account: &[u8], scheme: &[u8]) -> Vec<u8> {
    let mut joined = u32::try_from(account.len()).unwrap().to_be_bytes().to_vec();
    joined.extend_from_slice(account);
    joined.extend_from_slice(scheme);
    joined
}

/// The account's and the scheme's states, as one JSON object of their
/// fields; what the account keeps after what it signed (its anchor and
/// UNL) as `tail`, in hex.
fn join(account: &[u8], scheme: &[u8]) -> Vec<u8> {
    let mut joined = decode_scheme(scheme);
    joined.append(&mut decode_account(account));
    serde_json::to_vec(&joined).unwrap()
}

/// Bytes being written, as the crate writes its states.
struct Out(Vec<u8>);

impl Out {
    fn new() -> Self {
        Self(vec![1])
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn bool(&mut self, value: bool) {
        self.u8(if value { 0xff } else { 0 });
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn octets(&mut self, bytes: &[u8]) {
        self.u32(u32::try_from(bytes.len()).unwrap());
        self.bytes(bytes);
    }

    fn name(&mut self, name: &str) {
        self.u8(u8::try_from(name.len()).ok().unwrap_or(u8::MAX));
        self.bytes(name.as_bytes());
    }
}

/// Bytes being read: each `None` is the end of the input.
struct In<'a>(&'a [u8]);

impl<'a> In<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }

    fn octets(&mut self) -> Option<&'a [u8]> {
        let length = usize::try_from(self.u32()?).ok()?;
        self.take(length)
    }

    fn name(&mut self) -> Option<String> {
        let length = usize::from(self.u8()?);
        Some(String::from_utf8_lossy(self.take(length)?).into_owned())
    }

    fn hex(&mut self, count: usize) -> Option<Value> {
        Some(dassie_xrpl::hex_upper(self.take(count)?).into())
    }
}

fn number(value: Option<&Value>) -> Option<u64> {
    value?.as_u64()
}

fn small(value: Option<&Value>) -> Option<u32> {
    u32::try_from(number(value)?).ok()
}

fn hash(value: Option<&Value>) -> Option<[u8; 32]> {
    dassie_xrpl::from_hex(value?.as_str()?)?.try_into().ok()
}

fn list<'a>(state: &'a Map<String, Value>, key: &str) -> Option<&'a [Value]> {
    match state.get(key) {
        None => Some(&[]),
        Some(value) => Some(value.as_array()?.as_slice()),
    }
}

/// What a transaction is, as the account writes it.
fn kind(entry: &Value, out: &mut Out) -> Option<bool> {
    let trust_set = entry
        .get("trust_set")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match (entry.get("token"), trust_set) {
        (_, true) => out.u8(2),
        (Some(token), false) => {
            out.u8(1);
            out.u64(dassie_xrpl::amount::TokenValue::parse(token.as_str()?)?.to_bits());
        }
        (None, false) => out.u8(0),
    }
    Some(trust_set)
}

fn encode_account(state: &Map<String, Value>) -> Option<Vec<u8>> {
    let mut out = Out::new();
    out.u32(small(state.get("sequence"))?);
    out.u64(number(state.get("write_offs")).unwrap_or(0));
    let in_flight = state.get("in_flight")?.as_array()?;
    out.u32(u32::try_from(in_flight.len()).ok()?);
    for entry in in_flight {
        let scheme = entry.get("scheme").and_then(Value::as_str);
        out.name(scheme.unwrap_or(dassie_xrpl::scheme::SCHEME_ID));
        if kind(entry, &mut out)? {
            out.u64(number(entry.get("reserve")).unwrap_or(0));
        }
        out.octets(&dassie_xrpl::from_hex(entry.get("blob")?.as_str()?)?);
        out.u64(number(entry.get("drops"))?);
        out.u64(number(entry.get("fee"))?);
        out.u32(small(entry.get("sequence"))?);
        out.u32(small(entry.get("last_ledger"))?);
        out.u32(small(entry.get("submitted_at"))?);
        let executed = entry.get("executed").map_or(Some(true), Value::as_bool)?;
        out.bool(executed);
    }
    let never_sent = list(state, "never_sent")?;
    out.u32(u32::try_from(never_sent.len()).ok()?);
    for entry in never_sent {
        out.name(entry.get("scheme")?.as_str()?);
        out.bytes(&hash(entry.get("hash"))?);
    }
    let parked = list(state, "parked")?;
    out.u32(u32::try_from(parked.len()).ok()?);
    for entry in parked {
        out.name(entry.get("scheme")?.as_str()?);
        kind(entry, &mut out)?;
        out.bytes(&hash(entry.get("hash"))?);
        out.u64(number(entry.get("drops"))?);
    }
    if let Some(tail) = state.get("tail") {
        out.bytes(&dassie_xrpl::from_hex(tail.as_str()?)?);
        return Some(out.0);
    }
    // No anchor, written at 0, and a UNL with nothing of its publishers.
    out.bool(false);
    out.u64(0);
    out.u32(0);
    out.u32(0);
    out.bool(false);
    Some(out.0)
}

fn decode_account(state: &[u8]) -> Map<String, Value> {
    let mut input = In(state);
    let mut joined = Map::new();
    assert_eq!(input.u8(), Some(1), "version 1");
    let mut read = || -> Option<()> {
        joined.insert("sequence".into(), input.u32()?.into());
        joined.insert("write_offs".into(), input.u64()?.into());
        let mut in_flight = Vec::new();
        for _ in 0..input.u32()? {
            let mut entry = Map::new();
            entry.insert("scheme".into(), input.name()?.into());
            match input.u8()? {
                1 => {
                    let bits = input.u64()?;
                    let token = dassie_xrpl::amount::TokenValue::from_bits(bits)?;
                    entry.insert("token".into(), token.to_string().into());
                }
                2 => {
                    entry.insert("trust_set".into(), true.into());
                    entry.insert("reserve".into(), input.u64()?.into());
                }
                _ => {}
            }
            entry.insert(
                "blob".into(),
                dassie_xrpl::hex_upper(input.octets()?).into(),
            );
            entry.insert("drops".into(), input.u64()?.into());
            entry.insert("fee".into(), input.u64()?.into());
            entry.insert("sequence".into(), input.u32()?.into());
            entry.insert("last_ledger".into(), input.u32()?.into());
            entry.insert("submitted_at".into(), input.u32()?.into());
            entry.insert("executed".into(), input.bool()?.into());
            in_flight.push(Value::Object(entry));
        }
        joined.insert("in_flight".into(), in_flight.into());
        let mut never_sent = Vec::new();
        for _ in 0..input.u32()? {
            let scheme = input.name()?;
            never_sent.push(serde_json::json!({ "scheme": scheme, "hash": input.hex(32)? }));
        }
        joined.insert("never_sent".into(), never_sent.into());
        let mut parked = Vec::new();
        for _ in 0..input.u32()? {
            let mut entry = Map::new();
            entry.insert("scheme".into(), input.name()?.into());
            match input.u8()? {
                1 => {
                    let token = dassie_xrpl::amount::TokenValue::from_bits(input.u64()?)?;
                    entry.insert("token".into(), token.to_string().into());
                }
                2 => {
                    entry.insert("trust_set".into(), true.into());
                }
                _ => {}
            }
            entry.insert("hash".into(), input.hex(32)?);
            entry.insert("drops".into(), input.u64()?.into());
            parked.push(Value::Object(entry));
        }
        joined.insert("parked".into(), parked.into());
        joined.insert("tail".into(), dassie_xrpl::hex_upper(input.0).into());
        Some(())
    };
    assert!(read().is_some(), "a state the account wrote");
    joined
}

fn encode_scheme(state: &Map<String, Value>) -> Option<Vec<u8>> {
    let mut out = Out::new();
    match state.get("cursor") {
        None | Some(Value::Null) => out.bool(false),
        Some(cursor) => {
            out.bool(true);
            out.u32(small(cursor.get(0))?);
            out.u32(small(cursor.get(1))?);
        }
    }
    out.u64(number(state.get("write_offs")).unwrap_or(0));
    let peers = match state.get("peers") {
        None => &Map::new(),
        Some(peers) => peers.as_object()?,
    };
    out.u32(u32::try_from(peers.len()).ok()?);
    for (peer, address) in peers {
        out.name(peer);
        out.bytes(&AccountId::parse(address.as_str()?)?.0);
    }
    let recovering = list(state, "recovering")?;
    let unread = list(state, "unread")?;
    out.u32(u32::try_from(recovering.len()).ok()?);
    for entry in recovering {
        out.bytes(&hash(Some(entry))?);
        out.bool(unread.contains(entry));
    }
    let entries = list(state, "entries")?;
    out.u32(u32::try_from(entries.len()).ok()?);
    for entry in entries {
        out.name(entry.get(0)?.as_str()?);
        out.bytes(&AccountId::parse(entry.get(1)?.as_str()?)?.0);
        out.u64(number(entry.get(2))?);
        out.bytes(&hash(entry.get(3))?);
        out.u32(small(entry.get(4))?);
    }
    match state.get("owner_payment") {
        None | Some(Value::Null) => out.bool(false),
        Some(payment) => {
            out.bool(true);
            out.bytes(&hash(payment.get(0))?);
            out.u64(number(payment.get(1))?);
        }
    }
    match state.get("owner_outcome") {
        None | Some(Value::Null) => out.bool(false),
        Some(outcome) => {
            out.bool(true);
            out.bytes(&hash(outcome.get(0))?);
            out.u8(u8::try_from(number(outcome.get(1))?).ok()?);
        }
    }
    Some(out.0)
}

fn decode_scheme(state: &[u8]) -> Map<String, Value> {
    let mut input = In(state);
    let mut joined = Map::new();
    assert_eq!(input.u8(), Some(1), "version 1");
    let mut read = || -> Option<()> {
        let cursor = if input.bool()? {
            serde_json::json!([input.u32()?, input.u32()?])
        } else {
            Value::Null
        };
        joined.insert("cursor".into(), cursor);
        joined.insert("write_offs".into(), input.u64()?.into());
        let mut peers = Map::new();
        for _ in 0..input.u32()? {
            let peer = input.name()?;
            let address = AccountId(input.take(20)?.try_into().ok()?);
            peers.insert(peer, address.to_address().into());
        }
        joined.insert("peers".into(), peers.into());
        let (mut recovering, mut unread) = (Vec::new(), Vec::new());
        for _ in 0..input.u32()? {
            let hash = input.hex(32)?;
            if input.bool()? {
                unread.push(hash.clone());
            }
            recovering.push(hash);
        }
        joined.insert("recovering".into(), recovering.into());
        joined.insert("unread".into(), unread.into());
        let mut entries = Vec::new();
        for _ in 0..input.u32()? {
            let node = input.name()?;
            let from = AccountId(input.take(20)?.try_into().ok()?).to_address();
            let amount = input.u64()?;
            let hash = input.hex(32)?;
            let until = input.u32()?;
            entries.push(serde_json::json!([node, from, amount, hash, until]));
        }
        joined.insert("entries".into(), entries.into());
        let payment = if input.bool()? {
            serde_json::json!([input.hex(32)?, input.u64()?])
        } else {
            Value::Null
        };
        joined.insert("owner_payment".into(), payment);
        let outcome = if input.bool()? {
            serde_json::json!([input.hex(32)?, input.u8()?])
        } else {
            Value::Null
        };
        joined.insert("owner_outcome".into(), outcome);
        input.0.is_empty().then_some(())
    };
    assert!(read().is_some(), "a state the scheme wrote");
    joined
}
