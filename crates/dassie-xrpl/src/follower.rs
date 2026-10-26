//! What every scheme on an XRPL account does alike, whatever it settles in
//! (spec/areas/xrpl.md#following-the-account): the XRP scheme
//! ([`crate::XrpScheme`]) and the token scheme ([`crate::TokenScheme`])
//! each embed one [`Follower`], and add only what their holding needs: the
//! XRP and its reserve, or the line, its issuer and the peg guard.
//!
//! A follower keeps what the books hold of the holding, its cursor into the
//! account's transactions, the peers' addresses, the settlements a restart
//! left to look up, the count of write-offs it last saw, the entry payments
//! it holds and the owner's payment, and it writes and reads all of them as
//! the scheme's state. Each part of a scheme's state is defined here once,
//! so the two schemes cannot drift apart.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use dassie_ledger::{MAX_TOTAL, Name};
use dassie_settlement::{
    Effect, EntryPayment, ExternalAsset, OwnerPayment, OwnerPaymentState, PaymentFailure,
    ReconcileCause, SchemeError,
};

use crate::account::{News, Resumed, XrplAccount};
use crate::api::TxRecord;
use crate::codec::Memo;
use crate::entry::Entries;
use crate::from_hex;
use crate::kept::{In, Out};
use crate::keys::AccountId;
use crate::scheme::{account_of, owner_memos};

/// Most peers whose addresses are kept: at least the node's settlement
/// channels open, with core nodes and with downlinks, so every node it
/// holds a channel with keeps its own; an address goes when its channel
/// closes. A table keyed by account, rather than this state,
/// is to hold them (spec/roadmap.md#periphery).
pub const MAX_PEERS: usize = 1536;
/// Most settlements of ours whose outcome is looked for after a restart.
pub(crate) const MAX_RECOVERING: usize = 64;
/// The longest a settlement's outcome goes unasked after a restart, in
/// ledgers: about a quarter of an hour on the testnet. One that no history
/// holds is never found, and is asked less often the longer it is not.
pub(crate) const MAX_RECOVER_WAIT_LEDGERS: u32 = 256;

/// A settlement of ours a restart left pending, though the account no
/// longer has it in flight: its outcome is looked up by its hash. Only the
/// hash, and whether it is unread, are kept with the books; a restart asks
/// at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Recovering {
    pub(crate) hash: [u8; 32],
    /// What the books hold pending for it, in internal units: what we
    /// signed, as the node hands it at each reconcile. A settlement never
    /// read is booked by it, never by what a server says it delivered.
    pub(crate) amount: Option<u64>,
    /// Whether the scheme never read it: it validated while the scheme was
    /// not on the account, and the history no longer reached back to it
    /// once it was ([`News::Unread`]). What it did is in the balance the
    /// scheme synced to, booked through suspense; otherwise, the scheme
    /// read it before a restart, and booked it then.
    pub(crate) unread: bool,
    /// The validated ledger from which it may be asked again.
    pub(crate) ask_at: u32,
    /// How many ledgers the next lookup waits for the one after it: it
    /// doubles with each, up to [`MAX_RECOVER_WAIT_LEDGERS`].
    pub(crate) wait: u32,
}

impl Recovering {
    pub(crate) const fn new(hash: [u8; 32]) -> Self {
        Self {
            hash,
            amount: None,
            unread: false,
            ask_at: 0,
            wait: 1,
        }
    }
}

/// What a scheme on an XRPL account keeps of the account, alike for every
/// holding.
pub(crate) struct Follower {
    /// The account it settles from.
    pub(crate) account: AccountId,
    /// Our node's ID, as our memos carry it.
    pub(crate) node: Name,
    /// What the books hold of the holding, in internal units.
    pub(crate) booked: u64,
    /// The most an owner's payment pays, in the holding's whole units
    /// (drops, or millionths of a token).
    max_owner_units: u64,
    // Kept with the books.
    /// Where it read the account's transactions to, as (ledger, index):
    /// set by its first sync, dropped only with the account.
    pub(crate) cursor: Option<(u32, u32)>,
    /// Peers' addresses, with when each was learnt: past [`MAX_PEERS`], the
    /// longest known goes.
    peers: BTreeMap<Name, (AccountId, u64)>,
    /// The peer table's revision, counted up on each address learnt and
    /// each forgotten: the order addresses were learnt in, and part of
    /// [`fingerprint`](Self::fingerprint), so that the node keeps every
    /// change to the table. Not kept: a restore learns the addresses
    /// anew, and it counts up from there.
    peers_revision: u64,
    pub(crate) recovering: Vec<Recovering>,
    /// How many times the account was written off, as the scheme last saw
    /// it ([`XrplAccount::write_offs`]).
    pub(crate) write_offs: u64,
    /// What nodes not yet peered paid us to enter, off the books
    /// (spec/decisions/pay-to-enter.md).
    pub(crate) entries: Entries,
    /// The owner's payment on its way: its hash, and the whole units it
    /// pays.
    pub(crate) owner_payment: Option<([u8; 32], u64)>,
    /// The owner's last payment that is no longer on its way, and what
    /// became of it, kept with the books so a restart still answers for it.
    pub(crate) owner_outcome: Option<([u8; 32], OwnerPaymentState)>,
    // Learnt from the account, as it told the scheme.
    /// The account's newest validated ledger.
    pub(crate) validated: u32,
    /// Whether the node peers on it: an owner book does not. From its
    /// config, never kept with the books.
    pub(crate) peering: bool,
}

impl Follower {
    /// A follower of `account` for node `node`, whose books last
    /// checkpointed `checkpoint`, and whose owner's payments pay at most
    /// `max_owner_units`.
    pub(crate) fn new(
        account: &XrplAccount,
        node: Name,
        checkpoint: Option<u64>,
        max_owner_units: u64,
    ) -> Self {
        Self {
            account: account.account(),
            node,
            booked: checkpoint.unwrap_or(0),
            max_owner_units,
            cursor: None,
            peers: BTreeMap::new(),
            peers_revision: 0,
            recovering: Vec::new(),
            write_offs: 0,
            entries: Entries::default(),
            owner_payment: None,
            owner_outcome: None,
            validated: 0,
            peering: true,
        }
    }

    /// Whether what it restored fits `account`: a settlement is looked up
    /// only when the account has it neither in flight nor as never sent,
    /// and it never has it so again.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidData`] for a state the two never wrote.
    pub(crate) fn fits(&self, account: &XrplAccount) -> Result<(), SchemeError> {
        if self
            .recovering
            .iter()
            .any(|r| account.holds_transaction(&r.hash))
        {
            return Err(SchemeError::InvalidData);
        }
        Ok(())
    }

    /// Whether the books hold anything of the account.
    pub(crate) const fn holds_anything(&self) -> bool {
        self.cursor.is_some() || self.booked > 0
    }

    /// Before the news of an input is told: the account's newest ledger and
    /// its count of write-offs taken. Returns whether the scheme missed a
    /// write-off while it was not on the account (stopped, or removed): the
    /// account counts another, and is not telling one now, which the
    /// scheme writes off in order. What it held went with the account, and
    /// its cursor is on a chain gone, which would hold the account back
    /// (spec/areas/xrpl.md#following-the-account).
    pub(crate) fn catch_up(&mut self, account: &XrplAccount, news: &[News]) -> bool {
        self.validated = account.validated();
        let telling = news
            .iter()
            .any(|news| matches!(news, News::WrittenOff { .. }));
        let missed = self.write_offs != account.write_offs() && !telling;
        self.write_offs = account.write_offs();
        missed
    }

    /// After the news of an input was told: an owner's payment the account
    /// keeps no more, and told nothing of, was lost with its state, and what
    /// it did is in the holding; one it keeps for the scheme, validated
    /// while the scheme was away, it tells again. Then the account is told
    /// where the scheme is.
    pub(crate) fn caught_up(
        &mut self,
        account: &mut XrplAccount,
        scheme: Name,
        effects: &mut Vec<Effect>,
    ) {
        if let Some((hash, _)) = self.owner_payment
            && !account.keeps_transaction(&hash)
        {
            self.owner_done(OwnerPaymentState::Unknown, effects);
        }
        account.follow(scheme, self.cursor, self.holds_anything());
    }

    /// The account is gone: what the books held is a loss through suspense,
    /// the entry payments went with it, and the cursor, on a chain gone,
    /// is dropped. What is looked for is asked again on a new chain.
    pub(crate) fn written_off(&mut self, restarted: bool, effects: &mut Vec<Effect>) {
        if restarted {
            for recovering in &mut self.recovering {
                recovering.ask_at = 0;
            }
        }
        if self.booked > 0 {
            effects.push(Effect::Reconcile {
                amount: self.booked,
                gain: false,
                cause: ReconcileCause::Unexplained,
            });
            self.booked = 0;
        }
        self.entries.clear();
        self.cursor = None;
    }

    /// Makes the books hold `target`, through suspense, for `cause`.
    pub(crate) fn reconcile_to(
        &mut self,
        target: u64,
        cause: ReconcileCause,
        effects: &mut Vec<Effect>,
    ) {
        if target != self.booked {
            effects.push(Effect::Reconcile {
                amount: target.abs_diff(self.booked),
                gain: target > self.booked,
                cause,
            });
            self.booked = target;
        }
    }

    /// Makes the books hold `booked`, at most a total. Below zero, they
    /// hold nothing, and what they are short comes from suspense, so that a
    /// reconcile after starts from what the books hold.
    pub(crate) fn hold(&mut self, booked: i128, effects: &mut Vec<Effect>) {
        self.booked = narrow(booked.clamp(0, i128::from(MAX_TOTAL)));
        if booked < 0 {
            effects.push(Effect::Reconcile {
                amount: narrow(-booked),
                gain: true,
                cause: ReconcileCause::Unexplained,
            });
        }
    }
}

/// The owner's payments.
impl Follower {
    /// Whether `hash` is the owner's payment on its way.
    pub(crate) fn is_owner_payment(&self, hash: &[u8; 32]) -> bool {
        self.owner_payment.is_some_and(|(owner, _)| owner == *hash)
    }

    /// Whether `hash` is the owner's last payment no longer on its way.
    pub(crate) fn was_owner_payment(&self, hash: &[u8; 32]) -> bool {
        self.owner_outcome.is_some_and(|(owner, _)| owner == *hash)
    }

    /// The owner's payment on its way is no longer: what became of it,
    /// which posts the node's hold on the owner's funds if it applied, and
    /// voids it otherwise. One whose outcome the scheme never read is in
    /// the holding it synced to.
    pub(crate) fn owner_done(&mut self, outcome: OwnerPaymentState, effects: &mut Vec<Effect>) {
        if let Some((hash, _)) = self.owner_payment.take() {
            self.owner_outcome = Some((hash, outcome));
            effects.push(Effect::OwnerPaid {
                id: hash_name(&hash),
                applied: outcome == OwnerPaymentState::Applied,
            });
        }
    }

    /// What became of the owner's payment `id`.
    pub(crate) fn owner_state(&self, id: Name) -> OwnerPaymentState {
        let Some(hash) = name_hash(id) else {
            return OwnerPaymentState::Unknown;
        };
        if self.is_owner_payment(&hash) {
            return OwnerPaymentState::InFlight;
        }
        match self.owner_outcome {
            Some((done, outcome)) if done == hash => outcome,
            _ => OwnerPaymentState::Unknown,
        }
    }

    /// Where `payment` goes and the memo it carries, if the scheme may sign
    /// one now: one at a time, never to the account itself, with a memo of
    /// the type its reader expects, never a settlement's ([`owner_memos`]).
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] while one is on its way;
    /// [`SchemeError::InvalidData`] for one it never signs.
    pub(crate) fn owner_request(
        &self,
        payment: &OwnerPayment,
    ) -> Result<(AccountId, Vec<Memo>), SchemeError> {
        if self.owner_payment.is_some() {
            return Err(SchemeError::Unavailable);
        }
        let destination = AccountId::parse(&payment.destination).ok_or(SchemeError::InvalidData)?;
        let memos = owner_memos(payment)?;
        if destination == self.account {
            return Err(SchemeError::InvalidData);
        }
        Ok((destination, memos))
    }

    /// The owner's payment `hash`, of `units` of the holding, that
    /// `scheme` just signed on `account`: executed at once, its submission
    /// going out after the batch that keeps it with the account's and the
    /// scheme's state (spec/areas/xrpl.md#deposits-and-withdrawals).
    pub(crate) fn owner_signed(
        &mut self,
        account: &mut XrplAccount,
        scheme: Name,
        hash: [u8; 32],
        units: u64,
    ) -> Name {
        assert!(
            (1..=self.max_owner_units).contains(&units),
            "an owner's payment pays what the books can hold"
        );
        let executed = account.execute(scheme, hash);
        assert!(executed, "a payment just signed is in flight");
        self.owner_payment = Some((hash, units));
        hash_name(&hash)
    }
}

/// Settlements of ours, as the account signs, resumes and looks them up.
impl Follower {
    /// Submits settlement `settlement` of `scheme`, which the node keeps
    /// with the books: one signed and then lost, or never sent, can never
    /// apply, and is cancelled.
    pub(crate) fn execute(
        asset: Option<&mut dyn ExternalAsset>,
        scheme: Name,
        settlement: Name,
        effects: &mut Vec<Effect>,
    ) {
        let account = asset.and_then(account_of);
        assert!(
            account.is_some(),
            "the scheme settles through an XRPL account"
        );
        if let (Some(account), Some(hash)) = (account, name_hash(settlement))
            && account.execute(scheme, hash)
        {
            return;
        }
        effects.push(Effect::Cancel { settlement });
    }

    /// Drops settlement `settlement` of `scheme`, never executed.
    pub(crate) fn discard(asset: Option<&mut dyn ExternalAsset>, scheme: Name, settlement: Name) {
        if let (Some(account), Some(hash)) = (asset.and_then(account_of), name_hash(settlement)) {
            account.discard(scheme, hash);
        }
    }

    /// Every settlement the node holds pending is `scheme`'s. One in flight
    /// is submitted again; one never sent is cancelled; one settled before
    /// the node could post it (its posting waited for an earlier
    /// announcement) is looked for by its hash, and posted or voided by
    /// what the ledger says.
    pub(crate) fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        scheme: Name,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        let account = asset.and_then(account_of);
        assert!(
            account.is_some(),
            "the scheme settles through an XRPL account"
        );
        let Some(account) = account else {
            return;
        };
        for (id, amount) in pending {
            let Some(hash) = name_hash(*id) else {
                continue;
            };
            match account.resume(scheme, hash) {
                Resumed::InFlight => {}
                Resumed::NeverSent => effects.push(Effect::Cancel { settlement: *id }),
                Resumed::Unknown => self.recover(hash, *amount),
            }
        }
    }

    /// Looks up settlement `hash`, pending in the books for `amount`, unless
    /// it is already, or [`MAX_RECOVERING`] are: one restored from the
    /// state is told its amount.
    fn recover(&mut self, hash: [u8; 32], amount: u64) {
        assert!(
            amount <= MAX_TOTAL,
            "the books hold at most a total pending"
        );
        if let Some(known) = self.recovering.iter_mut().find(|r| r.hash == hash) {
            known.amount = Some(amount);
        } else if self.recovering.len() < MAX_RECOVERING {
            let mut new = Recovering::new(hash);
            new.amount = Some(amount);
            self.recovering.push(new);
        }
    }

    /// A settlement of ours that validated while the scheme was not on the
    /// account, and that it did not read again: it is booked as one never
    /// read once found. One not looked up (past [`MAX_RECOVERING`]) stays
    /// pending, as any is.
    pub(crate) fn unread(&mut self, hash: [u8; 32]) {
        if let Some(recovering) = self.recovering.iter_mut().find(|r| r.hash == hash) {
            recovering.unread = true;
        }
    }

    /// Settlement `hash` is no longer looked up: the ledger showed it.
    pub(crate) fn resolved(&mut self, hash: &[u8; 32]) {
        self.recovering.retain(|r| r.hash != *hash);
    }

    /// The settlement looked up as `hash`, found, out of those looked up.
    pub(crate) fn found(&mut self, hash: &[u8; 32]) -> Option<Recovering> {
        let position = self.recovering.iter().position(|r| r.hash == *hash)?;
        Some(self.recovering.remove(position))
    }

    /// Asks for the outcome of a restart's settlements not asked for yet,
    /// as many at a time as the account looks up. Each is asked again only
    /// once its wait is over, and waits twice as long after each lookup,
    /// so one never found is not asked at every ledger.
    /// Only once `synced`: what the scheme did not read again, the account
    /// tells with the sync, before any lookup can be answered.
    pub(crate) fn ask_recover(&mut self, account: &mut XrplAccount, synced: bool) {
        if !synced {
            return;
        }
        let validated = self.validated;
        for recovering in &mut self.recovering {
            if !account.can_look_up() {
                return;
            }
            if validated >= recovering.ask_at && !account.is_looking_up(&recovering.hash) {
                recovering.ask_at = validated.saturating_add(recovering.wait);
                recovering.wait = recovering
                    .wait
                    .saturating_mul(2)
                    .min(MAX_RECOVER_WAIT_LEDGERS);
                account.look_up(recovering.hash);
            }
        }
    }
}

/// Peers, their settlements to us, and what nodes not yet peered paid.
impl Follower {
    /// The node a successful Payment to us names in a memo of `memo_type`,
    /// other than ours: whoever sent it. What it delivered, the scheme
    /// checks.
    pub(crate) fn named_by(&self, record: &TxRecord, memo_type: &[u8]) -> Option<Name> {
        if !(record.succeeded() && record.is_payment && record.destination == Some(self.account)) {
            return None;
        }
        let (kind, data) = record.memo.as_ref()?;
        if kind.as_slice() != memo_type {
            return None;
        }
        let node = Name::new(core::str::from_utf8(data).ok()?).ok()?;
        (node != self.node).then_some(node)
    }

    /// Whether `record`, which names `peer`, came from the peer's known
    /// address: then it is the peer's settlement.
    pub(crate) fn sent_by_peer(&self, record: &TxRecord, peer: Name) -> bool {
        self.peers.get(&peer).map(|(address, _)| *address) == Some(record.account)
    }

    /// Holds `record`, which names `node` and delivered `amount` in
    /// internal units, as an entry payment, if the table takes it
    /// (spec/decisions/pay-to-enter.md). Returns whether it does.
    pub(crate) fn hold_entry(&mut self, record: &TxRecord, node: Name, amount: u64) -> bool {
        // Its hold starts when the scheme reads it, as after a restart,
        // so its node has the whole of it to ask.
        let read = self.validated.max(record.ledger_index);
        self.entries
            .hold(node, record.account, amount, record.hash, read)
    }

    /// Books the entry payments whose hold is over as the owner's deposits.
    /// Returns whether any was: what fees or a reserve took meanwhile came
    /// out of what was held, and the scheme matches the books to what is
    /// left, the shortfall through suspense.
    pub(crate) fn expire_entries(&mut self, effects: &mut Vec<Effect>) -> bool {
        if self.cursor.is_none() {
            return false;
        }
        let expired = self.entries.expire(self.validated);
        if expired.is_empty() {
            return false;
        }
        for entry in expired {
            effects.push(Effect::Deposit {
                amount: entry.amount,
            });
            self.booked = self.booked.saturating_add(entry.amount);
        }
        true
    }

    /// Books what it held from `peer`, from the address it peered with, as
    /// its settlements: the books hold it from now on. The peer is credited
    /// what it paid; what the account no longer holds of it, spent on fees
    /// or held back by a reserve meanwhile, is a shortfall the scheme then
    /// matches through suspense.
    pub(crate) fn take_entry(&mut self, peer: Name, effects: &mut Vec<Effect>) {
        let Some((address, _)) = self.peers.get(&peer).copied() else {
            return;
        };
        for entry in self.entries.take(peer, address) {
            effects.push(Effect::IncomingSettlement {
                peer,
                amount: entry.amount,
                id: Some(hash_name(&entry.hash)),
            });
            self.booked = self.booked.saturating_add(entry.amount);
        }
    }

    /// Whether one is held from `peer`, from the address its request names.
    pub(crate) fn entry(&self, peer: Name, data: &[u8]) -> EntryPayment {
        self.entries.entry(peer, parse_address(data).ok())
    }

    /// Keeps a peer's address; past [`MAX_PEERS`], the one learnt longest
    /// ago goes. Only peerings made are learnt, each with a settlement
    /// channel, whose address goes when the channel closes, and a node has
    /// at most [`MAX_PEERS`] channels open: no peer's address goes for
    /// another's.
    pub(crate) fn learn(&mut self, peer: Name, address: AccountId) {
        if !self.peers.contains_key(&peer)
            && self.peers.len() >= MAX_PEERS
            && let Some(oldest) = self
                .peers
                .iter()
                .min_by_key(|(_, (_, learnt))| *learnt)
                .map(|(name, _)| *name)
        {
            self.peers.remove(&oldest);
        }
        self.peers_revision = self.peers_revision.wrapping_add(1);
        self.peers.insert(peer, (address, self.peers_revision));
    }

    /// Where `peer` settles to, as its state says: learnt if it is not
    /// what the table holds.
    pub(crate) fn destination(
        &mut self,
        peer: Name,
        peer_state: &[u8],
    ) -> Result<AccountId, SchemeError> {
        let destination = parse_address(peer_state)?;
        if self.peers.get(&peer).map(|(address, _)| *address) != Some(destination) {
            self.learn(peer, destination);
        }
        Ok(destination)
    }

    /// Peering info: our address.
    pub(crate) fn peering_info(&self, out: &mut Vec<u8>) {
        address_bytes(self.account, out);
    }

    /// The responder's address, checked now and learnt once the peering is
    /// made ([`Self::on_peered`]); the original ignored it.
    pub(crate) fn create_peering_request(
        &self,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        if !self.peering {
            return Err(SchemeError::Unsupported);
        }
        parse_address(peering_info)?;
        address_bytes(self.account, out);
        Ok(())
    }

    pub(crate) fn accept_peering_request(
        &self,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError> {
        if !self.peering {
            return Err(SchemeError::Unsupported);
        }
        parse_address(data)?;
        response.clear();
        Ok(data.to_vec())
    }

    pub(crate) fn finalize_peering_request(
        &self,
        peering_info: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        if !self.peering {
            return Err(SchemeError::Unsupported);
        }
        parse_address(peering_info)?;
        Ok(peering_info.to_vec())
    }

    /// The state is the peer's address, as accept or finalize returned it:
    /// learnt only now, so attempts never take a peer's place.
    pub(crate) fn on_peered(&mut self, peer: Name, state: &[u8]) {
        let address = parse_address(state);
        assert!(address.is_ok(), "a peer state the scheme did not return");
        if let Ok(address) = address {
            self.learn(peer, address);
        }
    }

    /// A Payment from its address is no settlement any more: it was
    /// followed past every one the peer made before the close, and is the
    /// owner's deposit if one comes (spec/decisions/closing-channels.md),
    /// once held as an entry payment if it is one.
    pub(crate) fn on_closed(&mut self, peer: Name) {
        // A new revision, or the node would not keep that the address
        // went, and a restart would bring it back.
        if self.peers.remove(&peer).is_some() {
            self.peers_revision = self.peers_revision.wrapping_add(1);
        }
    }
}

/// What is kept with the books.
impl Follower {
    /// A number that changes whenever what [`Self::write`] keeps does,
    /// cheaply, mixed with `extra`, a number of what the scheme keeps
    /// beside it: the node writes the state only then.
    pub(crate) fn fingerprint(&self, extra: u64) -> u64 {
        let mut mix: u64 = 0xcbf2_9ce4_8422_2325;
        let mut add = |value: u64| {
            mix ^= value;
            mix = mix.wrapping_mul(0x0100_0000_01b3);
        };
        let (ledger, index) = self.cursor.unwrap_or((u32::MAX, u32::MAX));
        add(u64::from(ledger) << 32 | u64::from(index));
        add(self.peers_revision);
        for Recovering { hash, unread, .. } in &self.recovering {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]) | u64::from(*unread) << 16);
        }
        add(self.recovering.len() as u64);
        add(self.write_offs);
        add(extra);
        if let Some((hash, units)) = self.owner_payment {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]));
            add(units);
        }
        if let Some((hash, outcome)) = self.owner_outcome {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]));
            add(u64::from(outcome_code(outcome)));
        }
        self.entries.fingerprint(&mut add);
        mix
    }

    /// Writes what it keeps, in this order: the cursor, the count of
    /// write-offs, the peers' addresses in the order they were learnt, the
    /// settlements looked up with whether each is unread, the entry
    /// payments held, the owner's payment on its way and what became of
    /// the last (spec/areas/xrpl.md#state-and-restarts). The scheme writes
    /// the version before it, and what it keeps of its own after it.
    pub(crate) fn write(&self, out: &mut Out) {
        out.option(self.cursor, |out, (ledger, index)| {
            out.u32(ledger);
            out.u32(index);
        });
        out.u64(self.write_offs);
        let mut peers: Vec<_> = self.peers.iter().collect();
        peers.sort_by_key(|(_, (_, learnt))| *learnt);
        out.count(peers.len());
        for (peer, (address, _)) in peers {
            out.name(*peer);
            out.account(*address);
        }
        out.count(self.recovering.len());
        for recovering in &self.recovering {
            out.bytes(&recovering.hash);
            out.bool(recovering.unread);
        }
        self.entries.write(out);
        out.option(self.owner_payment, |out, (hash, units)| {
            out.bytes(&hash);
            out.u64(units);
        });
        out.option(self.owner_outcome, |out, (hash, outcome)| {
            out.bytes(&hash);
            out.u8(outcome_code(outcome));
        });
    }

    /// Reads back what [`Self::write`] wrote. `None` for a state no
    /// follower writes: more than it keeps, a peer or a settlement twice,
    /// entry payments before the first sync, an owner's payment of nothing
    /// or of more than the books hold, or an outcome it never writes.
    pub(crate) fn read(&mut self, input: &mut In<'_>) -> Option<()> {
        self.cursor = input.option(|input| Some((input.u32()?, input.u32()?)))?;
        self.write_offs = input.u64()?;
        // A name is at least two bytes, and an address twenty.
        let peers = input.count(22, MAX_PEERS)?;
        for _ in 0..peers {
            let (peer, address) = (input.name()?, input.account()?);
            if self.peers.contains_key(&peer) {
                return None;
            }
            self.learn(peer, address);
        }
        let recovering = input.count(33, MAX_RECOVERING)?;
        for _ in 0..recovering {
            let hash = input.array::<32>()?;
            if self.recovering.iter().any(|r| r.hash == hash) {
                return None;
            }
            let mut kept = Recovering::new(hash);
            kept.unread = input.bool()?;
            self.recovering.push(kept);
        }
        self.entries.read(input)?;
        if self.cursor.is_none() && self.entries.total() > 0 {
            return None;
        }
        self.owner_payment = input.option(|input| {
            let hash = input.array::<32>()?;
            let units = input.u64()?;
            (1..=self.max_owner_units)
                .contains(&units)
                .then_some((hash, units))
        })?;
        self.owner_outcome =
            input.option(|input| Some((input.array::<32>()?, outcome_of(input.u8()?)?)))?;
        Some(())
    }

    /// Asserts what holds after every change.
    pub(crate) fn check_invariants(&self) {
        assert!(self.peers.len() <= MAX_PEERS);
        self.entries.check_invariants();
        assert!(
            self.cursor.is_some() || self.entries.total() == 0,
            "entry payments are read past the first sync"
        );
        assert!(
            self.owner_payment
                .is_none_or(|(_, units)| (1..=self.max_owner_units).contains(&units)),
            "the owner's payment pays whole units the books can hold"
        );
        assert!(
            self.owner_outcome
                .is_none_or(|(_, outcome)| outcome != OwnerPaymentState::InFlight),
            "the last owner's payment done is no longer on its way"
        );
        assert!(self.recovering.len() <= MAX_RECOVERING);
        let mut seen = BTreeSet::new();
        assert!(
            self.recovering.iter().all(|r| seen.insert(r.hash)),
            "each settlement looked up once"
        );
        for recovering in &self.recovering {
            assert!((1..=MAX_RECOVER_WAIT_LEDGERS).contains(&recovering.wait));
            assert!(recovering.amount.is_none_or(|amount| amount <= MAX_TOTAL));
            assert!(
                recovering.ask_at <= self.validated.saturating_add(MAX_RECOVER_WAIT_LEDGERS),
                "a lookup waits at most its longest wait"
            );
        }
        assert!(self.booked <= MAX_TOTAL, "the books hold at most a total");
    }
}

/// The XRP Ledger's result of a payment to an account that does not exist,
/// of less than the reserve that would create it.
const TOO_LITTLE_RESULT: &str = "tecNO_DST_INSUF_XRP";

/// Why the owner's payment `record`, which failed, failed.
pub(crate) fn failure_of(record: &TxRecord) -> PaymentFailure {
    if record.result == TOO_LITTLE_RESULT {
        PaymentFailure::TooLittle
    } else {
        PaymentFailure::Other
    }
}

/// How a state writes what became of an owner's payment.
const fn outcome_code(outcome: OwnerPaymentState) -> u8 {
    match outcome {
        OwnerPaymentState::Unknown | OwnerPaymentState::InFlight => 0,
        OwnerPaymentState::Applied => 1,
        OwnerPaymentState::Failed(PaymentFailure::Other) => 2,
        OwnerPaymentState::Failed(PaymentFailure::TooLittle) => 3,
        OwnerPaymentState::Cancelled => 4,
    }
}

/// What became of an owner's payment, as a state writes it.
const fn outcome_of(code: u8) -> Option<OwnerPaymentState> {
    Some(match code {
        0 => OwnerPaymentState::Unknown,
        1 => OwnerPaymentState::Applied,
        2 => OwnerPaymentState::Failed(PaymentFailure::Other),
        3 => OwnerPaymentState::Failed(PaymentFailure::TooLittle),
        4 => OwnerPaymentState::Cancelled,
        _ => return None,
    })
}

/// What the books hold, or a difference from it. Every amount booked is
/// within [`MAX_TOTAL`], and so is what the books held before it.
pub(crate) fn narrow(booked: i128) -> u64 {
    let narrow = u64::try_from(booked);
    assert!(narrow.is_ok(), "booked past what a u64 holds");
    narrow.unwrap_or_default()
}

/// A transaction hash as a settlement ID: uppercase hex, 64 characters.
pub(crate) fn hash_name(hash: &[u8; 32]) -> Name {
    Name::hex(hash, true)
}

/// A settlement ID as the transaction hash it is, if it is one.
pub(crate) fn name_hash(settlement: Name) -> Option<[u8; 32]> {
    from_hex(settlement.as_str())?.try_into().ok()
}

/// Peering info: the address, as the original's OER wrote it (IA5String,
/// 25 to 35 characters, after a length byte).
pub(crate) fn address_bytes(account: AccountId, out: &mut Vec<u8>) {
    let address = account.to_address();
    out.clear();
    out.push(u8::try_from(address.len()).unwrap_or(0));
    out.extend_from_slice(address.as_bytes());
}

pub(crate) fn parse_address(bytes: &[u8]) -> Result<AccountId, SchemeError> {
    let (&length, rest) = bytes.split_first().ok_or(SchemeError::InvalidData)?;
    if usize::from(length) != rest.len() {
        return Err(SchemeError::InvalidData);
    }
    let text = core::str::from_utf8(rest).map_err(|_| SchemeError::InvalidData)?;
    AccountId::parse(text).ok_or(SchemeError::InvalidData)
}

/// The claim books nothing: the ledger says what arrived, once
/// (spec/areas/xrpl.md#settlement-messages). It must at least be a
/// transaction hash.
pub(crate) fn check_claim(data: &[u8]) -> Result<(), SchemeError> {
    if data.len() == 32 {
        Ok(())
    } else {
        Err(SchemeError::InvalidData)
    }
}
