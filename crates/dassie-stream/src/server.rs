//! A STREAM server: issues credentials and routes incoming packets to the
//! connections they belong to.

use alloc::collections::{BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

use dassie_ilp::{Address, ErrorCode as IlpErrorCode, Prepare, Reject, Timestamp};
use dassie_oer::Encode;
use rand_core::CryptoRng;

use crate::base64url;
use crate::connection::{AssetDetails, BUFFER_LENGTH, Config, Connection, Role};
use crate::frame::ErrorCode;
use crate::policy::Policy;
use crate::receipt::ReceiptSecret;
use crate::token::{MAX_TOKEN_LENGTH, ServerSecret, TokenContents};

/// Whether `ids` increase strictly, all below `next`, the next to give.
fn given_in_order(ids: impl Iterator<Item = ConnectionId>, next: u64) -> bool {
    let mut last: Option<u64> = None;
    ids.into_iter().all(|ConnectionId(id)| {
        let in_order = last.is_none_or(|last| last < id) && id < next;
        last = Some(id);
        in_order
    })
}

/// Identifies a connection of a [`Server`]. Assigned in order, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionId(pub u64);

/// What a client needs to connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// The ILP address to send to.
    pub destination: String,
    /// The connection's shared secret.
    pub shared_secret: [u8; 32],
}

/// Whether the application lets a packet into a connection, decided from
/// the connection's tag before the server opens or feeds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Reject the packet with this ILP error, without opening a connection.
    Refuse(IlpErrorCode),
    /// Let it in. The connection may receive at most `money_limit` more in
    /// total, over all its streams; `u64::MAX` means no limit beyond the
    /// streams' own, and 0 that it takes nothing more: a packet that
    /// offers money then closes it with `exhausted`. `received` is what
    /// the application already counts as received for this tag, over
    /// earlier connections too (see [`Connection::set_money_limit`]).
    Accept {
        /// What the connection may still receive.
        money_limit: u64,
        /// What was received before, as the application counts it.
        received: u64,
        /// The code that closes the connection once nothing is left.
        exhausted: ErrorCode,
    },
}

/// What [`Server::handle_prepare_admitted`] did with a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handled {
    /// The length of the ILP response written into the buffer.
    pub length: usize,
    /// The connection the packet went to, if one took it.
    pub connection: Option<ConnectionId>,
    /// The money the packet brought, if it was fulfilled.
    pub received: u64,
}

/// Everything a server needs at creation.
#[derive(Clone, Copy, Debug)]
pub struct ServerConfig<'a> {
    /// The master secret all credentials derive from. Keeping it across
    /// restarts keeps issued credentials valid.
    pub secret: [u8; 32],
    /// The server's ILP address; connection addresses are below it.
    pub address: Address<'a>,
    /// The server's asset.
    pub asset: Option<AssetDetails<'a>>,
    /// The policy of every connection.
    pub policy: Policy,
    /// Most connections open at once.
    pub max_connections: usize,
}

#[derive(Debug)]
struct Entry {
    id: ConnectionId,
    token: Vec<u8>,
    tag: Vec<u8>,
    connection: Connection,
}

/// A STREAM server.
#[derive(Debug)]
pub struct Server {
    secret: ServerSecret,
    address: String,
    asset: Option<(String, u8)>,
    policy: Policy,
    max_connections: usize,
    next_id: u64,
    entries: Vec<Entry>,
    opened: VecDeque<ConnectionId>,
}

impl Server {
    /// Creates a server.
    ///
    /// # Panics
    ///
    /// If the policy is inconsistent or `max_connections` is zero.
    #[must_use]
    pub fn new(config: &ServerConfig<'_>) -> Self {
        config.policy.validate();
        assert!(
            config.max_connections > 0,
            "a server needs room for a connection"
        );
        Self {
            secret: ServerSecret::new(config.secret),
            address: String::from(config.address.as_str()),
            asset: config
                .asset
                .map(|asset| (String::from(asset.code), asset.scale)),
            policy: config.policy,
            max_connections: config.max_connections,
            next_id: 0,
            entries: Vec::with_capacity(config.max_connections),
            opened: VecDeque::with_capacity(config.max_connections),
        }
    }

    /// The server's ILP address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Issues credentials for a new connection. `connection_tag` labels it
    /// (see [`connection_tag`](Self::connection_tag)); `receipt` makes it
    /// issue receipts.
    ///
    /// # Panics
    ///
    /// If the tag is longer than [`crate::token::MAX_CONNECTION_TAG_LENGTH`].
    pub fn generate_credentials(
        &self,
        rng: &mut (impl CryptoRng + ?Sized),
        connection_tag: &[u8],
        receipt: Option<ReceiptSecret>,
    ) -> Credentials {
        let token = self.issue_token(rng, connection_tag, receipt);
        self.credentials_for(&token)
    }

    /// Issues the token of new credentials, for an application that keeps
    /// it and gives out the same credentials again with
    /// [`credentials_for`](Self::credentials_for).
    ///
    /// # Panics
    ///
    /// If the tag is longer than [`crate::token::MAX_CONNECTION_TAG_LENGTH`].
    pub fn issue_token(
        &self,
        rng: &mut (impl CryptoRng + ?Sized),
        connection_tag: &[u8],
        receipt: Option<ReceiptSecret>,
    ) -> Vec<u8> {
        let mut token = [0u8; MAX_TOKEN_LENGTH];
        let contents = TokenContents {
            connection_tag,
            receipt,
        };
        let length = self.secret.issue_token(rng, &contents, &mut token);
        Vec::from(token.get(..length).unwrap_or_default())
    }

    /// The credentials of a token this server issued.
    ///
    /// # Panics
    ///
    /// If the token is longer than [`MAX_TOKEN_LENGTH`].
    #[must_use]
    pub fn credentials_for(&self, token: &[u8]) -> Credentials {
        assert!(token.len() <= MAX_TOKEN_LENGTH, "token too long");
        let mut encoded = [0u8; base64url::encoded_len(MAX_TOKEN_LENGTH)];
        let encoded_length = base64url::encode(token, &mut encoded);
        let mut destination = String::with_capacity(self.address.len() + 1 + encoded_length);
        destination.push_str(&self.address);
        destination.push('.');
        for &character in encoded.get(..encoded_length).unwrap_or_default() {
            destination.push(char::from(character));
        }
        Credentials {
            destination,
            shared_secret: self.secret.shared_secret(token),
        }
    }

    /// Answers an incoming Prepare addressed to this server, writing the
    /// ILP response into `buffer` and returning its length. A packet for a
    /// connection the server has not seen opens it.
    ///
    /// # Panics
    ///
    /// If `buffer` is shorter than [`BUFFER_LENGTH`].
    pub fn handle_prepare(
        &mut self,
        now: Timestamp,
        rng: &mut (impl CryptoRng + ?Sized),
        prepare: &Prepare<'_>,
        buffer: &mut [u8],
    ) -> usize {
        let admit = |_: &[u8]| Admission::Accept {
            money_limit: u64::MAX,
            received: 0,
            exhausted: ErrorCode::NO_ERROR,
        };
        self.handle_prepare_admitted(now, rng, prepare, admit, buffer)
            .length
    }

    /// Like [`handle_prepare`](Self::handle_prepare), but asks `admit`
    /// first, with the connection's tag, whether the packet may in and how
    /// much money its connection may still receive. A refused packet opens
    /// no connection.
    ///
    /// # Panics
    ///
    /// If `buffer` is shorter than [`BUFFER_LENGTH`].
    pub fn handle_prepare_admitted(
        &mut self,
        now: Timestamp,
        rng: &mut (impl CryptoRng + ?Sized),
        prepare: &Prepare<'_>,
        admit: impl FnOnce(&[u8]) -> Admission,
        buffer: &mut [u8],
    ) -> Handled {
        assert!(
            buffer.len() >= BUFFER_LENGTH,
            "buffer shorter than BUFFER_LENGTH"
        );
        let refused = |server: &Self, code, buffer: &mut [u8]| Handled {
            length: server.reject(code, buffer),
            connection: None,
            received: 0,
        };
        let mut token = [0u8; MAX_TOKEN_LENGTH];
        let Some(token_length) = self.token_of(prepare.destination, &mut token) else {
            return refused(self, IlpErrorCode::F02_UNREACHABLE, buffer);
        };
        let token = token.get(..token_length).unwrap_or_default();
        let existing = self.entries.iter().position(|entry| entry.token == token);
        let admission = match existing.and_then(|index| self.entries.get(index)) {
            Some(entry) => admit(&entry.tag),
            None => {
                let mut scratch = [0u8; MAX_TOKEN_LENGTH];
                match self.secret.open_token(token, &mut scratch) {
                    Ok(contents) => admit(contents.connection_tag),
                    Err(_) => Admission::Refuse(IlpErrorCode::F02_UNREACHABLE),
                }
            }
        };
        let (money_limit, received_before, exhausted) = match admission {
            Admission::Refuse(code) => return refused(self, code, buffer),
            Admission::Accept {
                money_limit,
                received,
                exhausted,
            } => (money_limit, received, exhausted),
        };
        let index = match existing {
            Some(index) => index,
            None => match self.open(token) {
                Ok(index) => index,
                Err(code) => return refused(self, code, buffer),
            },
        };
        let Some(entry) = self.entries.get_mut(index) else {
            return refused(self, IlpErrorCode::T00_INTERNAL_ERROR, buffer);
        };
        entry
            .connection
            .set_money_limit(money_limit, received_before, exhausted);
        let before = entry.connection.stats().amount_received;
        let length = entry.connection.handle_prepare(now, rng, prepare, buffer);
        let received = entry.connection.stats().amount_received - before;
        assert!(
            received <= money_limit,
            "connection took more than its limit"
        );
        Handled {
            length,
            connection: Some(entry.id),
            received,
        }
    }

    /// Extracts and decodes the token: the first segment after the server's
    /// address. Further segments are ignored, as upstream does; the original
    /// took everything after the address
    /// (spec/original/04-interledger.md#114-server-serverts).
    fn token_of(
        &self,
        destination: Address<'_>,
        output: &mut [u8; MAX_TOKEN_LENGTH],
    ) -> Option<usize> {
        let own = Address::parse(&self.address).ok()?;
        let local = destination.strip_prefix(own)?;
        let segment = local.split('.').next()?;
        if segment.len() > base64url::encoded_len(MAX_TOKEN_LENGTH) {
            return None;
        }
        base64url::decode(segment.as_bytes(), output).ok()
    }

    fn open(&mut self, token: &[u8]) -> Result<usize, IlpErrorCode> {
        let mut scratch = [0u8; MAX_TOKEN_LENGTH];
        let contents = self
            .secret
            .open_token(token, &mut scratch)
            .map_err(|_| IlpErrorCode::F02_UNREACHABLE)?;
        if self.entries.len() >= self.max_connections {
            self.remove_closed();
        }
        if self.entries.len() >= self.max_connections {
            self.evict_least_active();
        }
        if self.entries.len() >= self.max_connections {
            return Err(IlpErrorCode::T03_CONNECTOR_BUSY);
        }
        let shared_secret = self.secret.shared_secret(token);
        let connection = Connection::new(&Config {
            role: Role::Server,
            shared_secret: &shared_secret,
            local_address: Address::parse(&self.address).ok(),
            remote_address: None,
            local_asset: self.asset.as_ref().map(|(code, scale)| AssetDetails {
                code,
                scale: *scale,
            }),
            receipt: contents.receipt,
            policy: self.policy,
        });
        let id = ConnectionId(self.next_id);
        self.next_id += 1;
        self.entries.push(Entry {
            id,
            token: Vec::from(token),
            tag: Vec::from(contents.connection_tag),
            connection,
        });
        if self.opened.len() >= self.max_connections {
            // The application is not polling; the oldest notice goes.
            self.opened.pop_front();
        }
        self.opened.push_back(id);
        Ok(self.entries.len() - 1)
    }

    /// Makes room in a full table by forgetting the connection heard from
    /// least recently. Its sender's next packet opens it again: the
    /// application's admission says what it may still receive. So no
    /// remote can keep the table full by keeping idle connections alive.
    fn evict_least_active(&mut self) {
        let oldest = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.connection.last_activity())
            .map(|(index, _)| index);
        if let Some(index) = oldest {
            self.entries.remove(index);
        }
    }

    fn reject(&self, code: IlpErrorCode, buffer: &mut [u8]) -> usize {
        Reject {
            code,
            triggered_by: Address::parse(&self.address).ok(),
            message: "",
            data: &[],
        }
        .encode(buffer)
    }

    /// The next connection opened since the last call.
    pub fn poll_opened(&mut self) -> Option<ConnectionId> {
        self.opened.pop_front()
    }

    /// The open connections, oldest first.
    pub fn connections(&self) -> impl Iterator<Item = (ConnectionId, &Connection)> {
        self.entries
            .iter()
            .map(|entry| (entry.id, &entry.connection))
    }

    /// The connection `id`.
    #[must_use]
    pub fn connection(&self, id: ConnectionId) -> Option<&Connection> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.connection)
    }

    /// The connection `id`, to drive it.
    pub fn connection_mut(&mut self, id: ConnectionId) -> Option<&mut Connection> {
        self.entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .map(|entry| &mut entry.connection)
    }

    /// The tag the connection's credentials were issued with.
    #[must_use]
    pub fn connection_tag(&self, id: ConnectionId) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.tag.as_slice())
    }

    /// Asserts the table's invariants, and each connection's (D16): no
    /// more connections than room for them, IDs in the order they were
    /// given, and one connection per token, which is how packets find it.
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        assert!(self.entries.len() <= self.max_connections);
        assert!(self.opened.len() <= self.max_connections);
        let entries = self.entries.iter().map(|entry| entry.id);
        assert!(
            given_in_order(entries, self.next_id),
            "connections in order"
        );
        let opened = self.opened.iter().copied();
        assert!(given_in_order(opened, self.next_id), "opened in order");
        let mut tokens = BTreeSet::new();
        for entry in &self.entries {
            assert!(entry.token.len() <= MAX_TOKEN_LENGTH);
            assert!(entry.tag.len() <= MAX_TOKEN_LENGTH);
            assert!(
                tokens.insert(entry.token.as_slice()),
                "one connection per token"
            );
            entry.connection.check_invariants();
        }
    }

    /// Forgets connections that are closed, settled, and whose events have
    /// all been polled. Another packet for one opens it afresh. Called
    /// automatically when the table is full.
    pub fn remove_closed(&mut self) {
        self.entries
            .retain(|entry| !(entry.connection.is_settled() && !entry.connection.has_events()));
    }
}
