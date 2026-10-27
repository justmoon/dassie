//! Every secret a node derives below its node key, one variant of
//! [`NodeSecret`] each (spec/areas/identity.md#seed-paths).
//!
//! The variants are the only way to derive from the node key, so no crate
//! writes a path of its own, and no two uses can meet at one path. Each
//! names what it is for and takes typed arguments; the path it derives at
//! is its [`Display`](fmt::Display) form. Golden vectors from a generator
//! of the repository's own (`testdata/identity/`) pin every path and its
//! secret, and [`NodeSecret::is_frozen`] says which may never change.

use core::fmt;

use dassie_oer::Alphabet;

use crate::error::Error;
use crate::node_key::NodeKey;
use crate::seed::{DerivedSecret, LoginSecret, fold};

/// Longest segment a caller names, in bytes. Ledger IDs and scheme IDs are
/// a few dozen.
pub const MAX_SEGMENT_BYTES: usize = 64;

/// Characters of a segment: printable ASCII without the space and without
/// `/`, which separates segments.
const SEGMENT_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(0x21, 0x2e)
    .with_range(0x30, 0x7e);

/// A segment of a path that a caller names, such as a ledger ID: 1 to
/// [`MAX_SEGMENT_BYTES`] bytes of printable ASCII, without the space and
/// without `/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment<'a>(&'a str);

impl<'a> Segment<'a> {
    /// Checks `text` as a segment.
    pub fn parse(text: &'a str) -> Result<Self, Error> {
        let valid = (1..=MAX_SEGMENT_BYTES).contains(&text.len())
            && SEGMENT_ALPHABET.contains_all(text.as_bytes());
        if valid {
            Ok(Self(text))
        } else {
            Err(Error::InvalidSeedPath)
        }
    }

    /// The segment as text.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.0
    }
}

/// One of the node's two QUIC endpoints (spec/areas/peer-protocol.md#quic).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuicEndpoint {
    /// The endpoint over UDP.
    Udp,
    /// The endpoint over tunnels through a relay.
    Tunnel,
}

/// A key a node makes for a settlement hub
/// (spec/areas/hubs.md#the-escrow).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubKey {
    /// A validator's key for votes, certificates and receipts.
    Consensus,
    /// A validator's key on the escrow's signer list.
    Escrow,
    /// The parent of a validator's keys for payout sets.
    Payout,
    /// The sponsor's key of the escrow account itself.
    EscrowMaster,
}

impl HubKey {
    const fn name(self) -> &'static str {
        match self {
            Self::Consensus => "consensus",
            Self::Escrow => "escrow",
            Self::Payout => "payout",
            Self::EscrowMaster => "escrow-master",
        }
    }
}

/// A secret below the node key, and so the path it derives at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeSecret<'a> {
    /// `node/login`: the login proof (spec/areas/http.md#login-and-logout).
    Login,
    /// `node/open-payments-client`: the key the node signs its Open
    /// Payments requests with (spec/areas/payments.md#wallet-addresses).
    OpenPaymentsClient,
    /// `node/stream-server/<ledger ID>`: the secret of the STREAM receiver
    /// of a ledger, by the ledger's ID, the name its books have
    /// (spec/areas/stream.md#credentials-and-tokens).
    StreamReceiver {
        /// The ledger's ID, such as `xrpl-testnet-xrp+xrp`.
        ledger: Segment<'a>,
    },
    /// `node/quic/stateless-reset` and `node/quic/tunnel-stateless-reset`:
    /// the key of an endpoint's stateless resets.
    QuicStatelessReset(QuicEndpoint),
    /// `node/quic/connection-ids` and `node/quic/tunnel-connection-ids`:
    /// the key an endpoint makes and knows its connection IDs with.
    QuicConnectionIds(QuicEndpoint),
    /// `node/hub/<network>/nonce/<nonce>/<key>`: a key for the hub of
    /// `nonce` on the XRP Ledger network of ID `network`. The hub's ID
    /// cannot name it, since it is the hash of rules that contain the key,
    /// and the network is in the path because a transaction on the
    /// testnet or mainnet names none
    /// (spec/decisions/hub-keys-by-network.md).
    Hub {
        /// The XRP Ledger's network ID, as the hub's terms name it.
        network: u32,
        /// The nonce the sponsor picked.
        nonce: &'a [u8; 32],
        /// Which key.
        key: HubKey,
    },
    /// `node/hub/member/<hub ID>/account`: a member's account key in a
    /// hub, one per hub so that a member's accounts are not linked. The hub
    /// ID names the network, since the terms it hashes do.
    HubMember {
        /// The hub's ID.
        hub: &'a [u8; 32],
    },
    /// `node/xrpl/<network>/account/<index>`: the entropy of the node's
    /// account `index` on the XRP Ledger network of ID `network`, which
    /// every scheme on the account settles through
    /// (spec/areas/xrpl.md#the-wallet). Named as the account's asset ID
    /// names it: by its network, and its number among the node's there.
    XrplAccount {
        /// The XRP Ledger's network ID: 0 for mainnet, 1 for the testnet.
        network: u32,
        /// Which of the node's accounts on it: the first is 0.
        index: u16,
    },
}

impl NodeSecret<'_> {
    /// Whether the path may never change, because something outside the
    /// release depends on its secret (spec/overview.md#frozen-surfaces):
    /// funds on a ledger or in a hub, a key other wallets know the node by,
    /// or credentials the node handed out. The login is not: the wallet,
    /// `dassie setup` and the node that checks the proof ship together. Nor
    /// are the QUIC keys: a changed one only makes peers wait out the idle
    /// timeout once.
    #[must_use]
    pub const fn is_frozen(&self) -> bool {
        match self {
            Self::OpenPaymentsClient
            | Self::StreamReceiver { .. }
            | Self::Hub { .. }
            | Self::HubMember { .. }
            | Self::XrplAccount { .. } => true,
            Self::Login | Self::QuicStatelessReset(_) | Self::QuicConnectionIds(_) => false,
        }
    }

    /// Calls `f` with the path's segments below `node`.
    fn with_segments<R>(&self, f: impl FnOnce(&[&str]) -> R) -> R {
        let mut decimal = [0; 10];
        let mut hex = [0; 64];
        match *self {
            Self::Login => f(&["login"]),
            Self::OpenPaymentsClient => f(&["open-payments-client"]),
            Self::StreamReceiver { ledger } => f(&["stream-server", ledger.as_str()]),
            Self::QuicStatelessReset(QuicEndpoint::Udp) => f(&["quic", "stateless-reset"]),
            Self::QuicStatelessReset(QuicEndpoint::Tunnel) => {
                f(&["quic", "tunnel-stateless-reset"])
            }
            Self::QuicConnectionIds(QuicEndpoint::Udp) => f(&["quic", "connection-ids"]),
            Self::QuicConnectionIds(QuicEndpoint::Tunnel) => f(&["quic", "tunnel-connection-ids"]),
            Self::Hub {
                network,
                nonce,
                key,
            } => f(&[
                "hub",
                decimal_text(network, &mut decimal),
                "nonce",
                hex_text(nonce, &mut hex),
                key.name(),
            ]),
            Self::HubMember { hub } => f(&["hub", "member", hex_text(hub, &mut hex), "account"]),
            Self::XrplAccount { network, index } => {
                let mut number = [0; 10];
                f(&[
                    "xrpl",
                    decimal_text(network, &mut decimal),
                    "account",
                    decimal_text(u32::from(index), &mut number),
                ])
            }
        }
    }
}

impl fmt::Display for NodeSecret<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.with_segments(|segments| {
            f.write_str("node")?;
            for segment in segments {
                write!(f, "/{segment}")?;
            }
            Ok(())
        })
    }
}

impl NodeKey {
    /// The secret `which`, derived from the node key the way the browser
    /// derives it from the phrase, below `node`.
    #[must_use]
    pub fn secret(&self, which: NodeSecret<'_>) -> DerivedSecret {
        which.with_segments(|segments| fold(self.as_bytes(), segments.iter().copied()))
    }

    /// The login proof secret at `node/login`.
    #[must_use]
    pub fn login_secret(&self) -> LoginSecret {
        LoginSecret::from_bytes(*self.secret(NodeSecret::Login).as_bytes())
    }

    /// The entropy of the node's account `index` on the XRP Ledger network
    /// of ID `network`, at `node/xrpl/<network>/account/<index>`: what
    /// keeps the same phrase pointing at the same funds.
    #[must_use]
    pub fn xrpl_account_entropy(&self, network: u32, index: u16) -> DerivedSecret {
        self.secret(NodeSecret::XrplAccount { network, index })
    }
}

/// `value` in decimal, written into `buffer`.
// Panics only on a broken invariant: a u32 has at most ten digits.
#[allow(clippy::panic)]
fn decimal_text(value: u32, buffer: &mut [u8; 10]) -> &str {
    let mut start = buffer.len();
    let mut rest = value;
    loop {
        start -= 1;
        let digit = u8::try_from(rest % 10).unwrap_or(0);
        if let Some(slot) = buffer.get_mut(start) {
            *slot = b'0' + digit;
        }
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    let Some(Ok(text)) = buffer.get(start..).map(core::str::from_utf8) else {
        panic!("decimal digits are ASCII");
    };
    text
}

/// `bytes` as lowercase hex, written into `buffer`.
// Panics only on a broken invariant: hex digits are ASCII.
#[allow(clippy::panic)]
fn hex_text<'b>(bytes: &[u8; 32], buffer: &'b mut [u8; 64]) -> &'b str {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for ([high, low], byte) in buffer.as_chunks_mut::<2>().0.iter_mut().zip(bytes) {
        *high = DIGITS.get(usize::from(byte >> 4)).copied().unwrap_or(b'0');
        *low = DIGITS
            .get(usize::from(byte & 0x0f))
            .copied()
            .unwrap_or(b'0');
    }
    let Ok(text) = core::str::from_utf8(buffer) else {
        panic!("hex digits are ASCII");
    };
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;
    use std::vec::Vec;

    #[test]
    fn segments_are_bounded_and_have_no_separator() {
        assert!(Segment::parse("xrpl-testnet-xrp+xrp").is_ok());
        assert!(Segment::parse(&"a".repeat(MAX_SEGMENT_BYTES)).is_ok());
        for text in [
            "",
            "a/b",
            "a b",
            "\t",
            "é",
            &"a".repeat(MAX_SEGMENT_BYTES + 1),
        ] {
            assert_eq!(
                Segment::parse(text),
                Err(Error::InvalidSeedPath),
                "{text:?}"
            );
        }
    }

    #[test]
    fn numbers_and_nonces_are_written_one_way() {
        let mut buffer = [0; 10];
        assert_eq!(decimal_text(0, &mut buffer), "0");
        assert_eq!(decimal_text(21_338, &mut buffer), "21338");
        assert_eq!(decimal_text(u32::MAX, &mut buffer), "4294967295");
        let mut hex = [0; 64];
        let mut bytes = [0xab; 32];
        bytes[0] = 0x01;
        let text = hex_text(&bytes, &mut hex);
        assert!(text.starts_with("01abab") && text.len() == 64);
    }

    // No two secrets share a path: the uses never meet, and the hubs of one
    // nonce on two networks have keys of their own.
    #[test]
    fn every_path_is_its_own() {
        let ledger = Segment::parse("xrpl-testnet-xrp+xrp").unwrap();
        let mut all = Vec::from([
            NodeSecret::Login,
            NodeSecret::OpenPaymentsClient,
            NodeSecret::StreamReceiver { ledger },
            NodeSecret::QuicStatelessReset(QuicEndpoint::Udp),
            NodeSecret::QuicStatelessReset(QuicEndpoint::Tunnel),
            NodeSecret::QuicConnectionIds(QuicEndpoint::Udp),
            NodeSecret::QuicConnectionIds(QuicEndpoint::Tunnel),
            NodeSecret::HubMember { hub: &[2; 32] },
            NodeSecret::XrplAccount {
                network: 1,
                index: 0,
            },
            NodeSecret::XrplAccount {
                network: 0,
                index: 0,
            },
            NodeSecret::XrplAccount {
                network: 1,
                index: 1,
            },
        ]);
        for network in [0, 1, 2] {
            for key in [
                HubKey::Consensus,
                HubKey::Escrow,
                HubKey::Payout,
                HubKey::EscrowMaster,
            ] {
                all.push(NodeSecret::Hub {
                    network,
                    nonce: &[2; 32],
                    key,
                });
            }
        }
        let key = NodeKey::from_bytes(&[1; 32]);
        let paths: std::collections::BTreeSet<_> = all.iter().map(ToString::to_string).collect();
        let secrets: std::collections::BTreeSet<_> = all
            .iter()
            .map(|which| *key.secret(*which).as_bytes())
            .collect();
        assert_eq!(paths.len(), all.len());
        assert_eq!(secrets.len(), all.len());
        assert!(paths.iter().all(|path| path.starts_with("node/")));
    }
}
