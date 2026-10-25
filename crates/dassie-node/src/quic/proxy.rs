//! Explicit HTTP proxies for the tunnels a node dials
//! (spec/areas/peer-protocol.md#quic-over-websocket).
//!
//! Some networks let nothing out but through an HTTP proxy. A tunnel then
//! asks the proxy `HTTPS_PROXY` names to `CONNECT` it to the core node's
//! host and port, and runs its TLS and WebSocket handshakes over the stream
//! the proxy opens, as a browser does; `NO_PROXY` names the hosts it dials
//! directly. Only tunnels go through it: QUIC over UDP cannot.
//!
//! The proxy reaches the host by the name in its URL, so a proxy that lets
//! only some names out sees the name it judges. The node has resolved that
//! name already, for UDP and for the guard on private addresses
//! (spec/areas/peer-protocol.md#outbound-requests), but the proxy resolves
//! it again for itself: what it reaches is its network's to allow.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

use base64::Engine as _;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Most bytes of a proxy's answer to `CONNECT`, its blank line included. An
/// answer is a status line and a few headers.
pub const MAX_HEAD_BYTES: usize = 8 * 1024;

/// Most entries of `NO_PROXY` a node keeps; it skips those past them.
pub const MAX_NO_PROXY_ENTRIES: usize = 256;

/// Most characters of a skipped entry of `NO_PROXY` a warning shows.
const MAX_SHOWN_ENTRY_CHARS: usize = 64;

/// Most bytes of a proxy's URL.
const MAX_URL_BYTES: usize = 2048;

/// Most bytes of a host name, as DNS has them.
const MAX_HOST_BYTES: usize = 253;

/// A proxy's port when its URL names none: HTTP's.
const DEFAULT_PORT: u16 = 80;

/// An HTTP proxy the tunnels dial through, and the hosts they dial directly.
#[derive(Clone, PartialEq, Eq)]
pub struct Proxy {
    host: String,
    port: u16,
    /// `Proxy-Authorization`'s value, when the URL names a user.
    authorization: Option<String>,
    bypass: NoProxy,
}

impl Proxy {
    /// The proxy at `url`, `http://[user[:password]@]host[:port][/]` (the
    /// scheme may be left out), bypassed for the hosts `bypass` lists. User
    /// and password may be percent-encoded, and are sent with Basic auth.
    ///
    /// # Errors
    ///
    /// The URL is not one of an HTTP proxy, as a problem that never repeats
    /// its user or password.
    pub fn parse(url: &str, bypass: NoProxy) -> Result<Self, String> {
        if url.len() > MAX_URL_BYTES {
            return Err(format!("is longer than {MAX_URL_BYTES} bytes"));
        }
        let rest = match url.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
            Some((scheme, _)) => {
                return Err(format!(
                    "must be an http:// URL: a proxy over {scheme:?} is not supported"
                ));
            }
            None => url,
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (user, address) = match rest.rsplit_once('@') {
            Some((user, address)) => (Some(user), address),
            None => (None, rest),
        };
        if address.contains(['/', '?', '#']) {
            return Err("must name only a host and a port".into());
        }
        let (host, port) = host_and_port(address)?;
        let authorization = user.map(basic).transpose()?;
        Ok(Self {
            host,
            port,
            authorization,
            bypass,
        })
    }

    /// Whether a tunnel to `host` goes through the proxy: unless `NO_PROXY`
    /// lists it.
    #[must_use]
    pub fn applies_to(&self, host: &str) -> bool {
        !self.bypass.matches(host)
    }

    /// A TCP connection to `host` and `port` through the proxy, once it
    /// answered `CONNECT` with a 2xx. The caller bounds its time.
    ///
    /// # Errors
    ///
    /// The proxy cannot be reached, refuses, or answers what is not HTTP.
    pub(super) async fn connect(&self, (host, port): (&str, u16)) -> Result<TcpStream, String> {
        // The host came out of a peer's URL: nothing in it may end a line.
        let fits = |byte: u8| byte.is_ascii_alphanumeric() || b".-_:".contains(&byte);
        if host.is_empty() || !host.bytes().all(fits) {
            return Err("not a host a proxy can be asked for".into());
        }
        let mut tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| format!("the proxy cannot be reached: {e}"))?;
        let _ = tcp.set_nodelay(true);
        tcp.write_all(self.request((host, port)).as_bytes())
            .await
            .map_err(|e| format!("the proxy: {e}"))?;
        let head = read_head(&mut tcp).await?;
        match status(&head)? {
            200..=299 => Ok(tcp),
            refused => Err(format!("the proxy answered {refused}")),
        }
    }

    /// The `CONNECT` request for `host` and `port`.
    fn request(&self, (host, port): (&str, u16)) -> String {
        let authority = authority(host, port);
        let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
        if let Some(authorization) = &self.authorization {
            request.push_str("Proxy-Authorization: ");
            request.push_str(authorization);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        request
    }
}

/// Its address, never its credentials.
impl fmt::Display for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&authority(&self.host, self.port))
    }
}

/// Its address and whether it has credentials, never what they are.
impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Proxy")
            .field("address", &self.to_string())
            .field("authorization", &self.authorization.is_some())
            .field("bypass", &self.bypass)
            .finish()
    }
}

/// `host` and `port` as an authority, an IPv6 address in brackets.
fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// A proxy URL's `host[:port]`, an IPv6 address in brackets.
fn host_and_port(address: &str) -> Result<(String, u16), String> {
    let (host, port) = match address.strip_prefix('[') {
        Some(rest) => {
            let (ip, after) = rest.split_once(']').ok_or("has an unclosed '['")?;
            let ip: Ipv6Addr = ip.parse().map_err(|_| "has an invalid IPv6 address")?;
            let port = match after {
                "" => None,
                after => Some(after.strip_prefix(':').ok_or("has an invalid port")?),
            };
            (ip.to_string(), port)
        }
        None => match address.split_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (address.to_owned(), None),
        },
    };
    let port = match port {
        None => DEFAULT_PORT,
        Some(port) => port
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or("has an invalid port")?,
    };
    if !is_host_name(&host) && host.parse::<IpAddr>().is_err() {
        return Err("has an invalid host".into());
    }
    Ok((host, port))
}

/// Whether `name` is a host name: letters, digits, `-`, `_` and dots, as
/// long as DNS takes.
fn is_host_name(name: &str) -> bool {
    let fits = |byte: u8| byte.is_ascii_alphanumeric() || b".-_".contains(&byte);
    !name.is_empty() && name.len() <= MAX_HOST_BYTES && name.bytes().all(fits)
}

/// `Proxy-Authorization`'s Basic value for `user[:password]`, each
/// percent-encoded.
fn basic(user: &str) -> Result<String, String> {
    let (user, password) = user.split_once(':').unwrap_or((user, ""));
    let error = "has a user or password that is not percent-encoded";
    let user = percent_decoded(user).ok_or(error)?;
    let password = percent_decoded(password).ok_or(error)?;
    // Basic auth ends the user at its first colon.
    if user.is_empty() || user.contains(&b':') {
        return Err("has an empty user, or one with a colon".into());
    }
    let mut credentials = user;
    credentials.push(b':');
    credentials.extend_from_slice(&password);
    let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
    Ok(format!("Basic {encoded}"))
}

/// `text` with each `%XX` as its byte; `None` when a `%` is not followed by
/// two hex digits.
fn percent_decoded(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let digits = std::str::from_utf8(hex).ok()?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            decoded.push(u8::from_str_radix(digits, 16).ok()?);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    Some(decoded)
}

/// The hosts tunnels dial directly, though a proxy is set: `NO_PROXY`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoProxy {
    /// `*`: every host.
    all: bool,
    entries: Vec<Bypass>,
}

/// A host or hosts `NO_PROXY` lists.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Bypass {
    /// A name, lowercase, and every name beneath it.
    Domain(String),
    /// An IP address, as a host literally.
    Ip(IpAddr),
    /// The IP addresses in a network, as hosts literally: its address and
    /// the bits of its prefix.
    Network(IpAddr, u8),
}

impl NoProxy {
    /// The hosts `text` lists, comma-separated: `*` for all, names (each
    /// with every name beneath it; a leading `.` or `*.` is the same), IP
    /// addresses and networks in CIDR notation; and, if it skipped any, a
    /// problem to warn of. It skips an entry that is none of these, such as
    /// a `host:port` that only some programs read, and those past the
    /// first [`MAX_NO_PROXY_ENTRIES`]: the variable is set for other
    /// programs too, so the node takes what it can read.
    #[must_use]
    pub fn parse(text: &str) -> (Self, Option<String>) {
        let mut bypass = Self::default();
        let mut unread = None;
        let mut unread_count = 0_usize;
        let mut past_count = 0_usize;
        for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if entry == "*" {
                bypass.all = true;
            } else if bypass.entries.len() == MAX_NO_PROXY_ENTRIES {
                past_count += 1;
            } else if let Some(parsed) = parse_entry(entry) {
                bypass.entries.push(parsed);
            } else {
                unread = unread.or(Some(entry));
                unread_count += 1;
            }
        }
        assert!(bypass.entries.len() <= MAX_NO_PROXY_ENTRIES);
        let mut problems = Vec::with_capacity(2);
        if let Some(entry) = unread {
            let shown: String = entry.chars().take(MAX_SHOWN_ENTRY_CHARS).collect();
            problems.push(format!(
                "skips {unread_count} entries it cannot read, such as {shown:?}"
            ));
        }
        if past_count > 0 {
            problems.push(format!(
                "skips {past_count} entries past the first {MAX_NO_PROXY_ENTRIES}"
            ));
        }
        let problem = (!problems.is_empty()).then(|| problems.join(", and "));
        (bypass, problem)
    }

    /// Whether `host`, a name or an IP address, is dialled directly.
    #[must_use]
    pub fn matches(&self, host: &str) -> bool {
        if self.all {
            return true;
        }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let ip = unbracketed(&host)
            .parse::<IpAddr>()
            .ok()
            .map(|ip| ip.to_canonical());
        self.entries.iter().any(|entry| match entry {
            Bypass::Domain(domain) => {
                host == *domain
                    || host
                        .strip_suffix(domain.as_str())
                        .is_some_and(|beneath| beneath.ends_with('.'))
            }
            Bypass::Ip(listed) => ip == Some(*listed),
            Bypass::Network(network, bits) => ip.is_some_and(|ip| within(ip, *network, *bits)),
        })
    }
}

/// One entry of `NO_PROXY`, other than `*`.
fn parse_entry(entry: &str) -> Option<Bypass> {
    if let Some((network, bits)) = entry.split_once('/') {
        let network = unbracketed(network).parse::<IpAddr>().ok()?;
        let bits: u8 = bits.parse().ok()?;
        let most = if network.is_ipv4() { 32 } else { 128 };
        return (bits <= most).then_some(Bypass::Network(network, bits));
    }
    if let Ok(ip) = unbracketed(entry).parse::<IpAddr>() {
        return Some(Bypass::Ip(ip.to_canonical()));
    }
    let name = entry
        .strip_prefix("*.")
        .or_else(|| entry.strip_prefix('.'))
        .unwrap_or(entry)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    is_host_name(&name).then_some(Bypass::Domain(name))
}

/// `host` without the brackets around an IPv6 address.
fn unbracketed(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
}

/// Whether `ip` is in the network of `network`'s first `bits` bits, of the
/// same family.
fn within(ip: IpAddr, network: IpAddr, bits: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(network)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(bits)).unwrap_or(0);
            u32::from(ip) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(network)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(bits)).unwrap_or(0);
            u128::from(ip) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

/// The proxy's answer, up to and with its blank line, at most
/// [`MAX_HEAD_BYTES`]. Nothing may follow it: the server behind speaks only
/// once our TLS has, so bytes past the head are not a proxy's answer.
async fn read_head(tcp: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>, String> {
    let mut head = Vec::with_capacity(512);
    let mut buffer = [0; 512];
    loop {
        let room = MAX_HEAD_BYTES.saturating_sub(head.len()).min(buffer.len());
        let into = buffer.get_mut(..room).filter(|into| !into.is_empty());
        let into = into
            .ok_or_else(|| format!("the proxy's answer is longer than {MAX_HEAD_BYTES} bytes"))?;
        let read = tcp
            .read(into)
            .await
            .map_err(|e| format!("the proxy: {e}"))?;
        let read = into.get(..read).unwrap_or_default();
        if read.is_empty() {
            return Err("the proxy closed the connection before it answered".into());
        }
        head.extend_from_slice(read);
        if let Some(end) = head_end(&head) {
            if end != head.len() {
                return Err("the proxy sent more than its answer".into());
            }
            return Ok(head);
        }
    }
}

/// Where a response head ends in `bytes`, past its blank line, if it does.
fn head_end(bytes: &[u8]) -> Option<usize> {
    let at = bytes.windows(4).position(|four| four == b"\r\n\r\n")?;
    Some(at + 4)
}

/// The status code of a response head: `HTTP/1.x` and three digits.
fn status(head: &[u8]) -> Result<u16, String> {
    let error = || "the proxy's answer is not HTTP".to_owned();
    let line_end = head
        .windows(2)
        .position(|two| two == b"\r\n")
        .ok_or_else(error)?;
    let line = head.get(..line_end).ok_or_else(error)?;
    let line = std::str::from_utf8(line).map_err(|_| error())?;
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0")
        || code.len() != 3
        || !code.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(error());
    }
    code.parse().map_err(|_| error())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(url: &str) -> Result<Proxy, String> {
        Proxy::parse(url, NoProxy::default())
    }

    #[test]
    fn a_proxy_url_names_host_port_and_credentials() {
        let plain = proxy("http://proxy.corp:3128").unwrap();
        assert_eq!((plain.host.as_str(), plain.port), ("proxy.corp", 3128));
        assert_eq!(plain.authorization, None);
        // The scheme may be left out, and a slash end it; HTTP's port is the
        // default.
        let bare = proxy("proxy.corp/").unwrap();
        assert_eq!((bare.host.as_str(), bare.port), ("proxy.corp", 80));
        let ipv6 = proxy("HTTP://[::1]:8080").unwrap();
        assert_eq!((ipv6.host.as_str(), ipv6.port), ("::1", 8080));
        assert_eq!(ipv6.to_string(), "[::1]:8080");
        // Basic auth, of credentials percent-decoded: "Aladdin:open sesame".
        let user = proxy("http://Aladdin:open%20sesame@10.0.0.1:3128").unwrap();
        assert_eq!(
            user.authorization.as_deref(),
            Some("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==")
        );
        // A password may hold an `@`, encoded or not.
        let at = proxy("http://u:p@ss@proxy:1").unwrap();
        assert_eq!(at.authorization.as_deref(), Some("Basic dTpwQHNz"));
        // A user alone has an empty password.
        let alone = proxy("http://u@proxy:1").unwrap();
        assert_eq!(alone.authorization.as_deref(), Some("Basic dTo="));
    }

    #[test]
    fn urls_that_name_no_http_proxy_are_refused() {
        for url in [
            "https://proxy:443",
            "socks5://proxy:1080",
            "http://proxy:3128/path",
            "http://proxy:0",
            "http://proxy:99999",
            "http://proxy:port",
            "http://",
            "http://[::1",
            "http://[::1]3128",
            "http://pro xy:1",
            "http://:secret@proxy:1",
            "http://user:se%2xret@proxy:1",
        ] {
            assert!(proxy(url).is_err(), "{url}");
        }
        // Its problem never repeats the password.
        let problem = proxy("http://user:hunter2@proxy:x").unwrap_err();
        assert!(!problem.contains("hunter2"), "{problem}");
        // Nor does its Debug.
        let debug = format!("{:?}", proxy("http://user:hunter2@proxy:1").unwrap());
        assert!(
            !debug.contains("hunter2") && !debug.contains("Basic"),
            "{debug}"
        );
    }

    #[test]
    fn the_connect_request_names_the_host_twice_and_the_credentials() {
        let user = proxy("http://Aladdin:open%20sesame@proxy:3128").unwrap();
        assert_eq!(
            user.request(("node.example", 443)),
            "CONNECT node.example:443 HTTP/1.1\r\nHost: node.example:443\r\n\
             Proxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n\r\n"
        );
        let plain = proxy("http://proxy:3128").unwrap();
        assert_eq!(
            plain.request(("2001:db8::1", 8443)),
            "CONNECT [2001:db8::1]:8443 HTTP/1.1\r\nHost: [2001:db8::1]:8443\r\n\r\n"
        );
    }

    /// The hosts `text` lists, which it reads whole.
    fn no_proxy(text: &str) -> NoProxy {
        let (bypass, problem) = NoProxy::parse(text);
        assert_eq!(problem, None, "{text}");
        bypass
    }

    #[test]
    fn no_proxy_lists_names_beneath_them_addresses_and_networks() {
        let bypass =
            no_proxy(" Example.com, .corp.net,*.lan, 10.0.0.0/8, ::1, [fd00::]/8 ,192.0.2.7");
        for host in [
            "example.com",
            "EXAMPLE.com.",
            "a.b.example.com",
            "corp.net",
            "x.corp.net",
            "printer.lan",
            "10.1.2.3",
            "::1",
            "[::1]",
            "fd12::5",
            "192.0.2.7",
            // An IPv4 address mapped into IPv6 is that address.
            "::ffff:192.0.2.7",
        ] {
            assert!(bypass.matches(host), "{host}");
        }
        for host in [
            "notexample.com",
            "example.com.evil",
            "corp.network",
            "11.0.0.1",
            "192.0.2.8",
            "fe80::1",
        ] {
            assert!(!bypass.matches(host), "{host}");
        }
        assert!(no_proxy("*").matches("anything.example"));
        assert!(!NoProxy::default().matches("anything.example"));
        // A network of no bits holds every address of its family.
        assert!(no_proxy("0.0.0.0/0").matches("203.0.113.1"));
        assert!(!no_proxy("0.0.0.0/0").matches("2001:db8::1"));
    }

    // NO_PROXY is set for other programs too: what the node cannot read it
    // skips, and says so, keeping the rest.
    #[test]
    fn no_proxy_skips_what_it_cannot_read() {
        for text in [
            "host:8080",
            "10.0.0.0/33",
            "::/129",
            "a b",
            "1.2.3.4/x",
            "10.*",
        ] {
            let (bypass, problem) = NoProxy::parse(&format!("{text},near.example"));
            assert_eq!(bypass, no_proxy("near.example"), "{text}");
            let problem = problem.unwrap();
            assert!(
                problem.contains("skips 1 entries") && problem.contains(text),
                "{problem}"
            );
        }
        // A skipped entry is shown only in part.
        let long = "x".repeat(1000) + ":1";
        let problem = NoProxy::parse(&long).1.unwrap();
        assert!(problem.len() < 200, "{problem}");
        // It keeps the first entries, at most so many, and `*` past them.
        let enough = vec!["a.example"; MAX_NO_PROXY_ENTRIES].join(",");
        assert_eq!(NoProxy::parse(&enough).1, None);
        let many = format!("{enough},b.example,c:1,*");
        let (bypass, problem) = NoProxy::parse(&many);
        assert_eq!(bypass.entries.len(), MAX_NO_PROXY_ENTRIES);
        assert!(bypass.all);
        assert_eq!(
            problem,
            Some(format!(
                "skips 2 entries past the first {MAX_NO_PROXY_ENTRIES}"
            ))
        );
        // Empty entries are not entries.
        assert_eq!(NoProxy::parse(" , ,"), (NoProxy::default(), None));
    }

    /// What `read_head` makes of a proxy that sends `chunks`, one write
    /// each, then closes; or that sends them and then nothing, if `open`.
    async fn head_of(chunks: &[&[u8]], open: bool) -> Result<Vec<u8>, String> {
        let (mut node, mut proxy) = tokio::io::duplex(MAX_HEAD_BYTES * 2);
        for chunk in chunks {
            proxy.write_all(chunk).await.unwrap();
        }
        let _kept = open.then_some(proxy);
        read_head(&mut node).await
    }

    // spec/areas/peer-protocol.md#quic-over-websocket: the proxy's answer
    // is bounded in size, and is all it may send before the server.
    #[tokio::test]
    async fn the_proxys_answer_is_bounded() {
        let ok = b"HTTP/1.1 200 OK\r\n\r\n";
        assert_eq!(head_of(&[ok], false).await, Ok(ok.to_vec()));
        // An answer may come a byte at a time.
        let bytes: Vec<&[u8]> = ok.chunks(1).collect();
        assert_eq!(head_of(&bytes, true).await, Ok(ok.to_vec()));
        // An answer of exactly the most bytes is taken.
        let padding = MAX_HEAD_BYTES - ok.len() - "X: \r\n".len();
        let largest = format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "a".repeat(padding));
        assert_eq!(largest.len(), MAX_HEAD_BYTES);
        let read = head_of(&[largest.as_bytes()], true).await;
        assert_eq!(read.map(|head| head.len()), Ok(MAX_HEAD_BYTES));
        // A byte more is not, with or without its end, nor one that never
        // ends, though the proxy keeps sending.
        let over = format!("HTTP/1.1 200 OK\r\nX: {}a\r\n\r\n", "a".repeat(padding));
        let endless = vec![b'a'; MAX_HEAD_BYTES * 2];
        for answer in [over.as_bytes(), &endless] {
            let problem = head_of(&[answer], true).await.unwrap_err();
            assert!(problem.contains("longer than"), "{problem}");
        }
        // Bytes past the answer are not the proxy's to send.
        let early = [&ok[..], b"\x16\x03\x01"].concat();
        let problem = head_of(&[&early], true).await.unwrap_err();
        assert!(problem.contains("more than its answer"), "{problem}");
        // A proxy that closes before it has answered has not.
        for chunks in [&[][..], &[&b"HTTP/1.1 200 OK\r\n"[..]]] {
            let problem = head_of(chunks, false).await.unwrap_err();
            assert!(problem.contains("closed"), "{problem}");
        }
    }

    // A proxy that never answers holds the dial only as long as its caller
    // gives it (spec/areas/peer-protocol.md#quic-over-websocket); one that
    // answers what is not 2xx refuses.
    #[tokio::test]
    async fn connect_waits_for_a_2xx_and_no_more() {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let proxy = proxy(&format!("http://{address}")).unwrap();
        let silent = tokio::spawn(async move {
            let (mut held, _) = listener.accept().await.unwrap();
            let mut request = [0; 512];
            let _ = held.read(&mut request).await;
            let (mut refusing, _) = listener.accept().await.unwrap();
            let _ = refusing.read(&mut request).await;
            refusing
                .write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n")
                .await
                .unwrap();
            held
        });
        let waited = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            proxy.connect(("node.example", 443)),
        )
        .await;
        assert!(waited.is_err(), "the proxy has not answered");
        let refused = proxy.connect(("node.example", 443)).await;
        assert_eq!(refused.err(), Some("the proxy answered 403".to_owned()));
        drop(silent.await.unwrap());
        // A host that could end a line is never sent.
        let problem = proxy.connect(("a\r\nb", 443)).await.unwrap_err();
        assert!(problem.contains("not a host"), "{problem}");
    }

    #[test]
    fn a_response_head_ends_at_its_blank_line_and_has_a_status() {
        let head = b"HTTP/1.1 200 Connection established\r\nVia: proxy\r\n\r\n";
        assert_eq!(head_end(head), Some(head.len()));
        assert_eq!(status(head), Ok(200));
        assert_eq!(head_end(b"HTTP/1.1 200 OK\r\n"), None);
        assert_eq!(
            status(b"HTTP/1.0 407 Proxy Authentication Required\r\n\r\n"),
            Ok(407)
        );
        // No reason phrase is needed.
        assert_eq!(status(b"HTTP/1.1 204\r\n\r\n"), Ok(204));
        for head in [
            &b"HTTP/2 200 OK\r\n\r\n"[..],
            b"SSH-2.0-OpenSSH\r\n\r\n",
            b"HTTP/1.1 20 OK\r\n\r\n",
            b"HTTP/1.1 2000 OK\r\n\r\n",
            b"HTTP/1.1 +20 OK\r\n\r\n",
            b"HTTP/1.1 200 OK",
            b"\xff\xfe\r\n\r\n",
        ] {
            assert!(status(head).is_err(), "{head:?}");
        }
    }
}
