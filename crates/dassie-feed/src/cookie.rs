//! The address cookie (spec/areas/rate-feed.md#the-cookie).
//!
//! UDP's source address is whatever the sender writes. Were a node to send
//! its feed to every address a Subscribe came from, anyone could aim a
//! node's feeds at a victim. So an address is subscribed only once it has
//! shown that it receives there: the node answers a Subscribe without a
//! valid cookie with a Challenge carrying one, and only a Subscribe that
//! sends it back is taken. The cookie is a MAC of the address and the time,
//! under a secret only the node knows, so the node keeps nothing for an
//! address until it is proven, and nobody can compute a cookie for an
//! address whose datagrams they do not receive.
//!
//! Time is cut into epochs, and a cookie is taken in the epoch it was made
//! in and in the next, so it lives between one and two epochs.

use core::fmt;
use core::net::{IpAddr, SocketAddr};

use dassie_kit::MINUTE_MS;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::wire::COOKIE_BYTES;

/// How long an epoch of the cookie lasts.
pub const COOKIE_EPOCH_MS: u64 = 2 * MINUTE_MS;

/// What the MAC is over first, so that it never equals a MAC made for
/// anything else under the same secret.
const LABEL: &[u8] = b"dassie feed cookie v1";

const _: () = assert!(COOKIE_BYTES <= 32, "a cookie is a truncated HMAC-SHA256");

/// A cookie, as a node hands it out and a subscriber sends it back.
/// Compared in constant time.
#[derive(Clone, Copy)]
pub struct Cookie([u8; COOKIE_BYTES]);

impl Cookie {
    /// The cookie of these bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; COOKIE_BYTES]) -> Self {
        Self(bytes)
    }

    /// Its bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; COOKIE_BYTES] {
        &self.0
    }
}

impl PartialEq for Cookie {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for Cookie {}

impl fmt::Debug for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cookie(..)")
    }
}

/// Whether a cookie is the one for an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookieCheck {
    /// It was made in this epoch.
    Current,
    /// It was made in the epoch before: still taken, but the subscriber
    /// should get this epoch's.
    Previous,
    /// It is not one this node made for the address in either.
    Invalid,
}

/// The secret cookies are made under. The node draws one when it starts and
/// keeps it in memory only, so a restart voids every cookie, and
/// subscribers are challenged again. Wiped on drop.
pub struct CookieSecret([u8; 32]);

impl CookieSecret {
    /// The secret of these bytes, which the caller draws from a random
    /// source.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The cookie for `address` at `now_ms`.
    #[must_use]
    pub fn issue(&self, address: SocketAddr, now_ms: u64) -> Cookie {
        self.cookie(address, epoch(now_ms))
    }

    /// Whether `cookie` is the one for `address` in this epoch or the one
    /// before.
    #[must_use]
    pub fn check(&self, address: SocketAddr, cookie: &Cookie, now_ms: u64) -> CookieCheck {
        let current = epoch(now_ms);
        if self.cookie(address, current) == *cookie {
            return CookieCheck::Current;
        }
        let previous = current.checked_sub(1);
        if previous.is_some_and(|previous| self.cookie(address, previous) == *cookie) {
            return CookieCheck::Previous;
        }
        CookieCheck::Invalid
    }

    /// The cookie for `address` in `epoch`: HMAC-SHA256 under the secret
    /// over the label, the epoch and the address, truncated. The address is
    /// taken in its canonical form, so an IPv4 address mapped into IPv6 has
    /// the cookie of the IPv4 address.
    // HMAC takes keys of any length, and the digest is longer than a
    // cookie, so neither can panic.
    #[allow(clippy::panic)]
    fn cookie(&self, address: SocketAddr, epoch: u64) -> Cookie {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&self.0) else {
            panic!("HMAC accepts keys of any length");
        };
        mac.update(LABEL);
        mac.update(&epoch.to_be_bytes());
        match address.ip().to_canonical() {
            IpAddr::V4(v4) => {
                mac.update(&[4]);
                mac.update(&v4.octets());
            }
            IpAddr::V6(v6) => {
                mac.update(&[6]);
                mac.update(&v6.octets());
            }
        }
        mac.update(&address.port().to_be_bytes());
        let digest: [u8; 32] = mac.finalize().into_bytes().into();
        let Some(cookie) = digest.first_chunk::<COOKIE_BYTES>() else {
            panic!("the digest is longer than a cookie");
        };
        Cookie(*cookie)
    }
}

impl Drop for CookieSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for CookieSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CookieSecret(<redacted>)")
    }
}

/// The epoch `now_ms` falls in.
const fn epoch(now_ms: u64) -> u64 {
    now_ms / COOKIE_EPOCH_MS
}
