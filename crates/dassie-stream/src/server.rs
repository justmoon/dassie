//! A STREAM server: issues credentials and routes incoming packets to the
//! connections they belong to.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_ilp::{Address, ErrorCode as IlpErrorCode, Prepare, Reject, Timestamp};
use dassie_oer::Encode;
use rand_core::CryptoRng;

use crate::base64url;
use crate::connection::{AssetDetails, BUFFER_LENGTH, Config, Connection, Role};
use crate::policy::Policy;
use crate::receipt::ReceiptSecret;
use crate::token::{MAX_TOKEN_LENGTH, ServerSecret, TokenContents};

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
        let mut token = [0u8; MAX_TOKEN_LENGTH];
        let contents = TokenContents {
            connection_tag,
            receipt,
        };
        let length = self.secret.issue_token(rng, &contents, &mut token);
        let token = token.get(..length).unwrap_or_default();
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
        assert!(
            buffer.len() >= BUFFER_LENGTH,
            "buffer shorter than BUFFER_LENGTH"
        );
        let mut token = [0u8; MAX_TOKEN_LENGTH];
        let Some(token_length) = self.token_of(prepare.destination, &mut token) else {
            return self.reject(IlpErrorCode::F02_UNREACHABLE, buffer);
        };
        let token = token.get(..token_length).unwrap_or_default();
        let index = match self.entries.iter().position(|entry| entry.token == token) {
            Some(index) => index,
            None => match self.open(token) {
                Ok(index) => index,
                Err(code) => return self.reject(code, buffer),
            },
        };
        let Some(entry) = self.entries.get_mut(index) else {
            return self.reject(IlpErrorCode::T00_INTERNAL_ERROR, buffer);
        };
        entry.connection.handle_prepare(now, rng, prepare, buffer)
    }

    /// Extracts and decodes the token: the first segment after the server's
    /// address. Further segments are ignored, as upstream does; the original
    /// took everything after the address (spec 4 §11.4).
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
        self.opened.push_back(id);
        Ok(self.entries.len() - 1)
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

    /// Forgets closed connections whose events have all been polled.
    /// Another packet for one opens it afresh.
    pub fn remove_closed(&mut self) {
        self.entries
            .retain(|entry| !(entry.connection.is_closed() && !entry.connection.has_events()));
    }
}
