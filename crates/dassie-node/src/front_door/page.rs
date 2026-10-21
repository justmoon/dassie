//! The front door's one page (spec/areas/http.md#front-doors): what a
//! browser shows at `https://<label>.<domain>/`, the address of a node
//! someone typed in.
//!
//! The front door writes it alone. Nothing of it comes from the node: the
//! only variable parts are the name (the label, from the node ID, and the
//! front door's domain), the node ID, and whether the node is connected to
//! the front door now. Each is escaped, though none can hold a character
//! that needs it. The page runs nothing and loads nothing: no script, no
//! form, no image, no cookie, and its one style is pinned by its hash.

use dassie_identity::{NodeId, NodeLabel};
use hyper::header::{self, HeaderValue};
use hyper::{Response, StatusCode};

use crate::http::response::{self, Body};

/// The path the page answers, for `GET` and `HEAD`.
pub const PATH: &str = "/";

/// The page's Content Security Policy: nothing loads, nothing runs, no form
/// is sent and no one frames it; only the page's own style applies, by its
/// hash (`'sha256-…'` of [`STYLE`], checked by a test), so that even markup
/// that slipped past the escaping could not restyle it.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
     style-src 'sha256-6qclauwb099gLpQGpXRSsx2TLrLffT9xgxaqb03t+UY='; base-uri 'none'; \
     form-action 'none'; frame-ancestors 'none'; sandbox";

/// The page's style: a system font, readable on a phone, light and dark.
const STYLE: &str = ":root{color-scheme:light dark;--fg:#1d1d1b;--bg:#fbfaf7;--muted:#5f5e5a;\
--link:#1a5fb4;--on:#26734d;--off:#a1460f}\
@media (prefers-color-scheme:dark){:root{--fg:#ecebe7;--bg:#181816;--muted:#a3a29c;\
--link:#8bb8f2;--on:#7ccf9f;--off:#f0a874}}\
body{margin:0;background:var(--bg);color:var(--fg);\
font:1rem/1.5 system-ui,-apple-system,\"Segoe UI\",Roboto,sans-serif}\
main{max-width:36rem;margin:0 auto;padding:2.5rem 1rem}\
h1{font-size:1.5rem;line-height:1.25;margin:0 0 1rem}\
dl{margin:1.5rem 0}dt{color:var(--muted);font-size:.875rem}\
dd{margin:0 0 .75rem;overflow-wrap:anywhere}\
code{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:.9375rem}\
a{color:var(--link);overflow-wrap:anywhere}.on{color:var(--on)}.off{color:var(--off)}";

/// The longest domain, in bytes: DNS's longest name.
const MAX_DOMAIN_BYTES: usize = 253;

/// The most bytes the page takes: its fixed text, and the domain four times
/// over, even if each of its characters were escaped, to at most six.
pub const MAX_PAGE_BYTES: usize = 16 * 1024;

/// A piece of the page: markup the front door wrote, or text, which is
/// escaped. Markup is `'static`, so nothing a request names can be one.
enum Part<'a> {
    Html(&'static str),
    Text(&'a str),
}

/// The page for the node `label` names under `domain`, the front door's
/// own, connected to the front door now or not.
#[must_use]
pub fn render(domain: &str, label: &NodeLabel, node: &NodeId, connected: bool) -> String {
    assert!(domain.len() <= MAX_DOMAIN_BYTES, "a domain is a hostname");
    let label = label.as_str();
    let (state_class, state) = if connected {
        ("on", "Connected to this front door now.")
    } else {
        (
            "off",
            "Not connected to this front door now. Payments reach the node once it connects again.",
        )
    };
    let parts = [
        Part::Html(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
             <meta name=\"robots\" content=\"noindex\">\n<title>A Dassie node's address</title>\n<style>",
        ),
        Part::Html(STYLE),
        Part::Html(
            "</style>\n</head>\n<body>\n<main>\n<h1>A Dassie node's address</h1>\n\
             <p>This is the address of the Dassie node with the ID below, reached through \
             the ",
        ),
        Part::Text(domain),
        Part::Html(" front door.</p>\n<dl>\n<dt>Name</dt>\n<dd><code>"),
        Part::Text(label),
        Part::Html("."),
        Part::Text(domain),
        Part::Html("</code></dd>\n<dt>Node ID</dt>\n<dd><code>"),
        Part::Text(node.as_str()),
        Part::Html("</code></dd>\n<dt>Status</dt>\n<dd class=\""),
        Part::Html(state_class),
        Part::Html("\">"),
        Part::Html(state),
        Part::Html(
            "</dd>\n</dl>\n<p>This address serves Open Payments only. Wallets and apps pay \
             the node at its wallet address:</p>\n<p><a href=\"https://",
        ),
        Part::Text(label),
        Part::Html("."),
        Part::Text(domain),
        Part::Html("/.well-known/pay\">https://"),
        Part::Text(label),
        Part::Html("."),
        Part::Text(domain),
        Part::Html(
            "/.well-known/pay</a></p>\n<p>The node's owner manages it in the node's own \
             wallet, not here.</p>\n<p>Dassie is a peer-to-peer payment network: \
             <a href=\"https://dassie.land\">dassie.land</a></p>\n</main>\n</body>\n</html>\n",
        ),
    ];
    let mut page = String::with_capacity(MAX_PAGE_BYTES);
    for part in parts {
        match part {
            Part::Html(html) => page.push_str(html),
            Part::Text(text) => push_escaped(&mut page, text),
        }
    }
    assert!(page.len() <= MAX_PAGE_BYTES, "the page is bounded");
    page
}

/// The page as an answer: HTML under the page's own policy, never stored,
/// since whether the node is connected changes.
#[must_use]
pub fn response(domain: &str, label: &NodeLabel, node: &NodeId, connected: bool) -> Response<Body> {
    let page = render(domain, label, node, connected);
    let mut response = response::with_body(StatusCode::OK, "text/html; charset=utf-8", page);
    let headers = response.headers_mut();
    response::secure(headers, true);
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Appends `text` with every character that means something in HTML,
/// in text or in a quoted attribute, escaped.
fn push_escaped(page: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => page.push_str("&amp;"),
            '<' => page.push_str("&lt;"),
            '>' => page.push_str("&gt;"),
            '"' => page.push_str("&quot;"),
            '\'' => page.push_str("&#39;"),
            other => page.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use dassie_identity::NodeKey;
    use sha2::{Digest, Sha256};

    use super::*;

    fn node() -> (NodeId, NodeLabel) {
        let id = NodeKey::from_bytes(&[5; 32]).node_id();
        let label = id.label().unwrap();
        (id, label)
    }

    #[test]
    fn the_policy_pins_the_style_by_its_hash() {
        let hash = STANDARD.encode(Sha256::digest(STYLE.as_bytes()));
        assert!(
            CONTENT_SECURITY_POLICY.contains(&format!("style-src 'sha256-{hash}';")),
            "style-src 'sha256-{hash}'"
        );
        assert!(CONTENT_SECURITY_POLICY.starts_with("default-src 'none';"));
        assert!(CONTENT_SECURITY_POLICY.ends_with("; sandbox"));
        // Exactly one style element, whose text is the hashed style.
        let (id, label) = node();
        let page = render("dassiepay.net", &label, &id, true);
        assert_eq!(page.matches("<style>").count(), 1);
        assert!(page.contains(&format!("<style>{STYLE}</style>")));
    }

    #[test]
    fn the_page_names_the_node_and_its_state() {
        let (id, label) = node();
        let name = format!("{}.dassiepay.net", label.as_str());
        for connected in [true, false] {
            let page = render("dassiepay.net", &label, &id, connected);
            assert!(page.contains(&format!("<code>{name}</code>")));
            assert!(page.contains(&format!("<code>{}</code>", id.as_str())));
            let address = format!("https://{name}/.well-known/pay");
            assert!(page.contains(&format!("<a href=\"{address}\">{address}</a>")));
            assert!(page.contains("<a href=\"https://dassie.land\">"));
            // Any valid label has a page, even one no key gives: the page
            // claims only that the address is that ID's.
            assert!(page.contains(
                "<p>This is the address of the Dassie node with the ID below, reached \
                 through the dassiepay.net front door.</p>"
            ));
            assert_eq!(page.contains("class=\"on\""), connected);
            assert_eq!(page.contains("Not connected"), !connected);
            let absent = [
                "<script",
                "<form",
                "<img",
                "<iframe",
                "<link",
                "<object",
                "<embed",
                "javascript:",
                "http-equiv",
                "style=",
            ];
            for absent in absent {
                assert!(!page.to_ascii_lowercase().contains(absent), "{absent}");
            }
        }
    }

    #[test]
    fn text_is_escaped() {
        let (id, label) = node();
        let page = render("<x>&\"'.example", &label, &id, true);
        assert!(!page.contains("<x>"));
        assert!(page.contains("&lt;x&gt;&amp;&quot;&#39;.example"));
    }

    #[test]
    fn the_page_stays_within_its_bound() {
        let (id, label) = node();
        let longest = "\"".repeat(MAX_DOMAIN_BYTES);
        assert!(render(&longest, &label, &id, false).len() <= MAX_PAGE_BYTES);
    }

    #[test]
    fn the_answer_is_html_under_the_pages_policy() {
        let (id, label) = node();
        let answer = response("dassiepay.net", &label, &id, false);
        assert_eq!(answer.status(), StatusCode::OK);
        let headers = answer.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            CONTENT_SECURITY_POLICY
        );
        assert_eq!(
            headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
        assert!(headers.get(header::SET_COOKIE).is_none());
    }
}
