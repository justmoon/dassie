//! Reading an owner's passphrase from a file, for the two commands that
//! take one: `dassie setup` and `dassie market-maker`
//! (spec/areas/node.md#the-dassie-command).
//!
//! The file is read once and bounded in size, the phrase is wiped as soon
//! as the node key is derived from it, and no error names what the file
//! holds: the phrase is the owner's only backup and the key to every
//! ledger account the node has. A phrase that reads only by correcting
//! some of its words is refused, naming their positions, since no one is
//! there to confirm the correction; so is one that says it was made more
//! than a day after the host's clock, naming its date
//! (spec/decisions/phrase-format.md).

use std::path::Path;

use dassie_identity::{NodeKey, Phrase, Positions, Realm};
use zeroize::Zeroize;

/// Most bytes in a passphrase file: a phrase, and a line break or two.
const MAX_PHRASE_FILE_BYTES: u64 = dassie_identity::MAX_PHRASE_INPUT_BYTES as u64 + 2;

/// Why no node key was read from a passphrase file. Neither names what the
/// file holds beyond a realm.
#[derive(Debug)]
pub enum Refusal {
    /// The file holds a valid phrase, of this realm rather than the one
    /// expected, which the caller may know how to ask for.
    OtherRealm {
        /// What to tell the operator.
        message: String,
        /// The phrase's realm.
        realm: Realm,
    },
    /// Any other reason, as what to tell the operator.
    Other(String),
}

impl From<Refusal> for String {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::OtherRealm { message, .. } | Refusal::Other(message) => message,
        }
    }
}

/// The node key of the passphrase in `path`, a phrase of `realm` read by a
/// clock that reads `now_ms`, derived as the wallet derives it
/// (spec/areas/identity.md#derivation-chain). The phrase and the master
/// seed are wiped before this returns, and the key wipes itself when
/// dropped.
///
/// # Errors
///
/// The file cannot be read, is too long, holds no valid passphrase, a
/// phrase of the other realm, one made more than a day after `now_ms`, or
/// one that needs correcting.
pub fn node_key_from_file(path: &Path, realm: Realm, now_ms: u64) -> Result<NodeKey, Refusal> {
    let shown = path.display();
    let mut text = read_bounded(path, MAX_PHRASE_FILE_BYTES).map_err(|problem| {
        Refusal::Other(match problem {
            ReadProblem::TooLong => format!("{shown} is too long for a passphrase"),
            ReadProblem::Io(problem) => format!("cannot read {shown}: {problem}"),
        })
    })?;
    let reading = Phrase::read(&text, Positions::NONE, realm, now_ms);
    text.zeroize();
    // The phrase's errors name positions, counts, a realm or a date, never
    // a word.
    let refusal = |error| match error {
        dassie_identity::Error::OtherRealm { realm: theirs } => Refusal::OtherRealm {
            message: format!(
                "{shown} holds a phrase of the {theirs} realm, not of the {realm} realm"
            ),
            realm: theirs,
        },
        error => Refusal::Other(format!("{shown} holds no valid passphrase: {error}")),
    };
    let phrase = reading
        .and_then(dassie_identity::Reading::without_correction)
        .map_err(refusal)?;
    let seed = phrase.master_seed(realm).map_err(refusal)?;
    Ok(seed.node_key())
}

/// The host's clock, in milliseconds since the Unix epoch. A reader with
/// no one to confirm reads a phrase only once that clock is synced, which
/// the systemd units that run one see to (`time-sync.target`).
#[must_use]
// The CLI is the edge where the time comes in.
#[allow(clippy::disallowed_types)]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Why a file was not read.
pub enum ReadProblem {
    /// It is longer than allowed.
    TooLong,
    /// It could not be read, for this reason.
    Io(String),
}

/// A text file of at most `max` bytes. The buffer is allocated once, so no
/// copy of what was read is left behind by its growing; one that is too
/// long is wiped before it is dropped.
pub fn read_bounded(path: &Path, max: u64) -> Result<String, ReadProblem> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|e| ReadProblem::Io(e.to_string()))?;
    let capacity = usize::try_from(max.saturating_add(1)).map_err(|_| ReadProblem::TooLong)?;
    let mut text = String::with_capacity(capacity);
    let read = file.take(max.saturating_add(1)).read_to_string(&mut text);
    // A file that is not UTF-8 leaves what was read in the buffer's spare
    // capacity, which zeroizing a string wipes too.
    if let Err(error) = read {
        text.zeroize();
        return Err(ReadProblem::Io(error.to_string()));
    }
    if text.len() as u64 > max {
        text.zeroize();
        return Err(ReadProblem::TooLong);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    use dassie_identity::{Created, PhraseSecret};

    fn phrase(realm: Realm) -> Phrase {
        Phrase::new(realm, made(), &PhraseSecret::from_bytes([7; 16]))
    }

    fn made() -> Created {
        Created::from_hours(7_000).unwrap()
    }

    /// The clock of a reader an hour after the phrase was made.
    fn now_ms() -> u64 {
        made().unix_ms() + 3_600_000
    }

    #[test]
    fn the_key_is_the_phrases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase");
        let phrase = phrase(Realm::Test);
        // Surrounding white space, as an editor leaves it, is no part of it.
        std::fs::write(&path, format!("  {}\n\n", phrase.text().as_str())).unwrap();
        let key = node_key_from_file(&path, Realm::Test, now_ms()).unwrap();
        let expected = phrase.master_seed(Realm::Test).unwrap().node_key();
        assert_eq!(key.node_id(), expected.node_id());
        let Some(Refusal::OtherRealm { message, realm }) =
            node_key_from_file(&path, Realm::Live, now_ms()).err()
        else {
            panic!("a test phrase read as a live one is refused as the test realm's");
        };
        assert_eq!(realm, Realm::Test);
        assert!(
            message.contains("a phrase of the test realm, not of the live realm"),
            "{message}"
        );
    }

    // No one is there to confirm a correction, so a damaged phrase is
    // refused, by the positions of the words, never the words.
    #[test]
    fn a_phrase_that_needs_correcting_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase");
        let phrase = phrase(Realm::Test);
        let mut words: Vec<&str> = phrase.words().collect();
        let lost = words[2];
        words[2] = "?";
        std::fs::write(&path, words.join(" ")).unwrap();
        let problem = String::from(
            node_key_from_file(&path, Realm::Test, now_ms())
                .err()
                .unwrap(),
        );
        assert!(
            problem.contains("at word 3, which only its owner"),
            "{problem}"
        );
        assert!(!problem.contains(lost), "{problem}");
    }

    // A phrase from more than a day after the host's clock is refused,
    // with its date, so the owner sees why.
    #[test]
    fn a_phrase_from_the_future_is_refused_with_its_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase");
        let phrase = phrase(Realm::Test);
        std::fs::write(&path, phrase.text().as_str()).unwrap();
        let behind = made().unix_ms() - 86_400_001;
        let problem = String::from(
            node_key_from_file(&path, Realm::Test, behind)
                .err()
                .unwrap(),
        );
        assert!(
            problem.contains(&format!("made on {}", made())),
            "{problem}"
        );
        assert!(node_key_from_file(&path, Realm::Test, behind + 1).is_ok());
    }

    #[test]
    fn no_error_names_what_the_file_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase");
        std::fs::write(&path, "abandon not a phrase").unwrap();
        let problem = String::from(
            node_key_from_file(&path, Realm::Test, now_ms())
                .err()
                .unwrap(),
        );
        assert!(problem.contains("no valid passphrase"), "{problem}");
        assert!(!problem.contains("abandon"), "{problem}");
        std::fs::write(&path, "abandon ".repeat(512)).unwrap();
        let problem = String::from(
            node_key_from_file(&path, Realm::Test, now_ms())
                .err()
                .unwrap(),
        );
        assert!(problem.contains("too long"), "{problem}");
        assert!(!problem.contains("abandon"), "{problem}");
        let missing = String::from(
            node_key_from_file(&dir.path().join("absent"), Realm::Test, now_ms())
                .err()
                .unwrap(),
        );
        assert!(missing.contains("cannot read"), "{missing}");
    }
}
