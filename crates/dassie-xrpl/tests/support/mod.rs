//! The node's first XRPL testnet account and the `xrpl-testnet` scheme on
//! it, wired together as the node wires them: after each input to the
//! account, the scheme is told what it learnt, and what both ask is handed
//! back in order. It speaks the interface the scheme had when it owned its
//! account, so the tests of what the scheme books read as they always did.
//!
//! Its state is the one the scheme wrote then: a single JSON object, which
//! it splits into the account's and the scheme's as the node's migration
//! does (spec/areas/xrpl.md#state-and-restarts), and joins again.

#![allow(dead_code, clippy::unwrap_used, clippy::panic)]

use dassie_ledger::Name;
use dassie_node_core::testing::SeededRng;
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
            AssetEffect::SendTo { .. } => panic!("an XRPL account names no endpoints"),
        }
    }
}

/// Keys of the joined state that are the account's.
const ACCOUNT_KEYS: [&str; 5] = ["sequence", "in_flight", "never_sent", "parked", "newest"];

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
        let account = XrplAccount::new(account::FIRST, &schemes, config, entropy, &account_state)?;
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

    pub fn check_invariants(&self) {
        self.account.check_invariants();
        self.scheme.check_invariants();
    }
}

/// The joined state, split as the migration splits it: the account takes
/// the sequence, what is in flight (each the scheme's) and the newest
/// ledger seen, and the scheme the rest. What neither could have written,
/// they refuse themselves.
fn split(state: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if state.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }
    let Value::Object(mut scheme) = serde_json::from_slice::<Value>(state).ok()? else {
        return None;
    };
    let mut account = Map::new();
    for key in ACCOUNT_KEYS {
        if let Some(value) = scheme.remove(key) {
            account.insert(key.into(), value);
        }
    }
    // Both count the account's write-offs, the scheme as it last saw them:
    // one count, once the scheme has been told.
    if let Some(count) = scheme.get("write_offs") {
        account.insert("write_offs".into(), count.clone());
    }
    account
        .entry("never_sent")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(Value::Array(in_flight)) = account.get_mut("in_flight") {
        for entry in in_flight.iter_mut() {
            if let Value::Object(entry) = entry {
                entry.insert("scheme".into(), "xrpl-testnet".into());
            }
        }
    }
    Some((
        serde_json::to_vec(&account).ok()?,
        serde_json::to_vec(&scheme).ok()?,
    ))
}

/// The account's and the scheme's states, as one.
fn join(account: &[u8], scheme: &[u8]) -> Vec<u8> {
    let Value::Object(mut joined) = serde_json::from_slice::<Value>(scheme).unwrap() else {
        panic!("a scheme's state is an object");
    };
    let Value::Object(mut account) = serde_json::from_slice::<Value>(account).unwrap() else {
        panic!("an account's state is an object");
    };
    if let Some(Value::Array(in_flight)) = account.get_mut("in_flight") {
        for entry in in_flight.iter_mut() {
            if let Value::Object(entry) = entry {
                entry.remove("scheme");
            }
        }
    }
    joined.append(&mut account);
    serde_json::to_vec(&joined).unwrap()
}
