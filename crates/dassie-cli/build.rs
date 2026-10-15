//! Builds in the release key, which `dassie update` trusts releases with
//! (spec D11): the minisign public key in the file `DASSIE_RELEASE_KEY`
//! names, or else `resources/minisign.pub` at the repo root. With the
//! `test-key` feature, the test key of `testdata/release` instead, for
//! tests only. With none of them, the binary trusts no key, and `dassie
//! update` refuses to run. A key that does not parse fails the build.

use std::path::{Path, PathBuf};
use std::{env, fs};

use dassie_release::PublicKey;

type BuildResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BuildResult {
    println!("cargo:rerun-if-env-changed=DASSIE_RELEASE_KEY");
    let resources = Path::new("../../resources");
    println!("cargo:rerun-if-changed={}", resources.display());
    let path: Option<PathBuf> = if env::var_os("CARGO_FEATURE_TEST_KEY").is_some() {
        Some("../../testdata/release/test.pub".into())
    } else if let Some(path) = env::var_os("DASSIE_RELEASE_KEY") {
        Some(path.into())
    } else {
        Some(resources.join("minisign.pub")).filter(|path| path.exists())
    };
    let key = match &path {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.display());
            let text = fs::read_to_string(path)?;
            PublicKey::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            Some(text)
        }
        None => None,
    };
    let out = PathBuf::from(env::var("OUT_DIR")?).join("release_key.rs");
    fs::write(
        out,
        format!(
            "/// The release key this binary trusts, as `minisign.pub` holds it.\n\
             pub const RELEASE_KEY: Option<&str> = {key:?};\n"
        ),
    )?;
    Ok(())
}
