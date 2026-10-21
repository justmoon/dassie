//! What the app and its window write is its owner's alone
//! (spec/areas/desktop.md#private-files).
//!
//! The window's web engine keeps the wallet's session cookie, its cache and
//! its storage in the app's data directory, with whatever modes the umask
//! gives, and a session cookie lets whoever reads it use the wallet without
//! the passphrase. So the app makes its files with a umask of 0077, as the
//! server's systemd unit runs the node, and keeps its data directory at
//! 0700, tightening one that is looser, as the node does its own
//! (spec/areas/node.md#database).

use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;

use rustix::fs::Mode;

/// From now on, files and directories this process makes are its user's
/// alone.
pub fn umask() {
    rustix::process::umask(Mode::from_raw_mode(0o077));
}

/// Makes `dir` with mode 0700, or tightens it to that.
pub fn directory(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let mode = fs::metadata(dir)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        tracing::warn!(target: "dassie_desktop", dir = %dir.display(), from = format_args!("{mode:o}"), "tightening permissions");
        fs::set_permissions(dir, Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_directory_is_made_private() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("a/b");
        directory(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn a_looser_directory_is_tightened() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("app");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, Permissions::from_mode(0o755)).unwrap();
        directory(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }
}
