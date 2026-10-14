//! Payment pointers (`$host/path`) and wallet address URLs.
//!
//! Open Payments names accounts by wallet address URL. Payment pointers
//! remain as a shorter way to write one: `$host/path` is
//! `https://host/path`, and a bare `$host` is `https://host/.well-known/pay`.

use alloc::string::String;

use crate::uri::Uri;

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
