//! BTP tokens, and the connections their clients open.

use std::collections::BTreeSet;

use dassie_connector::{EndpointError, EndpointId};
use dassie_ledger::{AccountId, AccountName, AccountPath, LedgerId, OwnerSub};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use super::packets::RECEIVER_SEGMENT;
use super::state::Core;
use super::{EngineError, Outbound};
use crate::log::target;
use crate::store::{BtpTokenRow, Store, StoreError};

/// Most tokens not revoked.
pub const MAX_TOKENS: usize = 64;

/// Most tokens ever made, revoked ones included: each keeps its account,
/// which the journal refers to, so this bounds what revoking can leave.
pub const MAX_TOKEN_ROWS: usize = 1024;

/// Most bytes in a token's label.
const MAX_LABEL_BYTES: usize = 64;

/// Random bytes in a token: 22 base64url characters, as the original's.
const TOKEN_BYTES: usize = 16;

/// The characters of a connection's address segment.
const SEGMENT_ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// A token, as the engine holds it.
pub(super) struct TokenState {
    pub row: BtpTokenRow,
    pub account: AccountId,
    pub connections: BTreeSet<u64>,
}

/// A connected client.
pub(super) struct BtpLink {
    pub token: u64,
    /// Dropping it ends the connection's task.
    pub outbound: mpsc::Sender<Outbound>,
}

/// A token as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenView {
    /// Its ID.
    pub id: u64,
    /// Its label.
    pub label: String,
    /// Its ledger.
    pub ledger: LedgerId,
    /// When it was made.
    pub created_at_ms: u64,
    /// Received less sent through it, posted.
    pub net_received: i128,
    /// Clients connected with it.
    pub connections: usize,
}

/// A token just made: the only time its secret is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewToken {
    /// The token.
    pub view: TokenView,
    /// The secret a client authenticates with.
    pub secret: String,
}

/// A client's connection, once its token checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtpSession {
    /// The engine's number for it.
    pub link: u64,
    /// The client's ILP address, which ILDCP hands it too.
    pub address: String,
}

/// The SHA-256 of a token: what the database keeps (D10).
#[must_use]
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl Core {
    pub(super) fn load_tokens(&mut self, store: &Store) -> Result<(), StoreError> {
        for row in store.btp_tokens()? {
            let path = token_path(&row);
            let account = self
                .ledger
                .find(&path)
                .ok_or(StoreError::Corrupt("BTP token account"))?;
            if row.revoked_at_ms.is_none() {
                self.by_hash.insert(row.token_hash, row.id);
            }
            self.tokens.insert(
                row.id,
                TokenState {
                    row,
                    account,
                    connections: BTreeSet::new(),
                },
            );
        }
        Ok(())
    }

    pub(super) fn create_token(
        &mut self,
        label: String,
        ledger: &str,
    ) -> Result<super::NewToken, EngineError> {
        if label.len() > MAX_LABEL_BYTES {
            return Err(EngineError::Invalid(
                "the label is longer than 64 bytes".into(),
            ));
        }
        let ledger =
            LedgerId::parse(ledger).ok_or_else(|| EngineError::Invalid("unknown ledger".into()))?;
        if !self
            .schemes
            .values()
            .any(|active| active.scheme.ledger() == ledger)
        {
            return Err(EngineError::Precondition(
                "no active scheme serves that ledger".into(),
            ));
        }
        if self.by_hash.len() >= MAX_TOKENS {
            return Err(EngineError::Exhausted("there are 64 tokens already".into()));
        }
        if self.tokens.len() >= MAX_TOKEN_ROWS {
            return Err(EngineError::Exhausted("1024 tokens have been made".into()));
        }
        let mut random = [0u8; TOKEN_BYTES];
        self.node.with_rng(|rng| rng.fill_bytes(&mut random));
        let mut encoded = [0u8; dassie_stream::base64url::encoded_len(TOKEN_BYTES)];
        let length = dassie_stream::base64url::encode(&random, &mut encoded);
        let secret =
            String::from_utf8_lossy(encoded.get(..length).unwrap_or_default()).into_owned();
        let id = self.tokens.keys().next_back().map_or(1, |last| last + 1);
        let row = BtpTokenRow {
            id,
            token_hash: token_hash(&secret),
            label,
            ledger,
            created_at_ms: self.now_ms(),
            revoked_at_ms: None,
        };
        let account = self
            .ledger
            .open(token_path(&row))
            .map_err(|e| EngineError::Exhausted(e.to_string()))?;
        self.take_journal();
        self.batch.tokens_created.push(row.clone());
        self.by_hash.insert(row.token_hash, id);
        self.tokens.insert(
            id,
            TokenState {
                row,
                account,
                connections: BTreeSet::new(),
            },
        );
        let view = self
            .token_view(id)
            .ok_or_else(|| EngineError::Internal("token".into()))?;
        tracing::info!(target: target::BTP, id, "BTP token created");
        Ok(super::NewToken { view, secret })
    }

    pub(super) fn revoke_token(&mut self, id: u64) -> Result<(), EngineError> {
        let now = self.now_ms();
        let Some(token) = self
            .tokens
            .get_mut(&id)
            .filter(|t| t.row.revoked_at_ms.is_none())
        else {
            return Err(EngineError::NotFound("no such token".into()));
        };
        token.row.revoked_at_ms = Some(now);
        self.by_hash.remove(&token.row.token_hash);
        let links: Vec<u64> = token.connections.iter().copied().collect();
        self.batch.tokens_revoked.push(id);
        // Their messages of this batch go out first; then they end.
        for link in links {
            if !self.closing.contains(&link) {
                self.closing.push(link);
            }
        }
        tracing::info!(target: target::BTP, id, "BTP token revoked");
        Ok(())
    }

    pub(super) fn token_views(&self) -> Vec<TokenView> {
        self.tokens
            .iter()
            .filter(|(_, token)| token.row.revoked_at_ms.is_none())
            .filter_map(|(&id, _)| self.token_view(id))
            .collect()
    }

    fn token_view(&self, id: u64) -> Option<TokenView> {
        let token = self.tokens.get(&id)?;
        let account = self.ledger.account(token.account)?;
        Some(TokenView {
            id,
            label: token.row.label.clone(),
            ledger: token.row.ledger,
            created_at_ms: token.row.created_at_ms,
            net_received: account.balance().net_credit(),
            connections: token.connections.len(),
        })
    }

    /// Checks a client's token and gives its connection an address of its
    /// own below the node's, which ILDCP will hand it.
    pub(super) fn btp_connect(
        &mut self,
        token: &str,
        outbound: mpsc::Sender<Outbound>,
    ) -> Result<BtpSession, EngineError> {
        let Some(&id) = self.by_hash.get(&token_hash(token)) else {
            return Err(EngineError::NotFound("unknown token".into()));
        };
        let Some(state) = self.tokens.get(&id) else {
            return Err(EngineError::NotFound("unknown token".into()));
        };
        let (account, ledger) = (state.account, state.row.ledger);
        let link = self.next_link;
        self.next_link += 1;
        let address = self.link_address(link, account, ledger)?;
        self.links.insert(
            link,
            BtpLink {
                token: id,
                outbound,
            },
        );
        if let Some(state) = self.tokens.get_mut(&id) {
            state.connections.insert(link);
        }
        tracing::debug!(target: target::BTP, link, %address, "BTP client connected");
        Ok(BtpSession { link, address })
    }

    /// Registers the link at a random, free segment.
    fn link_address(
        &mut self,
        link: u64,
        account: AccountId,
        ledger: LedgerId,
    ) -> Result<String, EngineError> {
        for _ in 0..8 {
            let mut random = [0u8; 6];
            self.node.with_rng(|rng| rng.fill_bytes(&mut random));
            let segment: String = random
                .iter()
                .map(|byte| {
                    let index = usize::from(*byte) % SEGMENT_ALPHABET.len();
                    char::from(SEGMENT_ALPHABET.get(index).copied().unwrap_or(b'a'))
                })
                .collect();
            if segment == RECEIVER_SEGMENT {
                continue;
            }
            let address = format!("{}.{segment}", self.connector.own_address());
            match self
                .connector
                .add_endpoint(EndpointId::Btp(link), &address, account, ledger)
            {
                Ok(()) => return Ok(address),
                Err(EndpointError::InvalidAddress) => {}
                Err(EndpointError::Full | EndpointError::Duplicate) => {
                    return Err(EngineError::Exhausted("too many BTP connections".into()));
                }
            }
        }
        Err(EngineError::Internal("no free address".into()))
    }
}

fn token_path(row: &BtpTokenRow) -> AccountPath {
    AccountPath::new(
        row.ledger,
        AccountName::EquityOwnerSub(OwnerSub::Btp(row.id)),
    )
}
