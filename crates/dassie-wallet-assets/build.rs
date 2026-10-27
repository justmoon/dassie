//! Embeds the built wallet in `$DASSIE_WALLET_DIST` as `wallet.rs`: a
//! table of files with their type, ETag and, where it pays, a gzip copy.
//! Only that variable is read, never a default path, so a stale local build
//! cannot end up in a binary unnoticed. Without it the node's placeholder
//! page stands in, so `cargo build` and `cargo test` need no Node.js.

// Cargo hands a build script its inputs in the environment, and takes its
// instructions from what it prints.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

// The node's table of media types, which its dev server reads too.
#[path = "../dassie-node/src/http/mime.rs"]
mod mime;

use mime::content_type;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};

type BuildResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A file of the wallet, as `build.rs` found it.
struct Source {
    /// The URL path, `/` and forward slashes.
    url: String,
    bytes: Vec<u8>,
}

fn main() -> BuildResult {
    let out = PathBuf::from(env::var("OUT_DIR")?);
    println!("cargo:rerun-if-env-changed=DASSIE_WALLET_DIST");
    let table = match env::var_os("DASSIE_WALLET_DIST") {
        Some(dist) => {
            let dist = PathBuf::from(dist);
            println!("cargo:rerun-if-changed={}", dist.display());
            let mut sources = vec![];
            collect(&dist, &dist, &mut sources)?;
            if !sources.iter().any(|source| source.url == "/index.html") {
                return Err(format!("{} has no index.html", dist.display()).into());
            }
            let mut table =
                String::from("use dassie_node::http::assets::Asset;\n\nstatic ASSETS: &[Asset] = &[\n");
            for (index, source) in sources.iter().enumerate() {
                write_entry(&out, index, source, &mut table)?;
            }
            table.push_str("];\n\n/// The built wallet.\npub const WALLET: Wallet = Wallet::built(ASSETS);\n");
            table
        }
        None => "/// No wallet: the node's placeholder page.\npub const WALLET: Wallet = Wallet::PLACEHOLDER;\n".into(),
    };
    fs::write(out.join("wallet.rs"), table)?;
    Ok(())
}

fn collect(root: &Path, directory: &Path, sources: &mut Vec<Source>) -> BuildResult {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    // Sorted, so the table, and the binary, don't depend on the file system.
    entries.sort_by_key(fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(root, &path, sources)?;
            continue;
        }
        let relative = path.strip_prefix(root)?;
        let url = relative
            .components()
            .map(|part| part.as_os_str().to_str().ok_or("non-UTF-8 file name"))
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        let safe = url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
        if !safe {
            return Err(format!("unexpected characters in wallet file name {url}").into());
        }
        sources.push(Source {
            url: format!("/{url}"),
            bytes: fs::read(&path)?,
        });
    }
    Ok(())
}

fn write_entry(out: &Path, index: usize, source: &Source, table: &mut String) -> BuildResult {
    let extension = source
        .url
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension);
    let (content_type, compressible) = content_type(extension);
    let raw = out.join(format!("wallet-{index}"));
    fs::write(&raw, &source.bytes)?;
    let digest = Sha256::digest(&source.bytes);
    let etag: String = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let gzip = if compressible {
        gzip(&source.bytes)?
    } else {
        None
    };
    let gzip = match gzip {
        Some(bytes) => {
            let path = out.join(format!("wallet-{index}.gz"));
            fs::write(&path, bytes)?;
            format!("Some(include_bytes!({:?}))", path.display().to_string())
        }
        None => "None".into(),
    };
    writeln!(
        table,
        "    Asset {{ path: {:?}, content_type: {content_type:?}, etag: \"\\\"{etag}\\\"\", \
         body: include_bytes!({:?}), gzip: {gzip} }},",
        source.url,
        raw.display().to_string(),
    )?;
    Ok(())
}

/// The gzip of `bytes`, if it is smaller. The header carries no file name
/// or time, so the output depends on the input alone.
fn gzip(bytes: &[u8]) -> BuildResult<Option<Vec<u8>>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(bytes)?;
    let compressed = encoder.finish()?;
    Ok((compressed.len() < bytes.len()).then_some(compressed))
}
