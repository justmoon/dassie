//! Starting at login (spec/areas/desktop.md#lifecycle).
//!
//! A periphery node is reachable only while it runs, so the app starts at
//! login unless its owner says otherwise: an XDG autostart entry starts it
//! with `--hidden`, in the tray, without a window.
//!
//! - **The default is on** where `PATH` finds the binary by its name, which
//!   an installed app is; an entry naming a path that an upgrade or a
//!   garbage collection removes, as `nix run` gives, would only go stale.
//! - **The owner's choice is kept** in the app's data directory, whether
//!   made in the tray or in the desktop's own settings, which remove the
//!   entry or turn it off in place (`Hidden=true`,
//!   `X-GNOME-Autostart-enabled=false`, `X-MATE-Autostart-enabled=false`):
//!   such an entry is not written again. An entry turned on again there
//!   counts as on.
//! - **An entry that is wanted is written again at every start,** so that it
//!   names the binary as it is now.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file in the app's data directory that keeps the owner's choice.
pub const CHOICE_FILE: &str = "start-at-login";

/// The autostart entry's file name.
pub const ENTRY_FILE: &str = "land.dassie.desktop.desktop";

/// The binary's name, which the entry uses where `PATH` finds it.
const BINARY: &str = "dassie-desktop";

/// The owner's choice: `None` before the first start that made one.
fn choice(data_dir: &Path) -> Option<bool> {
    let text = fs::read_to_string(data_dir.join(CHOICE_FILE)).ok()?;
    Some(text.trim() != "off")
}

/// Whether the app starts at login, as the tray shows it.
#[must_use]
pub fn wanted(data_dir: &Path) -> bool {
    choice(data_dir).unwrap_or(false)
}

/// At each start: makes the default choice on the first, follows what the
/// owner did to the entry in the desktop's settings since, and writes a
/// wanted entry again.
pub fn refresh(data_dir: &Path) -> io::Result<()> {
    let entry = entry_path()?;
    match (choice(data_dir), state(&entry)) {
        (None, _) => choose(data_dir, on_path()),
        (Some(true), Entry::On) => write_entry(&entry),
        (Some(true), Entry::Missing | Entry::Off) => keep(data_dir, false),
        (Some(false), Entry::On) => {
            keep(data_dir, true)?;
            write_entry(&entry)
        }
        (Some(false), Entry::Missing | Entry::Off) => Ok(()),
    }
}

/// How the autostart entry stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// There is none.
    Missing,
    /// It is there, turned off in place.
    Off,
    /// It is there, and not turned off: or it cannot be read, and is written
    /// again, so that it does not go stale.
    On,
}

fn state(path: &Path) -> Entry {
    match fs::read(path) {
        // The keys read are ASCII, so a lossy decoding cannot hide them.
        Ok(bytes) if turned_off(&String::from_utf8_lossy(&bytes)) => Entry::Off,
        Ok(_) => Entry::On,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Entry::Missing,
        Err(_) => Entry::On,
    }
}

/// Whether the entry `text` was turned off in place, as the desktops'
/// startup settings do rather than remove it: `Hidden` (KDE, Xfce, LXQt),
/// GNOME's and Cinnamon's key, and MATE's.
fn turned_off(text: &str) -> bool {
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if in_entry
            && ((key == "Hidden" && value == "true")
                || (key == "X-GNOME-Autostart-enabled" && value == "false")
                || (key == "X-MATE-Autostart-enabled" && value == "false"))
        {
            return true;
        }
    }
    false
}

/// Keeps the owner's choice, and writes or removes the entry to match.
pub fn choose(data_dir: &Path, on: bool) -> io::Result<()> {
    keep(data_dir, on)?;
    let entry = entry_path()?;
    if on {
        return write_entry(&entry);
    }
    match fs::remove_file(&entry) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn keep(data_dir: &Path, on: bool) -> io::Result<()> {
    fs::write(
        data_dir.join(CHOICE_FILE),
        if on { "on\n" } else { "off\n" },
    )
}

fn write_entry(path: &Path) -> io::Result<()> {
    let exec = command(on_path(), &std::env::current_exe()?)
        .ok_or_else(|| io::Error::other("the app's path cannot be named in an autostart entry"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, entry(&exec))
}

/// `$XDG_CONFIG_HOME/autostart/<entry>`, or `~/.config/autostart/<entry>`.
fn entry_path() -> io::Result<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::other("neither XDG_CONFIG_HOME nor HOME is set"))?;
    Ok(config.join("autostart").join(ENTRY_FILE))
}

/// Whether `PATH` finds the binary by its name.
fn on_path() -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.is_absolute() && dir.join(BINARY).is_file())
    })
}

/// What the entry runs: the binary by name where `PATH` finds it, which
/// survives an upgrade that moves it, else its path; `None` for a path the
/// entry cannot hold.
#[must_use]
pub fn command(on_path: bool, current_exe: &Path) -> Option<String> {
    if on_path {
        return Some(BINARY.into());
    }
    quote(current_exe.to_str()?)
}

/// `arg` as the Desktop Entry spec's `Exec` key holds it: quoted, with `"`,
/// `` ` ``, `$` and `\` escaped by a backslash, and then, since the value is
/// a string first, every backslash doubled; `%` doubled as a field code
/// would be. Control characters cannot be held.
fn quote(arg: &str) -> Option<String> {
    if arg.chars().any(char::is_control) {
        return None;
    }
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        return Some(arg.into());
    }
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => quoted.push_str("\\\\"),
            '\\' => quoted.push_str("\\\\\\"),
            '%' => quoted.push('%'),
            _ => {}
        }
        quoted.push(c);
    }
    quoted.push('"');
    Some(quoted)
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
        assert_eq!(command(true, exe).unwrap(), "dassie-desktop");
        assert_eq!(command(false, exe).unwrap(), exe.to_str().unwrap());
    }

    #[test]
    fn paths_are_quoted_and_escaped_for_exec() {
        // In the file: \\$ for $, \\\\ for \, and %% for %.
        let exe = Path::new("/home/a b/$x/back\\slash/100%/dassie-desktop");
        assert_eq!(
            command(false, exe).unwrap(),
            r#""/home/a b/\\$x/back\\\\slash/100%%/dassie-desktop""#
        );
    }

    #[test]
    fn paths_with_control_characters_are_refused() {
        assert_eq!(command(false, Path::new("/home/a\nb/dassie-desktop")), None);
    }

    #[test]
    fn an_entry_turned_off_in_place_is_off() {
        assert!(!turned_off(&entry("dassie-desktop")));
        let gnome = entry("dassie-desktop").replace(
            "X-GNOME-Autostart-enabled=true",
            "X-GNOME-Autostart-enabled=false",
        );
        assert!(turned_off(&gnome));
        assert!(turned_off(&format!(
            "{}Hidden=true\n",
            entry("dassie-desktop")
        )));
        assert!(turned_off("[Desktop Entry]\nHidden = true\n"));
        assert!(turned_off(
            "[Desktop Entry]\nX-MATE-Autostart-enabled=false\n"
        ));
        // Only the entry's own group counts.
        assert!(!turned_off(
            "[Desktop Entry]\nName=Dassie\n[Desktop Action x]\nHidden=true\n"
        ));
    }

    #[test]
    fn an_entry_is_read_as_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ENTRY_FILE);
        assert_eq!(state(&path), Entry::Missing);
        // Not UTF-8: still read, and its keys still count.
        fs::write(&path, b"[Desktop Entry]\nName=Dass\xefe\nExec=x\n").unwrap();
        assert_eq!(state(&path), Entry::On);
        fs::write(&path, b"[Desktop Entry]\nName=Dass\xefe\nHidden=true\n").unwrap();
        assert_eq!(state(&path), Entry::Off);
        // A directory where the entry should be cannot be read: written again.
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(state(&path), Entry::On);
    }

    #[test]
    fn the_choice_is_none_until_made() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(choice(dir.path()), None);
        assert!(!wanted(dir.path()));
        keep(dir.path(), false).unwrap();
        assert_eq!(choice(dir.path()), Some(false));
        keep(dir.path(), true).unwrap();
        assert!(wanted(dir.path()));
    }
}
