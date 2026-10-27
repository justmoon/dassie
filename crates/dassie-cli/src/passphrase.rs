//! Reading an owner's passphrase from a file, for the two commands that
//! take one: `dassie setup` and `dassie market-maker`
//! (spec/areas/node.md#the-dassie-command).
//!
//! The file is read once and bounded in size, the phrase is wiped as soon
//! as the node key is derived from it, and no error names what the file
//! holds: the phrase is the owner's only backup and the key to every
//! ledger account the node has. A phrase that reads only by correcting
//! some of its words is refused, naming their positions, since no one is
//! there to confirm the correction (spec/decisions/phrase-format.md).

use std::path::Path;

use dassie_identity::{NodeKey, Phrase, Positions, Realm};
use zeroize::Zeroize;

/// Most bytes in a passphrase file: a phrase, and a line break or two.
const MAX_PHRASE_FILE_BYTES: u64 = dassie_identity::MAX_PHRASE_INPUT_BYTES as u64 + 2;

/// The node key of the passphrase in `path`, a phrase of `realm`, derived
/// as the wallet derives it (spec/areas/identity.md#derivation-chain). The
/// phrase and the master seed are wiped before this returns, and the key
/// wipes itself when dropped.
///
/// # Errors
///
/// The file cannot be read, is too long, holds no valid passphrase, a
/// phrase of the other realm, or one that needs correcting.
pub fn node_key_from_file(path: &Path, realm: Realm) -> Result<NodeKey, String> {
    let shown = path.display();
    let mut text = read_bounded(path, MAX_PHRASE_FILE_BYTES).map_err(|problem| match problem {
        ReadProblem::TooLong => format!("{shown} is too long for a passphrase"),
        ReadProblem::Io(problem) => format!("cannot read {shown}: {problem}"),
    })?;
    let reading = Phrase::read(&text, Positions::NONE, realm);
    text.zeroize();
    // The phrase's errors name positions and counts, never a word.
    let phrase = reading
        .and_then(dassie_identity::Reading::without_correction)
        .map_err(|error| format!("{shown} holds no valid passphrase: {error}"))?;
    let seed = phrase
        .master_seed(realm)
        .map_err(|error| format!("{shown} holds no valid passphrase: {error}"))?;
    Ok(seed.node_key())
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
        let created = Created::from_hours(7_000).unwrap();
        Phrase::new(realm, created, &PhraseSecret::from_bytes([7; 16]))
    }

    #[test]
    fn the_key_is_the_phrases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase");
        let phrase = phrase(Realm::Test);
        // Surrounding white space, as an editor leaves it, is no part of it.
        std::fs::write(&path, format!("  {}\n\n", phrase.text().as_str())).unwrap();
        let key = node_key_from_file(&path, Realm::Test).unwrap();
        let expected = phrase.master_seed(Realm::Test).unwrap().node_key();
        assert_eq!(key.node_id(), expected.node_id());
        let problem = node_key_from_file(&path, Realm::Live).err().unwrap();
        assert!(problem.contains("phrase of the test realm"), "{problem}");
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
        let problem = node_key_from_file(&path, Realm::Test).err().unwrap();
        assert!(
            problem.contains("at word 3, which only its owner"),
            "{problem}"
        );
        assert!(!problem.contains(lost), "{problem}");
    }

    #[test]
    fn no_error_names_what_the_file_holds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase");
        std::fs::write(&path, "abandon not a phrase").unwrap();
        let problem = node_key_from_file(&path, Realm::Test).err().unwrap();
        assert!(problem.contains("no valid passphrase"), "{problem}");
        assert!(!problem.contains("abandon"), "{problem}");
        std::fs::write(&path, "abandon ".repeat(512)).unwrap();
        let problem = node_key_from_file(&path, Realm::Test).err().unwrap();
        assert!(problem.contains("too long"), "{problem}");
        assert!(!problem.contains("abandon"), "{problem}");
        let missing = node_key_from_file(&dir.path().join("absent"), Realm::Test)
            .err()
            .unwrap();
        assert!(missing.contains("cannot read"), "{missing}");
    }
}
