//! Payment pointers (`$host/path`) and wallet address URLs.
//!
//! Open Payments names accounts by wallet address URL. Payment pointers
//! remain as a shorter way to write one: `$host/path` is
//! `https://host/path`, and a bare `$host` is `https://host/.well-known/pay`.
//! What an owner sends to is either, or an incoming payment's URL: a
//! payment request.

use alloc::string::String;

use crate::uri::{MAX_URI_BYTES, Uri};

/// The path a bare `$host` stands for.
pub const WELL_KNOWN_PATH: &str = "/.well-known/pay";

/// The input is neither a payment pointer nor a wallet address URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidWalletAddress;

/// The wallet address URL a payment pointer or URL names.
///
/// # Errors
///
/// If the input is neither, or the URL is not one a wallet address may
/// have: `https`, with a host, and no user information, query or fragment.
pub fn wallet_address_url(input: &str) -> Result<String, InvalidWalletAddress> {
    let input = input.trim();
    let url = match input.strip_prefix('$') {
        Some(pointer) => {
            let mut url = String::from("https://");
            url.push_str(pointer);
            let bare = !pointer.contains('/')
                || pointer.ends_with('/') && pointer.matches('/').count() == 1;
            if bare {
                url.truncate(url.trim_end_matches('/').len());
                url.push_str(WELL_KNOWN_PATH);
            }
            url
        }
        None => String::from(input),
    };
    check_wallet_address(&url)?;
    Ok(url)
}

/// What a payment goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recipient {
    /// A wallet address, where the payment's incoming payment is created.
    WalletAddress(String),
    /// An incoming payment that exists already: a payment request.
    IncomingPayment(String),
}

impl Recipient {
    /// The recipient a payment pointer, wallet address URL or incoming
    /// payment URL names.
    ///
    /// # Errors
    ///
    /// If the input is none of them; see [`wallet_address_url`].
    pub fn parse(input: &str) -> Result<Self, InvalidWalletAddress> {
        if input.len() > MAX_URI_BYTES {
            return Err(InvalidWalletAddress);
        }
        let url = wallet_address_url(input)?;
        if crate::gnap::is_receiver(&url) {
            Ok(Self::IncomingPayment(url))
        } else {
            Ok(Self::WalletAddress(url))
        }
    }

    /// Its URL.
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Self::WalletAddress(url) | Self::IncomingPayment(url) => url,
        }
    }
}

/// A wallet address URL written as a payment pointer: `$host` for the
/// well-known path, `$host/path` otherwise. None if it is not a wallet
/// address, or has no path, which no pointer names: `$host` is the
/// well-known path.
#[must_use]
pub fn payment_pointer(url: &str) -> Option<String> {
    check_wallet_address(url).ok()?;
    let uri = Uri::parse(url).ok()?;
    if uri.path.is_empty() || uri.path == "/" {
        return None;
    }
    let mut pointer = String::from("$");
    pointer.push_str(&uri.authority.to_ascii_lowercase());
    if uri.path != WELL_KNOWN_PATH {
        pointer.push_str(uri.path);
    }
    Some(pointer)
}

/// Checks that a URL may be a wallet address.
///
/// # Errors
///
/// If it is not `https`, has no host, or has user information, a query or
/// a fragment.
pub fn check_wallet_address(url: &str) -> Result<(), InvalidWalletAddress> {
    let uri = Uri::parse(url).map_err(|_| InvalidWalletAddress)?;
    if uri.scheme == "https" && uri.query.is_none() && uri.fragment.is_none() {
        Ok(())
    } else {
        Err(InvalidWalletAddress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_payment_pointers() {
        assert_eq!(
            wallet_address_url("$d2.localhost").as_deref(),
            Ok("https://d2.localhost/.well-known/pay")
        );
        assert_eq!(
            wallet_address_url("$d2.localhost/").as_deref(),
            Ok("https://d2.localhost/.well-known/pay")
        );
        assert_eq!(
            wallet_address_url("$d2.localhost:5002/pay/stub").as_deref(),
            Ok("https://d2.localhost:5002/pay/stub")
        );
        assert_eq!(
            wallet_address_url(" https://ilp.example/alice ").as_deref(),
            Ok("https://ilp.example/alice")
        );
    }

    #[test]
    fn writes_wallet_addresses_as_payment_pointers() {
        for (url, pointer) in [
            (
                "https://d2.localhost:5002/.well-known/pay",
                "$d2.localhost:5002",
            ),
            ("https://d2.localhost/pay/stub", "$d2.localhost/pay/stub"),
        ] {
            assert_eq!(payment_pointer(url).as_deref(), Some(pointer), "{url}");
            // Both ways, the same.
            assert_eq!(wallet_address_url(pointer).as_deref(), Ok(url));
        }
        assert_eq!(
            payment_pointer("https://ILP.example/alice").as_deref(),
            Some("$ilp.example/alice")
        );
        // `$ilp.example` is the well-known path, not these.
        assert_eq!(payment_pointer("https://ilp.example"), None);
        assert_eq!(payment_pointer("https://ilp.example/"), None);
        assert_eq!(payment_pointer("http://ilp.example/a"), None);
    }

    #[test]
    fn tells_payment_requests_from_wallet_addresses() {
        assert_eq!(
            Recipient::parse("$d2.localhost:5002"),
            Ok(Recipient::WalletAddress(
                "https://d2.localhost:5002/.well-known/pay".into()
            ))
        );
        assert_eq!(
            Recipient::parse(" https://d2.localhost/incoming-payments/abc "),
            Ok(Recipient::IncomingPayment(
                "https://d2.localhost/incoming-payments/abc".into()
            ))
        );
        assert_eq!(
            Recipient::parse("$wallet.example/alice/incoming-payments/1"),
            Ok(Recipient::IncomingPayment(
                "https://wallet.example/alice/incoming-payments/1".into()
            ))
        );
        // No id: a wallet address that happens to have the word in its path.
        assert_eq!(
            Recipient::parse("https://wallet.example/incoming-payments/"),
            Ok(Recipient::WalletAddress(
                "https://wallet.example/incoming-payments/".into()
            ))
        );
        for input in [
            "",
            "alice",
            "http://d2.localhost/incoming-payments/abc",
            "https://d2.localhost/incoming-payments/abc?x=1",
        ] {
            assert_eq!(
                Recipient::parse(input),
                Err(InvalidWalletAddress),
                "{input}"
            );
        }
        let long = alloc::format!("https://wallet.example/{}", "a".repeat(MAX_URI_BYTES));
        assert_eq!(Recipient::parse(&long), Err(InvalidWalletAddress));
    }

    #[test]
    fn refuses_what_cannot_be_a_wallet_address() {
        for input in [
            "",
            "$",
            "alice",
            "http://ilp.example/alice",
            "https://ilp.example/a?b",
            "$ilp.example/a#b",
            "$user@ilp.example",
        ] {
            assert_eq!(
                wallet_address_url(input),
                Err(InvalidWalletAddress),
                "{input}"
            );
        }
    }
}
