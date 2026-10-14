//! Absolute `http(s)` URLs, split into the parts that signatures and the
//! Open Payments rules look at. This is not a general URL parser: it takes
//! printable ASCII only and refuses user information, so that two parties
//! can never read one URL differently.

use alloc::string::String;
use alloc::vec::Vec;

/// The longest URL accepted, in bytes.
pub const MAX_URI_BYTES: usize = 2_048;
/// The most pairs a query string may have.
pub const MAX_QUERY_PAIRS: usize = 16;

/// The input is not an absolute URL of the accepted form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidUri;

/// An absolute URL, as parts of the original string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uri<'a> {
    /// `http` or `https`, in lower case.
    pub scheme: &'a str,
    /// Host and optional port.
    pub authority: &'a str,
    /// The path, empty when the URL has none.
    pub path: &'a str,
    /// The query without its `?`, if there is one.
    pub query: Option<&'a str>,
    /// The fragment without its `#`, if there is one.
    pub fragment: Option<&'a str>,
}

impl<'a> Uri<'a> {
    /// Parses `input`.
    ///
    /// # Errors
    ///
    /// If it is not an absolute `http` or `https` URL of printable ASCII
    /// with a host and no user information.
    pub fn parse(input: &'a str) -> Result<Self, InvalidUri> {
        let printable = input.bytes().all(|byte| matches!(byte, b'!'..=b'~'));
        if input.len() > MAX_URI_BYTES || !printable {
            return Err(InvalidUri);
        }
        let (scheme, rest) = input.split_once("://").ok_or(InvalidUri)?;
        if scheme != "https" && scheme != "http" {
            return Err(InvalidUri);
        }
        let (rest, fragment) = match rest.split_once('#') {
            Some((rest, fragment)) => (rest, Some(fragment)),
            None => (rest, None),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((rest, query)) => (rest, Some(query)),
            None => (rest, None),
        };
        let (authority, path) = match rest.find('/') {
            Some(index) => rest.split_at(index),
            None => (rest, ""),
        };
        if authority.is_empty() || authority.contains('@') || authority.starts_with(':') {
            return Err(InvalidUri);
        }
        Ok(Self {
            scheme,
            authority,
            path,
            query,
            fragment,
        })
    }

    /// The host without the port, in lower case.
    #[must_use]
    pub fn host(&self) -> String {
        let host = if self.authority.ends_with(']') {
            self.authority
        } else {
            self.authority
                .rsplit_once(':')
                .map_or(self.authority, |(host, _)| host)
        };
        host.to_ascii_lowercase()
    }

    /// `scheme://authority` with both in lower case.
    #[must_use]
    pub fn origin(&self) -> String {
        let mut origin = String::from(self.scheme);
        origin.push_str("://");
        let authority = self.authority.to_ascii_lowercase();
        // The scheme's own port says nothing more.
        let default = if self.scheme == "https" {
            ":443"
        } else {
            ":80"
        };
        origin.push_str(authority.strip_suffix(default).unwrap_or(&authority));
        origin
    }
}

/// Splits a query string into decoded pairs: `+` is a space and `%XX` a
/// byte, as forms encode them. A key without `=` has an empty value.
///
/// # Errors
///
/// If there are more than [`MAX_QUERY_PAIRS`] pairs, or an escape or the
/// decoded text is invalid.
pub fn query_pairs(query: &str) -> Result<Vec<(String, String)>, InvalidUri> {
    let mut pairs = Vec::new();
    for part in query.split('&').filter(|part| !part.is_empty()) {
        if pairs.len() >= MAX_QUERY_PAIRS {
            return Err(InvalidUri);
        }
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        pairs.push((percent_decode(key)?, percent_decode(value)?));
    }
    Ok(pairs)
}

fn percent_decode(input: &str) -> Result<String, InvalidUri> {
    let mut bytes = Vec::with_capacity(input.len());
    let mut rest = input.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let (high, low) = match rest {
                    [high, low, tail @ ..] => {
                        rest = tail;
                        (hex(*high)?, hex(*low)?)
                    }
                    _ => return Err(InvalidUri),
                };
                bytes.push(high << 4 | low);
            }
            _ => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).map_err(|_| InvalidUri)
}

fn hex(digit: u8) -> Result<u8, InvalidUri> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(InvalidUri),
    }
}

/// Encodes a query value: everything but unreserved characters as `%XX`.
#[must_use]
pub fn percent_encode(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push('%');
            for nibble in [byte >> 4, byte & 0xf] {
                let digit = HEX.get(usize::from(nibble)).copied().unwrap_or(b'0');
                output.push(char::from(digit));
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn splits_a_url() {
        let uri = Uri::parse("https://Example.com:5001/foo/bar?a=1&b#top").unwrap();
        assert_eq!(uri.scheme, "https");
        assert_eq!(uri.authority, "Example.com:5001");
        assert_eq!(uri.host(), "example.com");
        assert_eq!(uri.origin(), "https://example.com:5001");
        assert_eq!(uri.path, "/foo/bar");
        assert_eq!(uri.query, Some("a=1&b"));
        assert_eq!(uri.fragment, Some("top"));
        assert_eq!(Uri::parse("https://[::1]:443/").unwrap().host(), "[::1]");
        assert_eq!(Uri::parse("https://[::1]/").unwrap().host(), "[::1]");
        assert_eq!(Uri::parse("https://example.com").unwrap().path, "");
        // The scheme's own port is the same origin.
        assert_eq!(
            Uri::parse("https://Example.com:443/a").unwrap().origin(),
            "https://example.com"
        );
        assert_eq!(
            Uri::parse("https://example.com:4430/").unwrap().origin(),
            "https://example.com:4430"
        );
        assert_eq!(
            Uri::parse("http://example.com:80/").unwrap().origin(),
            "http://example.com"
        );
    }

    #[test]
    fn refuses_other_urls() {
        for input in [
            "example.com/foo",
            "ftp://example.com/",
            "https://user@example.com/",
            "https:///path",
            "https://exa mple.com/",
            "https://exämple.com/",
        ] {
            assert_eq!(Uri::parse(input), Err(InvalidUri), "{input}");
        }
    }

    #[test]
    fn decodes_and_encodes_queries() {
        let pairs = query_pairs("wallet-address=https%3A%2F%2Fa.example%2Fpay&first=5&x").unwrap();
        assert_eq!(
            pairs,
            vec![
                ("wallet-address".into(), "https://a.example/pay".into()),
                ("first".into(), "5".into()),
                ("x".into(), String::new()),
            ]
        );
        assert_eq!(
            percent_encode("https://a.example/pay"),
            "https%3A%2F%2Fa.example%2Fpay"
        );
        assert_eq!(query_pairs("a=%2"), Err(InvalidUri));
        assert_eq!(query_pairs("a=%ff"), Err(InvalidUri));
    }
}
