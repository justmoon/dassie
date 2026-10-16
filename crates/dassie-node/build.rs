//! Generates the RPC contract's messages and embeds the wallet.
//!
//! - `proto/dassie/ui/v1` at the repo root becomes `dassie.ui.v1.rs`.
//!   protox parses it in Rust, so no `protoc` binary is needed.
//! - The built wallet in `$DASSIE_WALLET_DIST` becomes `wallet_assets.rs`, a
//!   table of files with their type, ETag and, where it pays, a gzip copy.
//!   Only that variable is read, never a default path, so a stale local
//!   build cannot end up in a binary unnoticed. Without it the table holds a
//!   page saying the binary has no wallet, so `cargo build` and `cargo test`
//!   need no Node.js.

// Cargo hands a build script its inputs in the environment, and takes its
// instructions from what it prints.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

#[path = "src/http/mime.rs"]
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

fn main() -> BuildResult {
    let out = PathBuf::from(env::var("OUT_DIR")?);
    generate_contract()?;
    embed_wallet(&out)
}

fn generate_contract() -> BuildResult {
    let root = "../../proto";
    println!("cargo:rerun-if-changed={root}/dassie/ui/v1");
    let file = format!("{root}/dassie/ui/v1/transport.proto");
    let descriptors = protox::compile([file], [root])?;
    prost_build::Config::new().compile_fds(descriptors)?;
    Ok(())
}

/// A file of the wallet, as `build.rs` found it.
struct Source {
    /// The URL path, `/` and forward slashes.
    url: String,
    bytes: Vec<u8>,
}

fn embed_wallet(out: &Path) -> BuildResult {
    println!("cargo:rerun-if-env-changed=DASSIE_WALLET_DIST");
    let (sources, built) = match env::var_os("DASSIE_WALLET_DIST") {
        Some(dist) => {
            let dist = PathBuf::from(dist);
            println!("cargo:rerun-if-changed={}", dist.display());
            let mut sources = vec![];
            collect(&dist, &dist, &mut sources)?;
            if !sources.iter().any(|source| source.url == "/index.html") {
                return Err(format!("{} has no index.html", dist.display()).into());
            }
            (sources, true)
        }
        None => (vec![placeholder()], false),
    };
    let mut table =
        String::from("/// The wallet's files, sorted by path.\npub static ASSETS: &[Asset] = &[\n");
    for (index, source) in sources.iter().enumerate() {
        write_entry(out, index, source, &mut table)?;
    }
    table.push_str("];\n");
    writeln!(
        table,
        "/// Whether this is a built wallet rather than the placeholder.\npub const WALLET_BUILT: bool = {built};"
    )?;
    fs::write(out.join("wallet_assets.rs"), table)?;
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

fn placeholder() -> Source {
    let page = "<!doctype html>\n<meta charset=\"utf-8\">\n<title>Dassie</title>\n\
        <p>This Dassie binary was built without its wallet. Build the wallet \
        (<code>node --run build:wallet</code>) and rebuild with \
        <code>DASSIE_WALLET_DIST</code> pointing at it.</p>\n";
    Source {
        url: "/index.html".into(),
        bytes: page.as_bytes().to_vec(),
    }
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
