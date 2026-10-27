//! JavaScript bindings (wasm-bindgen) for the wallet and the dev tools.
//!
//! Byte-oriented on purpose: keys and secrets cross the boundary as
//! `Uint8Array`s and phrases as strings. This is the edge where randomness
//! comes from the platform (`crypto.getRandomValues`), so the core stays
//! deterministic (spec/decisions/tigerstyle.md); the time comes from the
//! caller, as milliseconds since the Unix epoch (`Date.now()`).
//!
//! A phrase is read as the decision says (spec/decisions/phrase-format.md):
//! [`read_phrase`] returns a [`PhraseReading`], which shows a correction's
//! words and the positions that changed, and gives up the phrase only
//! through `confirm()`, once the owner has seen it, or `exact()`, which
//! refuses a reading that needed correcting. Refusals are thrown as `Error`s
//! with a `code` (see `packages/wasm/phrase_error.ts`), so the wallet can
//! word each one.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use rand_core::UnwrapErr;
use wasm_bindgen::prelude::*;

use dassie_identity::{
    Created, Error, MAX_PHRASE_INPUT_BYTES, NodeId, NodeKey, PHRASE_WORDS, Phrase, PhraseSecret,
    PhraseText, Positions, Reading, Realm, Words, word_named,
};

/// A day in milliseconds: how far apart the wallet's clock and the node's
/// may be when a phrase is made at setup.
const DAY_MS: u64 = 86_400_000;

#[wasm_bindgen]
extern "C" {
    /// A JavaScript `Error`, to throw with a `code` and details.
    #[wasm_bindgen(js_name = Error)]
    type ErrorObject;

    #[wasm_bindgen(constructor, js_class = "Error")]
    fn new(message: &str) -> ErrorObject;

    #[wasm_bindgen(method, setter, js_class = "Error")]
    fn set_code(this: &ErrorObject, code: &str);

    #[wasm_bindgen(method, setter, js_class = "Error")]
    fn set_words(this: &ErrorObject, words: u32);

    #[wasm_bindgen(method, setter, js_class = "Error")]
    fn set_kind(this: &ErrorObject, kind: u32);

    #[wasm_bindgen(method, setter, js_class = "Error")]
    fn set_realm(this: &ErrorObject, realm: &str);

    #[wasm_bindgen(method, setter, js_class = "Error")]
    fn set_date(this: &ErrorObject, date: &str);

    #[wasm_bindgen(method, setter, js_class = "Error")]
    fn set_changed(this: &ErrorObject, changed: Vec<u32>);
}

/// An `Error` with `code`, and the message `message`.
fn thrown(code: &str, message: &str) -> ErrorObject {
    let error = ErrorObject::new(message);
    error.set_code(code);
    error
}

/// The error a refusal of the identity crate is thrown as: its message, a
/// `code`, and what it names. A phrase's errors name positions, counts, a
/// realm or a date, never a word.
fn js(error: Error) -> JsValue {
    let message = error.to_string();
    let object = match error {
        Error::PhraseTooLong => thrown("tooLong", &message),
        Error::Bip39Phrase { words } => {
            let object = thrown("bip39", &message);
            object.set_words(u32::try_from(words).unwrap_or(u32::MAX));
            object
        }
        Error::WrongWordCount { words } => {
            let object = thrown("wordCount", &message);
            object.set_words(u32::try_from(words).unwrap_or(u32::MAX));
            object
        }
        Error::TooDamaged => thrown("tooDamaged", &message),
        Error::InvalidKind => thrown("invalidKind", &message),
        Error::NewerKind { kind } => {
            let object = thrown("newerKind", &message);
            object.set_kind(u32::from(kind));
            object
        }
        Error::OtherRealm { realm } => {
            let object = thrown("otherRealm", &message);
            object.set_realm(realm.as_str());
            object
        }
        Error::CreatedInFuture { created } => {
            let object = thrown("createdInFuture", &message);
            object.set_date(&created.to_string());
            object
        }
        Error::NeedsConfirmation { changed } => {
            let object = thrown("needsConfirmation", &message);
            object.set_changed(positions(changed));
            object
        }
        Error::ClockOutOfRange => thrown("clockOutOfRange", &message),
        _ => thrown("invalid", &message),
    };
    object.into()
}

fn node_key_from(bytes: &[u8]) -> Result<NodeKey, JsError> {
    let seed: &[u8; 32] = bytes
        .try_into()
        .map_err(|_| JsError::new("a node key has 32 bytes"))?;
    Ok(NodeKey::from_bytes(seed))
}

/// The realm named `test` or `live`.
fn realm_of(name: &str) -> Result<Realm, JsValue> {
    Realm::parse(name).ok_or_else(|| thrown("realm", "a realm is test or live").into())
}

/// A clock reading from JavaScript: milliseconds since the Unix epoch. A
/// reading before 1970, or one that is not a number, is refused.
// The bindings are the edge where the time comes in as a JavaScript
// number: a whole, non-negative number of milliseconds below 2⁵³ converts
// exactly.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clock(now_ms: f64) -> Result<u64, JsValue> {
    if (0.0..=9_007_199_254_740_991.0).contains(&now_ms) {
        Ok(now_ms as u64)
    } else {
        Err(thrown("clockOutOfRange", "the clock reads no time since 1970").into())
    }
}

/// Positions, zero-based, as a list.
fn positions(positions: Positions) -> Vec<u32> {
    positions
        .iter()
        .map(|position| u32::try_from(position).unwrap_or(u32::MAX))
        .collect()
}

/// A phrase: from [`generate_phrase`], [`phrase_from_secret`], or a
/// [`PhraseReading`] that read as written or whose correction the owner
/// confirmed. It is held in the module's memory and wiped when freed.
#[wasm_bindgen(js_name = Phrase)]
pub struct JsPhrase {
    phrase: Phrase,
}

#[wasm_bindgen(js_class = Phrase)]
impl JsPhrase {
    /// The canonical phrase: the words in full, lowercase, single spaces.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn text(&self) -> String {
        String::from(self.phrase.text().as_str())
    }

    /// The 20 words in order.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn words(&self) -> Vec<String> {
        self.phrase.words().map(String::from).collect()
    }

    /// The realm the phrase's kind names: `test` or `live`.
    #[wasm_bindgen(getter, unchecked_return_type = "\"test\" | \"live\"")]
    #[must_use]
    pub fn realm(&self) -> String {
        String::from(self.phrase.realm().as_str())
    }

    /// When the phrase was made, in hours since 2026-01-01T00:00:00Z.
    #[wasm_bindgen(getter, js_name = createdHours)]
    #[must_use]
    pub fn created_hours(&self) -> u32 {
        self.phrase.created().hours()
    }

    /// The day the phrase was made, as shown with it: `2026-02-19`.
    #[wasm_bindgen(getter, js_name = createdDate)]
    #[must_use]
    pub fn created_date(&self) -> String {
        self.phrase.created().to_string()
    }

    /// The node key (Ed25519 seed, 32 bytes), derived in `realm`, the realm
    /// the caller expects.
    ///
    /// # Errors
    ///
    /// The phrase is of the other realm, or `realm` names none.
    #[wasm_bindgen(js_name = nodeKey)]
    pub fn node_key(
        &self,
        #[wasm_bindgen(unchecked_param_type = "\"test\" | \"live\"")] realm: &str,
    ) -> Result<Vec<u8>, JsValue> {
        let seed = self.phrase.master_seed(realm_of(realm)?).map_err(js)?;
        Ok(seed.node_key().as_bytes().to_vec())
    }
}

/// What reading a phrase found: the phrase as written, or a correction to
/// show the owner, with the positions that changed marked.
#[wasm_bindgen]
pub struct PhraseReading {
    reading: Reading,
}

impl PhraseReading {
    fn words_shown(&self) -> Words<'_> {
        match &self.reading {
            Reading::Exact(phrase) => phrase.words(),
            Reading::Corrected(correction) => correction.words(),
        }
    }

    fn text_shown(&self) -> PhraseText {
        match &self.reading {
            Reading::Exact(phrase) => phrase.text(),
            Reading::Corrected(correction) => correction.text(),
        }
    }

    fn realm_shown(&self) -> Realm {
        match &self.reading {
            Reading::Exact(phrase) => phrase.realm(),
            Reading::Corrected(correction) => correction.realm(),
        }
    }

    fn created_shown(&self) -> Created {
        match &self.reading {
            Reading::Exact(phrase) => phrase.created(),
            Reading::Corrected(correction) => correction.created(),
        }
    }
}

#[wasm_bindgen]
impl PhraseReading {
    /// Whether some words were wrong or unreadable and have been restored,
    /// so the owner must see the correction and confirm it.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn corrected(&self) -> bool {
        matches!(self.reading, Reading::Corrected(_))
    }

    /// The positions of the words that changed, zero-based, unreadable ones
    /// included: empty if the phrase read as written.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn changed(&self) -> Vec<u32> {
        match &self.reading {
            Reading::Exact(_) => Vec::new(),
            Reading::Corrected(correction) => positions(correction.changed()),
        }
    }

    /// The words, corrected, to show.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn words(&self) -> Vec<String> {
        self.words_shown().map(String::from).collect()
    }

    /// The phrase in canonical form, corrected, to show.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn text(&self) -> String {
        String::from(self.text_shown().as_str())
    }

    /// The realm the phrase's kind names: always the one expected.
    #[wasm_bindgen(getter, unchecked_return_type = "\"test\" | \"live\"")]
    #[must_use]
    pub fn realm(&self) -> String {
        String::from(self.realm_shown().as_str())
    }

    /// When the phrase was made, in hours since 2026-01-01T00:00:00Z.
    #[wasm_bindgen(getter, js_name = createdHours)]
    #[must_use]
    pub fn created_hours(&self) -> u32 {
        self.created_shown().hours()
    }

    /// The day the phrase was made: `2026-02-19`.
    #[wasm_bindgen(getter, js_name = createdDate)]
    #[must_use]
    pub fn created_date(&self) -> String {
        self.created_shown().to_string()
    }

    /// The phrase, once the owner has seen the correction, if there was
    /// one, and confirmed it. This reading is spent.
    #[must_use]
    pub fn confirm(self) -> JsPhrase {
        let phrase = match self.reading {
            Reading::Exact(phrase) => phrase,
            Reading::Corrected(correction) => correction.confirm(),
        };
        JsPhrase { phrase }
    }

    /// The phrase, if it read as written, for a reader with no one to
    /// confirm a correction. This reading is spent.
    ///
    /// # Errors
    ///
    /// `needsConfirmation`, naming the positions a correction would
    /// change, never the words.
    pub fn exact(self) -> Result<JsPhrase, JsValue> {
        let phrase = self.reading.without_correction().map_err(js)?;
        Ok(JsPhrase { phrase })
    }
}

/// A new phrase in `realm`, with a secret from the platform's random number
/// generator, made at `nowMs` by the caller's clock.
///
/// At setup, the wallet passes the node's clock as `nodeNowMs`, and no
/// phrase is made while the two are more than a day apart: a phrase made
/// by a clock that runs ahead would be refused by every correct reader
/// until its date.
///
/// # Errors
///
/// `realm` names no realm; `clockOutOfRange` if `nowMs` is outside the
/// hours a phrase can record; `clocksApart` if it is more than a day from
/// `nodeNowMs`.
#[wasm_bindgen(js_name = generatePhrase)]
pub fn generate_phrase(
    #[wasm_bindgen(unchecked_param_type = "\"test\" | \"live\"")] realm: &str,
    #[wasm_bindgen(js_name = nowMs)] now_ms: f64,
    #[wasm_bindgen(js_name = nodeNowMs)] node_now_ms: Option<f64>,
) -> Result<JsPhrase, JsValue> {
    let realm = realm_of(realm)?;
    let now_ms = clock(now_ms)?;
    if let Some(node_now_ms) = node_now_ms
        && clock(node_now_ms)?.abs_diff(now_ms) > DAY_MS
    {
        return Err(thrown(
            "clocksApart",
            "this clock and the node's are more than a day apart, so no phrase is made: set the \
             wrong one right",
        )
        .into());
    }
    let mut rng = UnwrapErr(getrandom::SysRng);
    let phrase = Phrase::generate(&mut rng, realm, now_ms).map_err(js)?;
    Ok(JsPhrase { phrase })
}

/// Reads a phrase that the owner typed, expecting one of `realm`, by a
/// clock that reads `nowMs`. The words at the positions in `unreadable`
/// (zero-based) are erasures, as is any word not on the list.
///
/// # Errors
///
/// What the identity crate refuses, by `code`: `tooLong`, `bip39`,
/// `wordCount`, `tooDamaged`, `invalidKind`, `newerKind`, `otherRealm`
/// and `createdInFuture`; and `realm`, `clockOutOfRange` and `unreadable`
/// for arguments that are not a realm, a time or positions in a phrase.
#[wasm_bindgen(js_name = readPhrase)]
pub fn read_phrase(
    text: &str,
    #[wasm_bindgen(unchecked_param_type = "\"test\" | \"live\"")] realm: &str,
    #[wasm_bindgen(js_name = nowMs)] now_ms: f64,
    unreadable: Option<Vec<u32>>,
) -> Result<PhraseReading, JsValue> {
    let realm = realm_of(realm)?;
    let now_ms = clock(now_ms)?;
    let mut marked = Positions::NONE;
    for position in unreadable.unwrap_or_default() {
        let position = usize::try_from(position).unwrap_or(usize::MAX);
        if position >= PHRASE_WORDS {
            return Err(thrown("unreadable", "an unreadable word is one of the 20").into());
        }
        marked = marked.with(position);
    }
    let reading = Phrase::read(text, marked, realm, now_ms).map_err(js)?;
    Ok(PhraseReading { reading })
}

/// What each word of `text` reads as, in order, split and matched as
/// [`read_phrase`] splits and matches them: the word of the list it names,
/// or `undefined` for a word not on the list, which a reading takes as
/// unreadable. For the wallet to show the owner, while they type, how
/// many words it reads and which it does not know.
///
/// # Errors
///
/// `tooLong` beyond the longest input a reading looks at.
#[wasm_bindgen(
    js_name = phraseWords,
    unchecked_return_type = "(string | undefined)[]"
)]
pub fn phrase_words(text: &str) -> Result<Vec<JsValue>, JsValue> {
    if text.len() > MAX_PHRASE_INPUT_BYTES {
        return Err(js(Error::PhraseTooLong));
    }
    Ok(text
        .split_whitespace()
        .map(|typed| word_named(typed).map_or(JsValue::UNDEFINED, JsValue::from_str))
        .collect())
}

/// The node key (Ed25519 seed) of a phrase of `realm` that reads as
/// written by a clock that reads `nowMs`: for scripts and tests, which
/// have no one to confirm a correction.
///
/// # Errors
///
/// As [`read_phrase`], and `needsConfirmation` for a phrase that would
/// need correcting.
#[wasm_bindgen(js_name = deriveNodeKey)]
pub fn derive_node_key(
    phrase: &str,
    #[wasm_bindgen(unchecked_param_type = "\"test\" | \"live\"")] realm: &str,
    #[wasm_bindgen(js_name = nowMs)] now_ms: f64,
) -> Result<Vec<u8>, JsValue> {
    read_phrase(phrase, realm, now_ms, None)?
        .exact()?
        .node_key(realm)
}

/// The phrase of a 16-byte `secret` in `realm`, made `createdHours` hours
/// after 2026-01-01T00:00:00Z: how the dev environment turns its table of
/// secrets into node keys. A dev tool's only: the wallet makes phrases with
/// [`generate_phrase`].
///
/// # Errors
///
/// The secret is not 16 bytes, `realm` names no realm, or the hour is
/// outside the field.
#[wasm_bindgen(js_name = phraseFromSecret)]
pub fn phrase_from_secret(
    secret: &[u8],
    #[wasm_bindgen(unchecked_param_type = "\"test\" | \"live\"")] realm: &str,
    #[wasm_bindgen(js_name = createdHours)] created_hours: u32,
) -> Result<JsPhrase, JsValue> {
    let bytes: [u8; 16] = secret
        .try_into()
        .map_err(|_| thrown("secret", "a phrase's secret has 16 bytes"))?;
    let created = Created::from_hours(created_hours).ok_or_else(|| {
        thrown(
            "clockOutOfRange",
            "the hour is outside the hours a phrase can record",
        )
    })?;
    let phrase = Phrase::new(realm_of(realm)?, created, &PhraseSecret::from_bytes(bytes));
    Ok(JsPhrase { phrase })
}

/// The login secret of a node key: what setup and login send as proof.
#[wasm_bindgen(js_name = loginSecret)]
pub fn login_secret(node_key: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(node_key_from(node_key)?.login_secret().as_bytes().to_vec())
}

/// The Ed25519 public key of a node key.
#[wasm_bindgen(js_name = publicKey)]
pub fn public_key(node_key: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(node_key_from(node_key)?.public_key().as_bytes().to_vec())
}

/// The node ID of an Ed25519 public key.
#[wasm_bindgen(js_name = nodeId)]
pub fn node_id(public_key: &[u8]) -> Result<String, JsError> {
    let key: &[u8; 32] = public_key
        .try_into()
        .map_err(|_| JsError::new("a public key has 32 bytes"))?;
    Ok(String::from(NodeId::from_public_key(key).as_str()))
}

/// Signs `message` with a node key.
#[wasm_bindgen]
pub fn sign(node_key: &[u8], message: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(node_key_from(node_key)?.sign(message).as_bytes().to_vec())
}
