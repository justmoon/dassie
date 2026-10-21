//! `dassie-release`: the release workflow's checks, with the code the
//! updater trusts a release with.
//!
//! - `dassie-release verify <tree> <minisign.pub>` checks a signed tree for
//!   the mirror: one `meta/<channel>` naming a version of that channel, its
//!   `SHA256SUMS` signed for that version by this key, every bundle and the
//!   flake listed, and each matching its sum.
//! - `dassie-release supersedes <new> <old>` exits 0 if the updater would
//!   move from `old` to `new` on its own, 1 if not: the workflow never moves
//!   `meta/<channel>` back.

use std::path::Path;
use std::process::ExitCode;
use std::{env, fs};

use dassie_release::layout::{self, Arch, Compression};
use dassie_release::{Channel, PublicKey, Version, verify_sums};
use sha2::{Digest as _, Sha256};

const USAGE: &str = "\
Usage: dassie-release verify <tree> <minisign.pub>
       dassie-release supersedes <new version> <old version>
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["verify", tree, key] => verify(Path::new(tree), Path::new(key)),
        ["supersedes", new, old] => {
            return match (Version::parse(new), Version::parse(old)) {
                (Ok(new), Ok(old)) if new.supersedes(&old) => ExitCode::SUCCESS,
                (Ok(_), Ok(_)) => ExitCode::from(1),
                (Err(error), _) | (_, Err(error)) => {
                    eprintln!("dassie-release: {error}");
                    ExitCode::from(2)
                }
            };
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("dassie-release: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn verify(tree: &Path, key: &Path) -> Result<String, String> {
    let read = |path: &Path| fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    let key = String::from_utf8(read(key)?).map_err(|_| "the key is not text".to_string())?;
    let key = PublicKey::parse(&key).map_err(|e| format!("the key was refused: {e}"))?;
    let (channel, version) = release_of(tree)?;
    let dir = tree.join(version.to_string());
    let signature = String::from_utf8(read(&dir.join(layout::SIGNATURE))?)
        .map_err(|_| "the signature is not text".to_string())?;
    let sums = verify_sums(&key, &read(&dir.join(layout::SUMS))?, &signature, &version)
        .map_err(|e| e.to_string())?;
    let expected: Vec<String> = Arch::ALL
        .iter()
        .flat_map(|arch| {
            [Compression::Gz, Compression::Xz]
                .map(|compression| layout::bundle(&version, *arch, compression))
        })
        .chain([layout::FLAKE.to_string()])
        .collect();
    let listed: Vec<&str> = sums.names().collect();
    if listed.len() != expected.len()
        || !expected.iter().all(|name| listed.contains(&name.as_str()))
    {
        return Err(format!("SHA256SUMS lists {listed:?}, not {expected:?}"));
    }
    for name in &expected {
        let digest = Sha256::digest(read(&dir.join(name))?);
        if sums.digest(name).map(<[u8; 32]>::as_slice) != Some(digest.as_slice()) {
            return Err(format!("{name} does not match SHA256SUMS"));
        }
    }
    Ok(format!(
        "verified dassie {version} on {} with key {}",
        channel.name(),
        key.id()
    ))
}

/// The one `meta/<channel>` of a tree, and the version it names, which must
/// be of that channel.
fn release_of(tree: &Path) -> Result<(Channel, Version), String> {
    let meta = tree.join("meta");
    let entries: Vec<String> = fs::read_dir(&meta)
        .map_err(|e| format!("{}: {e}", meta.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    let [name] = entries.as_slice() else {
        return Err(format!("meta/ holds {entries:?}, not one channel"));
    };
    let channel = Channel::parse(name).ok_or_else(|| format!("unknown channel {name:?}"))?;
    let text = fs::read_to_string(meta.join(name)).map_err(|e| format!("meta/{name}: {e}"))?;
    let version = Version::parse(&text).map_err(|e| format!("meta/{name}: {e}"))?;
    if version.channel() != channel {
        return Err(format!("meta/{name} names {version}, of another channel"));
    }
    Ok((channel, version))
}
