//! A connector with a funded owner, BTP children and a local receiver.

#![allow(dead_code)]

use dassie_connector::{Config, Connector, EndpointId, Outcome, Rates, Upstream};
use dassie_ilp::{
    Address, ErrorCode, Fulfill, MAX_PACKET_LENGTH, Prepare, Reject, Response, Timestamp,
    condition_for,
};
use dassie_ledger::{
    AccountId, AccountName, AccountPath, Config as LedgerConfig, JournalEntry, Ledger, LedgerId,
    OwnerSub, Reference, TransferSpec,
};
use dassie_oer::{Decode, Encode};

pub const OWN: &str = "test.das.dnode";
pub const NOW: u64 = 1_700_000_000_000;
pub const RECEIVER: EndpointId = EndpointId::Local(0);

pub struct Harness {
    pub connector: Connector,
    pub ledger: Ledger,
    pub rates: Rates,
    pub buffer: Vec<u8>,
    pub owner: AccountId,
    pub journal: Vec<JournalEntry>,
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
            rates: Rates::new(NOW),
            buffer: vec![0; MAX_PACKET_LENGTH],
            owner,
            journal: Vec::new(),
        };
        harness.add_btp(1, "alice");
        harness.add_btp(2, "bob");
        harness
            .connector
            .add_endpoint(RECEIVER, &format!("{OWN}.stream"), owner, LedgerId::StubUsd)
            .unwrap();
        harness
    }

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

    pub fn prepare(
        &mut self,
        now: u64,
        from: EndpointId,
        request_id: u32,
        packet: &[u8],
    ) -> Outcome {
        let outcome = self.connector.handle_prepare(
            now,
            from,
            request_id,
            packet,
            &mut self.ledger,
            &self.rates,
            &mut self.buffer,
        );
        self.check();
        outcome
    }

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
        upstream
    }

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

    pub fn reject_written(&self, length: usize) -> (ErrorCode, Vec<u8>) {
        match Response::decode_exact(&self.buffer[..length]).unwrap() {
            Response::Reject(reject) => (reject.code, reject.data.to_vec()),
            Response::Fulfill(_) => panic!("expected a reject"),
        }
    }

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

pub fn fulfill(fulfillment: [u8; 32]) -> Vec<u8> {
    Fulfill {
        fulfillment,
        data: b"thanks",
    }
    .to_vec()
}

pub fn reject(code: ErrorCode) -> Vec<u8> {
    Reject {
        code,
        triggered_by: Address::parse("test.elsewhere").ok(),
        message: "no",
        data: &[],
    }
    .to_vec()
}

pub fn decode_prepare(bytes: &[u8]) -> Prepare<'_> {
    Prepare::decode_exact(bytes).unwrap()
}
