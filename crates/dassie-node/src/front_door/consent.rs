//! The front door's consent page (spec/areas/http.md#front-doors): what a
//! browser shows at `https://<name>/gnap/interact/{id}/{key}`, where an app
//! sends the owner of a node without a hostname to approve the grant it
//! asked for (spec/designs/periphery.md#7-open-payments-without-a-public-address).
//!
//! The wallet is never served through a front door, so the owner decides in
//! their app, which lists the grants that wait. This page only says so, and
//! follows the grant: its one script asks the node for the grant's state,
//! `…/state`, which the front door passes on as any Open Payments route,
//! and once the owner decided, sends the browser on to the client's finish
//! URI, as the wallet's own consent page does.
//!
//! The front door writes the page alone, as its page at a name's root
//! ([`page`](super::page)): nothing of it comes from the node or the
//! request but the name, which is escaped. It asks for nothing: no form, no
//! field, no cookie. Its one script and its one style are pinned by their
//! hashes, and the script reaches nothing but the grant's state on the same
//! name.

use hyper::header::{self, HeaderValue};
use hyper::{Response, StatusCode};

use super::page::{Part, Reached, STYLE, push_escaped};
use crate::http::response::{self, Body};

/// Where interactive grants send the owner.
pub const PREFIX: &str = "/gnap/interact/";

/// The longest grant ID or key the page takes, in bytes: more than the
/// node's IDs and keys ever are.
const MAX_SEGMENT_BYTES: usize = 64;

/// How often the page asks for the grant's state, in milliseconds, as its
/// script writes it.
pub const POLL_MS: u64 = 2_000;

/// How many times the page asks before it gives up, as its script writes
/// it: for longer than a grant waits for the owner
/// (`PENDING_GRANT_LIFETIME_MS`), so a grant that lapsed says so first.
pub const MAX_POLLS: u64 = 330;

const _: () = assert!(
    POLL_MS * MAX_POLLS > crate::open_payments::auth::PENDING_GRANT_LIFETIME_MS,
    "the page follows a grant for longer than it waits for the owner"
);

/// The page's Content Security Policy: only its own script and style run,
/// by their hashes (`'sha256-…'` of [`SCRIPT`] and [`STYLE`], checked by a
/// test); the script may fetch from the page's own name, nothing else
/// loads, no form is sent and no one frames it. A sandbox that allows
/// scripts and nothing more gives the page an origin of its own, so it
/// shares nothing with the name's other answers.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
     script-src 'sha256-GUD9asdxpPZ84E6xpXyu4t/KAzEaGWn7RC8TAqQCTX4='; \
     style-src 'sha256-6qclauwb099gLpQGpXRSsx2TLrLffT9xgxaqb03t+UY='; connect-src 'self'; \
     base-uri 'none'; form-action 'none'; frame-ancestors 'none'; sandbox allow-scripts";

/// The page's script: it asks for the grant's state every [`POLL_MS`] while
/// the grant waits, at most [`MAX_POLLS`] times, and once the owner
/// approved, goes on to the finish URI the node names, only an `http` or
/// `https` one, as the wallet's consent page goes there after an approval
/// through the app's link. A refusal goes nowhere: the app chose that URL,
/// and learns the answer from the node (spec/areas/payments.md#the-owners-consent).
pub const SCRIPT: &str = r#"(function () {
  "use strict";
  var status = document.getElementById("status");
  var url = location.pathname + "/state";
  var tries = 0;
  function say(text) { status.textContent = text; }
  function go(finish, text) {
    var target = null;
    try { target = new URL(finish); } catch (error) { target = null; }
    if (!target || (target.protocol !== "https:" && target.protocol !== "http:")) return false;
    say(text);
    location.replace(target.href);
    return true;
  }
  function later() {
    tries += 1;
    if (tries > 330) {
      say("The node did not answer for a long time. Ask the app to start again.");
      return;
    }
    setTimeout(check, 2000);
  }
  function settle(body) {
    var finish = typeof body.finish === "string" ? body.finish : "";
    if (body.state === "pending") {
      later();
    } else if (body.state === "approved") {
      if (!go(finish, "Approved. Taking you back to the app…")) say("Approved. You can go back to the app.");
    } else if (body.state === "denied") {
      say("Denied. You can close this page.");
    } else if (body.state === "lapsed") {
      say("This request lapsed: the owner did not decide in time. Ask the app to start again.");
    } else {
      later();
    }
  }
  function check() {
    fetch(url, { cache: "no-store", credentials: "omit", redirect: "error" }).then(function (reply) {
      if (reply.status === 404) {
        say("This request is no longer waiting. Ask the app to start again.");
        return null;
      }
      if (!reply.ok) {
        later();
        return null;
      }
      return reply.json().then(settle);
    }).catch(later);
  }
  check();
})();"#;

/// Whether `path` is an interaction, `/gnap/interact/{id}/{key}`, by its
/// shape alone: two segments of base64url or a UUID's characters, of at
/// most [`MAX_SEGMENT_BYTES`] each. Whether such a grant waits, the node
/// says, through the grant's state.
#[must_use]
pub fn is_interaction(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return false;
    };
    let mut segments = rest.split('/');
    let (Some(id), Some(key), None) = (segments.next(), segments.next(), segments.next()) else {
        return false;
    };
    [id, key].iter().all(|segment| {
        !segment.is_empty()
            && segment.len() <= MAX_SEGMENT_BYTES
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    })
}

/// The most bytes the page takes: its fixed text, and the name, even if
/// each of its characters were escaped, to at most six.
pub const MAX_PAGE_BYTES: usize = 16 * 1024;

/// The page for an interaction under the name `reached` under `domain`.
#[must_use]
pub fn render(domain: &str, reached: Reached<'_>) -> String {
    let first = match reached {
        Reached::Id(id) => id.as_str(),
        Reached::Name(name) => name.as_str(),
    };
    let parts = [
        Part::Html(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
             <meta name=\"robots\" content=\"noindex\">\n\
             <title>Approve this in your Dassie app</title>\n<style>",
        ),
        Part::Html(STYLE),
        Part::Html(
            "</style>\n</head>\n<body>\n<main>\n<h1>Approve this in your Dassie app</h1>\n\
             <p>An app sent you here to ask the owner of the Dassie node at <code>",
        ),
        Part::Text(first),
        Part::Html("."),
        Part::Text(domain),
        Part::Html(
            "</code> for access.</p>\n<p>If the node is yours, the request waits for you in \
             your Dassie app: open it to see who asks and for what, and approve or deny it \
             there. This page then takes you back to the app that sent you here.</p>\n\
             <p id=\"status\" role=\"status\">Waiting for the owner to decide\u{2026}</p>\n\
             <noscript><p>Once you decided in your Dassie app, go back to the app that sent \
             you here.</p></noscript>\n\
             <p>This page asks for nothing. Never enter your recovery phrase or a password on \
             a page an app sent you to.</p>\n<script>",
        ),
        Part::Html(SCRIPT),
        Part::Html("</script>\n</main>\n</body>\n</html>\n"),
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
/// sending no referrer, so the finish URI never learns the interaction's
/// key from it.
#[must_use]
pub fn response(domain: &str, reached: Reached<'_>) -> Response<Body> {
    let page = render(domain, reached);
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
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use sha2::{Digest, Sha256};

    use super::*;

    fn node() -> dassie_identity::NodeId {
        dassie_identity::TableKeygen::identity_key(&[5; 32]).node_id()
    }

    fn hash(text: &str) -> String {
        format!(
            "'sha256-{}'",
            STANDARD.encode(Sha256::digest(text.as_bytes()))
        )
    }

    #[test]
    fn the_policy_pins_the_script_and_the_style_by_their_hashes() {
        assert!(
            CONTENT_SECURITY_POLICY.contains(&format!("script-src {};", hash(SCRIPT))),
            "script-src {}",
            hash(SCRIPT)
        );
        assert!(CONTENT_SECURITY_POLICY.contains(&format!("style-src {};", hash(STYLE))));
        assert!(CONTENT_SECURITY_POLICY.starts_with("default-src 'none';"));
        assert!(CONTENT_SECURITY_POLICY.ends_with("; sandbox allow-scripts"));
        let id = node();
        let page = render("dassiepay.net", Reached::Id(&id));
        assert_eq!(page.matches("<script>").count(), 1);
        assert!(page.contains(&format!("<script>{SCRIPT}</script>")));
        assert_eq!(page.matches("<style>").count(), 1);
        assert!(page.contains(&format!("<style>{STYLE}</style>")));
    }

    #[test]
    fn the_page_asks_for_nothing() {
        let id = node();
        let page = render("dassiepay.net", Reached::Id(&id)).to_ascii_lowercase();
        assert!(page.contains(&format!(
            "<code>{}.dassiepay.net</code>",
            id.as_str().to_ascii_lowercase()
        )));
        for absent in [
            "<form",
            "<input",
            "<img",
            "<iframe",
            "<link",
            "<object",
            "<embed",
            "javascript:",
            "http-equiv",
            "style=",
            "onclick",
            "src=",
        ] {
            assert!(!page.contains(absent), "{absent}");
        }
    }

    #[test]
    fn the_script_follows_the_grant_for_longer_than_it_waits() {
        assert!(SCRIPT.contains(&format!("setTimeout(check, {POLL_MS})")));
        assert!(SCRIPT.contains(&format!("tries > {MAX_POLLS}")));
        // It reaches the grant's state on the same name, and nothing else.
        assert_eq!(SCRIPT.matches("fetch(").count(), 1);
        assert!(SCRIPT.contains("location.pathname + \"/state\""));
        assert!(!SCRIPT.contains("</"));
    }

    #[test]
    fn only_an_interaction_is_a_page() {
        let longest = "a".repeat(MAX_SEGMENT_BYTES);
        let too_long = "a".repeat(MAX_SEGMENT_BYTES + 1);
        for path in [
            "/gnap/interact/9b2c0e4e-6a3f-4d27-9d7a-3f1c2b5e8a10/Ab-_09",
            &format!("/gnap/interact/{longest}/{longest}"),
        ] {
            assert!(is_interaction(path), "{path}");
        }
        for path in [
            "/gnap/interact/id",
            "/gnap/interact/id/",
            "/gnap/interact//key",
            "/gnap/interact/id/key/",
            "/gnap/interact/id/key/state",
            "/gnap/interact/id/k.y",
            "/gnap/interact/id/k%2Fy",
            "/gnap/interact/i<d/key",
            &format!("/gnap/interact/{too_long}/key"),
            "/gnap/continue/id/key",
        ] {
            assert!(!is_interaction(path), "{path}");
        }
    }

    #[test]
    fn a_name_is_escaped_and_the_page_bounded() {
        let id = node();
        let page = render("<x>&\"'.example", Reached::Id(&id));
        assert!(!page.contains("<x>"));
        assert!(page.contains("&lt;x&gt;&amp;&quot;&#39;.example"));
        let longest = "\"".repeat(253);
        assert!(render(&longest, Reached::Id(&id)).len() <= MAX_PAGE_BYTES);
    }

    #[test]
    fn the_answer_is_html_under_the_pages_policy() {
        let id = node();
        let answer = response("dassiepay.net", Reached::Id(&id));
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
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
        assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        assert!(headers.get(header::SET_COOKIE).is_none());
    }
}
