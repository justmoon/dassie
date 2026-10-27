//! A connector with a funded owner, BTP children and a local receiver, for
//! this crate's tests and the simulation runner
//! (spec/decisions/random-seed-simulations.md).
//!
//! Test code, behind the `sim` feature: a panic here is a failed run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

pub mod traffic;

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use crate::pricing::DEFAULT_REBALANCE_PPM;
use crate::{
    Charges, Config, Connector, EndpointId, Holding, Inventory, OpensNone, Outcome, PriceTable,
    Pricing, Rates, Responded, Upstream,
};
use dassie_ilp::{
    Address, ErrorCode, Fulfill, MAX_PACKET_LENGTH, Prepare, Reject, Response, Timestamp,
    condition_for,
};
use dassie_ledger::{
    AccountId, AccountName, AccountPath, Config as LedgerConfig, JournalEntry, Ledger, LedgerId,
    OwnerSub, Reference, TransferSpec,
};
use dassie_oer::{Decode, Encode};

/// The connector's own address.
pub const OWN: &str = "test.das.dnode";
/// When every test starts.
pub const NOW: u64 = 1_700_000_000_000;
/// The local STREAM receiver's endpoint.
pub const RECEIVER: EndpointId = EndpointId::Local(0);

/// A connector and its ledger, checked after every step.
pub struct Harness {
    /// The connector.
    pub connector: Connector,
    /// Its ledger.
    pub ledger: Ledger,
    /// Its exchange rates.
    pub rates: Rates,
    /// What the owner set for transit: no charges, and the curve a new
    /// node starts with.
    pub pricing: Pricing,
    /// Where it writes packets.
    pub buffer: Vec<u8>,
    /// The owner's pool.
    pub owner: AccountId,
    /// What the ledger journaled, as the node persists it.
    pub journal: Vec<JournalEntry>,
    /// The number the next Prepare's next hop is drawn with, unless a test
    /// or the simulation sets it (spec/decisions/weighted-next-hops.md).
    /// 0 takes a route's first usable option on the packet's incoming
    /// ledger, or its first usable one when none is on that ledger; with an
    /// exploring share above 0, as by default, a draw of 0 explores, so
    /// earnings never change what it takes.
    pub draw: u64,
}

impl Harness {
    /// The owner's pool holds `funds`; `btp:1` and `btp:2` are token
    /// sub-accounts at `own.alice` and `own.bob`; the receiver is the pool
    /// itself at `own.stream`.
    pub fn new(config: Config, funds: u64) -> Self {
        let mut ledger = Ledger::new(LedgerConfig::DEFAULT);
        let open = |ledger: &mut Ledger, name| {
            ledger
                .open(AccountPath::new(LedgerId::StubUsd, name))
                .unwrap()
        };
        let settlement = open(&mut ledger, AccountName::AssetsSettlement);
        let owner = open(&mut ledger, AccountName::EquityOwner);
        if funds > 0 {
            let deposit = TransferSpec {
                debit: settlement,
                credit: owner,
                amount: funds,
                pending: false,
                reference: Reference::Deposit,
            };
            ledger.create(0, &deposit).unwrap();
        }
        let mut harness = Self {
            connector: Connector::new(config, OWN),
            ledger,
            rates: Rates::new(),
            pricing: Pricing {
                charges: Charges::NONE,
                rebalance_ppm: DEFAULT_REBALANCE_PPM,
            },
            buffer: vec![0; MAX_PACKET_LENGTH],
            owner,
            journal: Vec::new(),
            draw: 0,
        };
        harness.add_btp(1, "alice");
        harness.add_btp(2, "bob");
        harness
            .connector
            .add_endpoint(RECEIVER, &format!("{OWN}.stream"), owner, LedgerId::StubUsd)
            .unwrap();
        harness
    }

    /// A BTP child `btp:{id}` at `own.{segment}`, on its own token
    /// sub-account.
    pub fn add_btp(&mut self, id: u64, segment: &str) -> AccountId {
        let account = self
            .ledger
            .open(AccountPath::new(
                LedgerId::StubUsd,
                AccountName::EquityOwnerSub(OwnerSub::Btp(id)),
            ))
            .unwrap();
        self.connector
            .add_endpoint(
                EndpointId::Btp(id),
                &format!("{OWN}.{segment}"),
                account,
                LedgerId::StubUsd,
            )
            .unwrap();
        account
    }

    /// Handles a Prepare from `from`.
    pub fn prepare(
        &mut self,
        now: u64,
        from: EndpointId,
        request_id: u32,
        packet: &[u8],
    ) -> Outcome {
        let prices = self.prices(now);
        let outcome = self.connector.handle_prepare(
            now,
            from,
            request_id,
            packet,
            &mut self.ledger,
            &prices,
            self.draw,
            &mut self.buffer,
        );
        self.check();
        outcome
    }

    /// Handles a Prepare from `from` on its slot on `on`, the ledger of the
    /// settlement method a peer's Prepare came on.
    pub fn prepare_on(
        &mut self,
        now: u64,
        (from, on): (EndpointId, LedgerId),
        request_id: u32,
        packet: &[u8],
    ) -> Outcome {
        let prices = self.prices(now);
        let outcome = self.connector.handle_prepare_with(
            now,
            (from, request_id),
            Some(on),
            packet,
            &mut self.ledger,
            &prices,
            &mut OpensNone,
            self.draw,
            &mut self.buffer,
        );
        self.check();
        outcome
    }

    /// The price table at `now`, from the rates, the pricing and what the
    /// owner's group holds on each ledger whose books are open, as the node
    /// builds it.
    pub fn prices(&self, now: u64) -> PriceTable {
        let inventories: Vec<Inventory> = LedgerId::ALL
            .into_iter()
            .filter_map(|ledger| self.inventory(ledger))
            .collect();
        let max_age_ms = self.connector.config().max_rate_age_ms;
        PriceTable::build(&self.rates, &inventories, self.pricing, now, max_age_ms)
    }

    /// The owner's group on `ledger`, if its books are open, and what it
    /// holds.
    pub fn inventory(&self, ledger: LedgerId) -> Option<Inventory> {
        let owner = self
            .ledger
            .find(&AccountPath::new(ledger, AccountName::EquityOwner))?;
        let balance = self.ledger.group_balance(owner)?;
        Some(Inventory {
            ledger,
            owner,
            holding: Holding::of(&balance),
        })
    }

    /// Handles a response, and says where it goes on.
    pub fn respond(
        &mut self,
        now: u64,
        from: EndpointId,
        request_id: u32,
        packet: &[u8],
    ) -> Option<Upstream> {
        let upstream = self.connector.handle_response(
            now,
            from,
            request_id,
            packet,
            &mut self.ledger,
            &mut self.buffer,
        );
        self.check();
        upstream.upstream()
    }

    /// Handles a response, and says what it did.
    pub fn respond_full(
        &mut self,
        now: u64,
        from: EndpointId,
        request_id: u32,
        packet: &[u8],
    ) -> Responded {
        let responded = self.connector.handle_response(
            now,
            from,
            request_id,
            packet,
            &mut self.ledger,
            &mut self.buffer,
        );
        self.check();
        responded
    }

    /// Expires what is due.
    pub fn timeout(&mut self, now: u64) -> Option<Upstream> {
        let upstream = self
            .connector
            .handle_timeout(now, &mut self.ledger, &mut self.buffer);
        self.check();
        upstream
    }

    /// Checks the invariants and persists the journal, as the node does
    /// after every step.
    pub fn check(&mut self) {
        self.connector.check_invariants(&self.ledger);
        self.ledger.check_invariants();
        while let Some(entry) = self.ledger.poll_journal() {
            self.journal.push(entry);
        }
    }

    /// The packet the last call wrote, `length` bytes of it.
    pub fn written(&self, length: usize) -> Vec<u8> {
        self.buffer[..length].to_vec()
    }

    /// The Reject the last call wrote: its code and data.
    pub fn reject_written(&self, length: usize) -> (ErrorCode, Vec<u8>) {
        match Response::decode_exact(&self.buffer[..length]).unwrap() {
            Response::Reject(reject) => (reject.code, reject.data.to_vec()),
            Response::Fulfill(_) => panic!("expected a reject"),
        }
    }

    /// An account's balance.
    pub fn net(&self, account: AccountId) -> i128 {
        self.ledger.account(account).unwrap().balance().net_credit()
    }
}

/// A Prepare's bytes.
pub fn prepare(destination: &str, amount: u64, expires_at: u64, fulfillment: [u8; 32]) -> Vec<u8> {
    Prepare {
        amount,
        expires_at: Timestamp::from_unix_ms(expires_at),
        execution_condition: condition_for(&fulfillment),
        destination: Address::parse(destination).unwrap(),
        data: b"payload",
    }
    .to_vec()
}

/// A Fulfill's bytes.
pub fn fulfill(fulfillment: [u8; 32]) -> Vec<u8> {
    Fulfill {
        fulfillment,
        data: b"thanks",
    }
    .to_vec()
}

/// A Reject's bytes.
pub fn reject(code: ErrorCode) -> Vec<u8> {
    Reject {
        code,
        triggered_by: Address::parse("test.elsewhere").ok(),
        message: "no",
        data: &[],
    }
    .to_vec()
}

/// Decodes a Prepare, which must be one.
pub fn decode_prepare(bytes: &[u8]) -> Prepare<'_> {
    Prepare::decode_exact(bytes).unwrap()
}

impl Harness {
    /// A peer `peer:{id}`, extending `trust`, at `test.das.{node}`, with
    /// its first request ID `first`.
    pub fn add_peer(&mut self, id: u64, node: &str, trust: u64, first: u32) -> AccountId {
        self.add_peer_on(LedgerId::StubUsd, id, node, trust, first)
    }

    /// A peer as [`add_peer`](Self::add_peer), on `ledger`.
    pub fn add_peer_on(
        &mut self,
        ledger: LedgerId,
        id: u64,
        node: &str,
        trust: u64,
        first: u32,
    ) -> AccountId {
        let name = dassie_ledger::Name::new(node).unwrap();
        let (account, _) = self.ledger.open_peer(0, ledger, name, trust).unwrap();
        while let Some(entry) = self.ledger.poll_journal() {
            self.journal.push(entry);
        }
        self.connector
            .add_peer(
                EndpointId::Peer(id),
                &format!("test.das.{node}"),
                Some(account),
                ledger,
                first,
            )
            .unwrap();
        account
    }

    /// A further settlement method of the peer `peer:{id}`, whose node is
    /// `node`, on `ledger`, extending it `trust` there: a slot on the
    /// peer's accounts on that ledger.
    pub fn add_slot(&mut self, ledger: LedgerId, id: u64, node: &str, trust: u64) -> AccountId {
        let name = dassie_ledger::Name::new(node).unwrap();
        let (account, _) = self.ledger.open_peer(0, ledger, name, trust).unwrap();
        while let Some(entry) = self.ledger.poll_journal() {
            self.journal.push(entry);
        }
        self.connector
            .add_slot(EndpointId::Peer(id), ledger, Some(account))
            .unwrap();
        self.check();
        account
    }

    /// A downlink `peer:{id}`, extending `trust`, at its child address
    /// below ours, `{OWN}.{node}` (spec/areas/connector.md#addresses-and-endpoints).
    pub fn add_downlink(&mut self, id: u64, node: &str, trust: u64, first: u32) -> AccountId {
        let name = dassie_ledger::Name::new(node).unwrap();
        let ledger = LedgerId::StubUsd;
        let (account, _) = self.ledger.open_peer(0, ledger, name, trust).unwrap();
        while let Some(entry) = self.ledger.poll_journal() {
            self.journal.push(entry);
        }
        self.connector
            .add_peer(
                EndpointId::Peer(id),
                &format!("{OWN}.{node}"),
                Some(account),
                ledger,
                first,
            )
            .unwrap();
        account
    }

    /// Opens the accounts a ledger's fees and conversions book to, as the
    /// node does when it activates the ledger.
    pub fn open_books(&mut self, ledger: LedgerId) {
        for name in [
            AccountName::RevenueFees,
            AccountName::RevenueFx,
            AccountName::ExpensesFx,
        ] {
            self.ledger.open(AccountPath::new(ledger, name)).unwrap();
        }
        while let Some(entry) = self.ledger.poll_journal() {
            self.journal.push(entry);
        }
    }

    /// Deposits `amount` into the owner's funds on `ledger`, opening its
    /// accounts: what the owner pays out on it, conversions included.
    pub fn fund(&mut self, ledger: LedgerId, amount: u64) -> AccountId {
        let settlement = self
            .ledger
            .open(AccountPath::new(ledger, AccountName::AssetsSettlement))
            .unwrap();
        let owner = self
            .ledger
            .open(AccountPath::new(ledger, AccountName::EquityOwner))
            .unwrap();
        let deposit = TransferSpec {
            debit: settlement,
            credit: owner,
            amount,
            pending: false,
            reference: Reference::Deposit,
        };
        if amount > 0 {
            self.ledger.create(0, &deposit).unwrap();
        }
        while let Some(entry) = self.ledger.poll_journal() {
            self.journal.push(entry);
        }
        owner
    }

    /// The balance of the account at `name` on `ledger`.
    pub fn net_at(&self, ledger: LedgerId, name: AccountName) -> i128 {
        let id = self.ledger.find(&AccountPath::new(ledger, name)).unwrap();
        self.net(id)
    }
}
