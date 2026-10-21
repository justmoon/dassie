//! Starting at login (spec/areas/desktop.md#lifecycle).
//!
//! A periphery node is reachable only while it runs, so the app starts at
//! login unless its owner says otherwise: an XDG autostart entry starts it
//! with `--hidden`, in the tray, without a window. The owner's choice is
//! kept in the app's data directory, so that an entry the owner removed
//! stays removed. The entry is written again at every start while it is
//! wanted, so that it names the binary as it is now.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file in the app's data directory that keeps the owner's choice.
pub const CHOICE_FILE: &str = "start-at-login";

/// The autostart entry's file name.
pub const ENTRY_FILE: &str = "land.dassie.desktop.desktop";

/// The binary's name, which the entry uses where `PATH` finds it.
const BINARY: &str = "dassie-desktop";

/// The owner's choice, or the default, on, if they made none.
#[must_use]
pub fn wanted(data_dir: &Path) -> bool {
    fs::read_to_string(data_dir.join(CHOICE_FILE)).map_or(true, |text| text.trim() != "off")
}

/// Keeps the owner's choice, and writes or removes the entry to match.
pub fn choose(data_dir: &Path, on: bool) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    fs::write(
        data_dir.join(CHOICE_FILE),
        if on { "on\n" } else { "off\n" },
    )?;
    apply(on)
}

/// Writes the entry if `on`, else removes it.
pub fn apply(on: bool) -> io::Result<()> {
    let Some(dir) = autostart_dir() else {
        return Err(io::Error::other("neither XDG_CONFIG_HOME nor HOME is set"));
    };
    let path = dir.join(ENTRY_FILE);
    if !on {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let exec = command(on_path(), &std::env::current_exe()?);
    fs::create_dir_all(&dir)?;
    fs::write(path, entry(&exec))
}

/// `$XDG_CONFIG_HOME/autostart`, or `~/.config/autostart`.
fn autostart_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart"))
}

/// Whether `PATH` finds the binary by its name.
fn on_path() -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.is_absolute() && dir.join(BINARY).is_file())
    })
}

/// What the entry runs: the binary by name where `PATH` finds it, which
/// survives an upgrade that moves it (a Nix profile does), else its path.
#[must_use]
pub fn command(on_path: bool, current_exe: &Path) -> String {
    if on_path {
        return BINARY.into();
    }
    quote(&current_exe.to_string_lossy())
}

/// `arg` quoted as the Desktop Entry spec's `Exec` key needs.
fn quote(arg: &str) -> String {
    let plain = arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        return arg.into();
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        if c == '%' {
            quoted.push('%');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// The entry for `exec`.
#[must_use]
pub fn entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Dassie\n\
         Comment=Keeps your Dassie node running\n\
         Exec={exec} --hidden\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_starts_the_app_hidden() {
        let text = entry("dassie-desktop");
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("\nExec=dassie-desktop --hidden\n"));
    }

    #[test]
    fn the_binary_is_named_by_path_where_path_finds_it() {
        let exe = Path::new("/nix/store/abc-dassie-desktop/bin/dassie-desktop");
        assert_eq!(command(true, exe), "dassie-desktop");
        assert_eq!(command(false, exe), exe.to_str().unwrap());
    }

    #[test]
    fn paths_are_quoted_for_exec() {
        let exe = Path::new("/home/a b/$x/100%/dassie-desktop");
        assert_eq!(
            command(false, exe),
            "\"/home/a b/\\$x/100%%/dassie-desktop\""
        );
    }

    #[test]
    fn the_choice_defaults_to_on() {
        let dir = tempfile::tempdir().unwrap();
        assert!(wanted(dir.path()));
        fs::write(dir.path().join(CHOICE_FILE), "off\n").unwrap();
        assert!(!wanted(dir.path()));
        fs::write(dir.path().join(CHOICE_FILE), "on\n").unwrap();
        assert!(wanted(dir.path()));
    }
}
