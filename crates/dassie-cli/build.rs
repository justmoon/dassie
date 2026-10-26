//! Builds in the release key, which `dassie update` trusts releases with
//! (spec/decisions/release-integrity.md): the minisign public key in the
//! file `DASSIE_RELEASE_KEY` names, or else `resources/minisign.pub` at the
//! repo root. With the `test-key` feature, the test key of `testdata/release`
//! instead, for tests only. Every build trusts a key: one that is missing or
//! does not parse fails the build, since a binary without one could never
//! update.

use std::path::{Path, PathBuf};
use std::{env, fs};

use dassie_release::PublicKey;

type BuildResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BuildResult {
    println!("cargo:rerun-if-env-changed=DASSIE_RELEASE_KEY");
    let resources = Path::new("../../resources");
    println!("cargo:rerun-if-changed={}", resources.display());
    let path: PathBuf = if env::var_os("CARGO_FEATURE_TEST_KEY").is_some() {
        "../../testdata/release/test.pub".into()
    } else if let Some(path) = env::var_os("DASSIE_RELEASE_KEY") {
        path.into()
    } else {
        resources.join("minisign.pub")
    };
    println!("cargo:rerun-if-changed={}", path.display());
    let key = fs::read_to_string(&path)
        .map_err(|e| format!("the release key, {}: {e}", path.display()))?;
    PublicKey::parse(&key).map_err(|e| format!("{}: {e}", path.display()))?;
    let out = PathBuf::from(env::var("OUT_DIR")?).join("release_key.rs");
    fs::write(
        out,
        format!(
            "/// The release key this binary trusts, as `minisign.pub` holds it.\n\
             pub const RELEASE_KEY: &str = {key:?};\n"
        ),
    )?;
    Ok(())
}
