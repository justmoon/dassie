//! `dassie update`: installs a newer release from the mirror
//! (spec/areas/release.md#dassie-update, D11).
//!
//! The install is `/opt/dassie/<version>`, with `/opt/dassie/current`
//! pointing at the one that runs. The updater finds it from its own path,
//! which must be `<base>/<its version>/bin/dassie`. Then:
//!
//! 1. It asks `meta/<channel>` for the newest version, and stops unless
//!    that supersedes this one (`Version::supersedes`): a mirror cannot take
//!    a node back or to another channel. The owner can, with
//!    `--target-version` or `--channel`.
//! 2. It verifies `SHA256SUMS` with the release key built into the binary,
//!    then the bundle against its sum, before it opens the bundle.
//! 3. It unpacks only plain files and directories under `dassie/`
//!    (`dassie_release::bundle`), into a staging directory beside the
//!    versions, and asks the new binary for its version and to verify its
//!    install.
//! 4. It moves it into place, installs its systemd units, points `current`
//!    at it with one `rename`, and restarts the node. Should any of that
//!    fail, the old version's units go back, and those the new one added
//!    are disabled and removed.
//! 5. If the node does not come up, it points `current` back, puts back the
//!    database from before the new version migrated it, if it did
//!    (`dassie_node::store::restore_pre_migration`), and starts the old
//!    version again.
//! 6. It keeps the version it came from, and removes older ones.
//!
//! One update runs at a time: each holds a lock on `<base>/.update.lock`.
//! Each records how it went in `<base>/last-update`, for `dassie status`.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use dassie_node::peer_client::{PeerClient, WebRequest};
use dassie_node::store::{self, ROLLED_BACK_FILE, Restored};
use dassie_release::bundle::{self, Kind, Unpacking};
use dassie_release::layout::{self, Arch, Compression};
use dassie_release::{Channel, PublicKey, Sums, Version, verify_sums};
use sha2::{Digest as _, Sha256};

include!(concat!(env!("OUT_DIR"), "/release_key.rs"));

/// Where systemd units go, unless `DASSIE_SYSTEMD_UNIT_DIR` says otherwise.
const UNIT_DIR: &str = "/etc/systemd/system";
/// The node's service, restarted once `current` moved.
const SERVICE: &str = "dassie.service";
/// The node's data directory under systemd, unless `DASSIE_STATE_DIRECTORY`
/// says otherwise.
const STATE_DIRECTORY: &str = "/var/lib/dassie";
/// Where each update records how it went, in the install's base.
pub const LAST_UPDATE_FILE: &str = "last-update";
/// How long fetching `meta/<channel>`, the sums or the signature may take.
const SMALL_TIMEOUT_MS: u64 = 30_000;
/// How long fetching a bundle may take.
const BUNDLE_TIMEOUT_MS: u64 = 600_000;
/// The longest `meta/<channel>` read.
const MAX_META: usize = 128;
/// The longest signature read.
const MAX_SIGNATURE: usize = 1024;

/// What `dassie update` was asked to do.
#[derive(Debug, Default)]
pub struct Options {
    /// Install even what is already installed, or older.
    pub force: bool,
    /// This version rather than the newest of the channel.
    pub target: Option<String>,
    /// This channel rather than the installed version's.
    pub channel: Option<String>,
    /// This mirror rather than `DASSIE_MIRROR_URL` or get.dassie.land.
    pub mirror: Option<String>,
    /// Neither talk to systemd nor restart the node.
    pub no_restart: bool,
}

/// What went wrong, for the owner.
type Problem = String;

/// Runs `dassie update`.
pub fn run(options: Options) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::FAILURE;
    };
    match runtime.block_on(update(options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("Error: {problem}");
            ExitCode::FAILURE
        }
    }
}

/// The install this binary belongs to.
pub struct Install {
    /// `/opt/dassie`.
    pub base: PathBuf,
    /// The version running.
    version: Version,
}

/// How an update went.
#[derive(Debug)]
enum Outcome {
    /// There was nothing newer.
    UpToDate,
    /// The node runs this version now.
    Updated(Version),
    /// This version did not come up, and the one before runs again.
    RolledBack(Version, Problem),
    /// The update stopped, or could not go back: the problem says which.
    Failed(Problem),
}

async fn update(options: Options) -> Result<(), Problem> {
    let install = locate()?;
    let key = release_key()?;
    let _lock = lock(&install.base)?;
    let outcome = match attempt(&options, &install, &key).await {
        Ok(outcome) => outcome,
        Err(problem) => Outcome::Failed(problem),
    };
    if let Err(problem) = record(&install, &outcome) {
        eprintln!("Note: could not record how the update went: {problem}");
    }
    match outcome {
        Outcome::UpToDate | Outcome::Updated(_) => Ok(()),
        Outcome::RolledBack(target, _) => Err(format!(
            "dassie {target} did not start, and {} runs again",
            install.version
        )),
        Outcome::Failed(problem) => Err(problem),
    }
}

async fn attempt(
    options: &Options,
    install: &Install,
    key: &PublicKey,
) -> Result<Outcome, Problem> {
    let mirror = Mirror::new(options.mirror.as_deref())?;
    let Some(target) = choose(options, install, &mirror).await? else {
        return Ok(Outcome::UpToDate);
    };
    println!("Downloading dassie {target} from {}...", mirror.url);
    let bundle = mirror.bundle(key, &target).await?;
    let staging = install
        .base
        .join(format!(".staging-{target}-{}", std::process::id()));
    let installed =
        stage(&bundle, &staging, &target).and_then(|root| place(&install.base, &root, &target));
    // The staging directory goes either way; what it held is in place.
    let _ = fs::remove_dir_all(&staging);
    let dir = installed?;
    let mut units = Units::default();
    let switched = install_units(&dir.join("share/systemd"), &mut units)
        .and_then(|()| switch(&install.base, &target));
    if let Err(problem) = switched {
        // systemd has not read the new units yet: with the old ones and
        // `current` back on disk, nothing changed.
        return Err(match put_back(install, &units) {
            Ok(()) => problem,
            Err(failed) => format!(
                "{problem}, and going back to {} failed: {failed}",
                install.version
            ),
        });
    }
    println!("Installed dassie {target}.");
    // Whatever the new version writes from here on, a copy of the database
    // from before its migration included, is this update's.
    let since = SystemTime::now();
    let restarted = if !options.no_restart && systemd_running() {
        restart(&units)
    } else {
        Ok(())
    };
    if let Err(problem) = restarted {
        eprintln!(
            "dassie {target} did not start ({problem}); going back to {}.",
            install.version
        );
        if let Err(failed) = roll_back(install, &units, since) {
            return Err(format!(
                "dassie {target} did not start ({problem}), and going back to {} failed: {failed}",
                install.version
            ));
        }
        return Ok(Outcome::RolledBack(target, problem));
    }
    prune(&install.base, &[&target, &install.version]);
    Ok(Outcome::Updated(target))
}

/// Makes systemd take in the units that changed, enables the new ones, and
/// restarts the node, which must come up.
fn restart(units: &Units) -> Result<(), Problem> {
    if units.changed {
        systemctl(&["daemon-reload"])?;
    }
    for unit in &units.added {
        systemctl(&["enable", unit])?;
    }
    println!("Restarting {SERVICE}...");
    systemctl(&["restart", SERVICE])
}

/// Points `current` back at the version that ran, with its units, puts back
/// the database from before the update if the new version migrated it, and
/// starts the old version. The units the update added are disabled and go.
fn roll_back(install: &Install, units: &Units, since: SystemTime) -> Result<(), Problem> {
    for unit in &units.added {
        systemctl(&["disable", "--now", unit])?;
    }
    put_back(install, units)?;
    systemctl(&["daemon-reload"])?;
    // systemd may be restarting the failed node; the database must be left
    // alone while it is put back.
    systemctl(&["stop", SERVICE])?;
    let restored = restore_database(since);
    let started = systemctl(&["start", SERVICE]);
    restored.and(started)
}

/// Puts back the database from before the update, if the version that
/// failed migrated it.
fn restore_database(since: SystemTime) -> Result<(), Problem> {
    let data_dir = std::env::var_os("DASSIE_STATE_DIRECTORY")
        .map_or_else(|| PathBuf::from(STATE_DIRECTORY), PathBuf::from);
    match store::restore_pre_migration(&data_dir, since) {
        Ok(Restored::Nothing) => Ok(()),
        Ok(Restored::Copy { from, to }) => {
            println!(
                "Put back the database from before the update (schema {to}). The one the new \
                 version left (schema {from}) is {}.",
                data_dir.join(ROLLED_BACK_FILE).display()
            );
            Ok(())
        }
        Err(error) => Err(format!("{}: {error}", data_dir.display())),
    }
}

/// How the last update went, as `<base>/last-update` has it.
#[derive(Debug, PartialEq, Eq)]
pub struct LastUpdate {
    /// When it ran.
    pub at_unix_ms: u64,
    /// `up-to-date`, `updated`, `rolled-back` or `failed`.
    pub outcome: String,
    /// The version it installed or tried, if any.
    pub target: Option<String>,
    /// What went wrong, if anything.
    pub problem: Option<String>,
}

impl LastUpdate {
    /// The record, as lines of `key=value`.
    fn to_text(&self) -> String {
        let mut text = format!("at={}\noutcome={}\n", self.at_unix_ms, self.outcome);
        if let Some(target) = &self.target {
            text.push_str(&format!("target={target}\n"));
        }
        if let Some(problem) = &self.problem {
            // One line, whatever the problem said.
            let line = problem.split_whitespace().collect::<Vec<_>>().join(" ");
            text.push_str(&format!("problem={line}\n"));
        }
        text
    }

    /// Reads a record; `None` if it is not one.
    fn parse(text: &str) -> Option<Self> {
        let (mut at, mut outcome, mut target, mut problem) = (None, None, None, None);
        for line in text.lines() {
            match line.split_once('=')? {
                ("at", value) => at = value.parse().ok(),
                ("outcome", value) => outcome = Some(value.to_owned()),
                ("target", value) => target = Some(value.to_owned()),
                ("problem", value) => problem = Some(value.to_owned()),
                _ => {}
            }
        }
        Some(Self {
            at_unix_ms: at?,
            outcome: outcome?,
            target,
            problem,
        })
    }
}

/// Writes how the update went to `<base>/last-update`, readable by all.
fn record(install: &Install, outcome: &Outcome) -> Result<(), Problem> {
    let at_unix_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let (outcome, target, problem) = match outcome {
        Outcome::UpToDate => ("up-to-date", None, None),
        Outcome::Updated(target) => ("updated", Some(target.to_string()), None),
        Outcome::RolledBack(target, problem) => (
            "rolled-back",
            Some(target.to_string()),
            Some(problem.clone()),
        ),
        Outcome::Failed(problem) => ("failed", None, Some(problem.clone())),
    };
    let text = LastUpdate {
        at_unix_ms,
        outcome: outcome.into(),
        target,
        problem,
    }
    .to_text();
    let path = install.base.join(LAST_UPDATE_FILE);
    let staged = install
        .base
        .join(format!(".{LAST_UPDATE_FILE}-{}", std::process::id()));
    let write = || -> io::Result<()> {
        let mut file = File::create(&staged)?;
        io::Write::write_all(&mut file, text.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        file.sync_all()
    };
    write().map_err(|e| permission(&staged, &e))?;
    fs::rename(&staged, &path).map_err(|e| permission(&path, &e))
}

/// How the last update of this install went, if one recorded it.
pub fn last_update(install: &Install) -> Option<LastUpdate> {
    let text = fs::read_to_string(install.base.join(LAST_UPDATE_FILE)).ok()?;
    LastUpdate::parse(&text)
}

/// Finds the install from this binary's path.
pub fn locate() -> Result<Install, Problem> {
    let not_installed = |why: &str| {
        format!(
            "this binary is not one the installer installed ({why}); \
             it must run from /opt/dassie/<version>/bin/dassie"
        )
    };
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| not_installed(&e.to_string()))?;
    let root = exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| not_installed("no install directory"))?;
    if root.file_name().and_then(|name| name.to_str()) != Some(dassie_node::VERSION) {
        return Err(not_installed(&format!(
            "{} is not named {}",
            root.display(),
            dassie_node::VERSION
        )));
    }
    let base = root
        .parent()
        .ok_or_else(|| not_installed("no base directory"))?;
    let version =
        Version::parse(dassie_node::VERSION).map_err(|e| format!("this binary's version: {e}"))?;
    Ok(Install {
        base: base.to_path_buf(),
        version,
    })
}

/// The release key built in.
pub fn release_key() -> Result<PublicKey, Problem> {
    let text =
        RELEASE_KEY.ok_or("this binary trusts no release key, so it cannot verify an update")?;
    PublicKey::parse(text).map_err(|e| format!("the built-in release key: {e}"))
}

/// Holds `<base>/.update.lock` for as long as it lives.
fn lock(base: &Path) -> Result<File, Problem> {
    let path = base.join(".update.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| permission(&path, &e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err("another update is running".into()),
        Err(fs::TryLockError::Error(error)) => Err(permission(&path, &error)),
    }
}

/// An IO error, with the hint the original gave for permissions.
fn permission(path: &Path, error: &io::Error) -> Problem {
    if error.kind() == io::ErrorKind::PermissionDenied {
        format!(
            "{}: insufficient permissions; run this command as root or with sudo",
            path.display()
        )
    } else {
        format!("{}: {error}", path.display())
    }
}

/// Which version to install, if any: the one asked for, or the channel's
/// newest if it supersedes the one running.
async fn choose(
    options: &Options,
    install: &Install,
    mirror: &Mirror,
) -> Result<Option<Version>, Problem> {
    if let Some(target) = &options.target {
        let target = Version::parse(target).map_err(|e| format!("--target-version: {e}"))?;
        if target == install.version && !options.force {
            println!("Already up to date ({target}).");
            return Ok(None);
        }
        return Ok(Some(target));
    }
    let channel = match &options.channel {
        Some(name) => {
            Channel::parse(name).ok_or_else(|| format!("--channel: no channel {name:?}"))?
        }
        None => install.version.channel(),
    };
    println!(
        "Checking {} for the newest {} version...",
        mirror.url,
        channel.name()
    );
    let offered = mirror.newest(channel).await?;
    let switching = channel != install.version.channel();
    if offered == install.version && !options.force {
        println!("Already up to date ({offered}).");
        Ok(None)
    } else if switching || options.force || offered.supersedes(&install.version) {
        Ok(Some(offered))
    } else {
        println!(
            "The mirror offers {offered}, which is not newer than {}: not updating.",
            install.version
        );
        Ok(None)
    }
}

/// The mirror and a client for it.
struct Mirror {
    url: String,
    client: PeerClient,
}

impl Mirror {
    /// The mirror named by `--mirror`, `DASSIE_MIRROR_URL` or the default:
    /// HTTPS only. `DASSIE_MIRROR_CA_FILE` adds roots to trust it by.
    fn new(url: Option<&str>) -> Result<Self, Problem> {
        let url = url
            .map(str::to_owned)
            .or_else(|| std::env::var("DASSIE_MIRROR_URL").ok())
            .unwrap_or_else(|| layout::DEFAULT_MIRROR.into());
        let url = url.trim_end_matches('/').to_owned();
        if !url.starts_with("https://") {
            return Err(format!("the mirror {url:?} is not an https:// URL"));
        }
        let roots = std::env::var_os("DASSIE_MIRROR_CA_FILE").map(PathBuf::from);
        // The owner names the mirror, so it may be on a private network.
        let client = PeerClient::new(roots.as_deref(), true)?;
        Ok(Self { url, client })
    }

    async fn get(&self, path: &str, max_bytes: usize, timeout_ms: u64) -> Result<Vec<u8>, Problem> {
        let url = format!("{}/{path}", self.url);
        let response = self
            .client
            .web(WebRequest {
                method: http::Method::GET,
                url: url.clone(),
                headers: Vec::new(),
                body: None,
                timeout_ms,
                max_response_bytes: max_bytes,
            })
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        match response.status {
            200 => Ok(response.body),
            status => Err(format!("{url}: the mirror answered {status}")),
        }
    }

    async fn newest(&self, channel: Channel) -> Result<Version, Problem> {
        let body = self
            .get(&layout::meta(channel), MAX_META, SMALL_TIMEOUT_MS)
            .await?;
        let text = String::from_utf8_lossy(&body);
        let version =
            Version::parse(text.trim()).map_err(|e| format!("meta/{}: {e}", channel.name()))?;
        // The file is not signed: a version of the other channel would let
        // the mirror move a node switching channels anywhere, back included.
        if version.channel() == channel {
            Ok(version)
        } else {
            Err(format!(
                "the mirror's meta/{name} names {version}, which is not a {name} version",
                name = channel.name()
            ))
        }
    }

    /// The bundle of `version` for this platform, once its sum, signed by
    /// `key`, matches.
    async fn bundle(&self, key: &PublicKey, version: &Version) -> Result<Vec<u8>, Problem> {
        let sums = self
            .get(
                &layout::in_release(version, layout::SUMS),
                dassie_release::MAX_SUMS,
                SMALL_TIMEOUT_MS,
            )
            .await?;
        let signature = self
            .get(
                &layout::in_release(version, layout::SIGNATURE),
                MAX_SIGNATURE,
                SMALL_TIMEOUT_MS,
            )
            .await?;
        let signature = String::from_utf8(signature)
            .map_err(|_| "SHA256SUMS.minisig is not text".to_string())?;
        let sums: Sums = verify_sums(key, &sums, &signature, version).map_err(|e| e.to_string())?;
        let arch = Arch::current().ok_or("there are no releases for this platform")?;
        let name = layout::bundle(version, arch, Compression::Gz);
        let expected = *sums
            .digest(&name)
            .ok_or_else(|| format!("SHA256SUMS lists no {name}"))?;
        let bundle = self
            .get(
                &layout::in_release(version, &name),
                bundle::MAX_BUNDLE,
                BUNDLE_TIMEOUT_MS,
            )
            .await?;
        if Sha256::digest(&bundle).as_slice() != expected {
            return Err(format!("{name} does not match its signed sum: refusing it"));
        }
        Ok(bundle)
    }
}

/// Unpacks `bundle` into `staging` and checks the binary in it. Returns the
/// unpacked `dassie/`, on disk.
fn stage(bundle: &[u8], staging: &Path, version: &Version) -> Result<PathBuf, Problem> {
    let _ = fs::remove_dir_all(staging);
    let root = staging.join(bundle::ROOT);
    make_dir(staging)?;
    make_dir(&root)?;
    unpack(bundle, &root)?;
    check_binary(&root.join(bundle::BINARY), version)?;
    sync_tree(&root)?;
    Ok(root)
}

/// Creates a directory others may read and enter, whatever the umask: the
/// node runs as a user of its own.
fn make_dir(path: &Path) -> Result<(), Problem> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(error) => return Err(permission(path, &error)),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| permission(path, &e))
}

/// Creates the directories from `root` down to `inner`.
fn make_dirs(root: &Path, inner: &Path) -> Result<(), Problem> {
    let mut path = root.to_path_buf();
    for component in inner.components() {
        path.push(component);
        make_dir(&path)?;
    }
    Ok(())
}

fn unpack(bundle: &[u8], root: &Path) -> Result<(), Problem> {
    let broken = |e: io::Error| format!("the bundle is broken: {e}");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bundle));
    let mut unpacking = Unpacking::new();
    for entry in archive.entries().map_err(broken)? {
        let mut entry = entry.map_err(broken)?;
        let header = entry.header();
        let kind = match header.entry_type() {
            tar::EntryType::Directory => Kind::Directory,
            tar::EntryType::Regular => Kind::File,
            _ => Kind::Other,
        };
        let size = header.size().map_err(broken)?;
        let executable = header.mode().map_err(broken)? & 0o111 != 0;
        let path = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let inner = PathBuf::from(
            unpacking
                .admit(&path, kind, size)
                .map_err(|e| e.to_string())?,
        );
        if kind == Kind::Directory {
            make_dirs(root, &inner)?;
            continue;
        }
        if let Some(parent) = inner.parent() {
            make_dirs(root, parent)?;
        }
        let target = root.join(&inner);
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| permission(&target, &e))?;
        let copied = io::copy(&mut (&mut entry).take(size), &mut file).map_err(broken)?;
        if copied != size {
            return Err("the bundle is broken: a file is cut short".into());
        }
        let mode = if executable { 0o755 } else { 0o644 };
        file.set_permissions(fs::Permissions::from_mode(mode))
            .and_then(|()| file.sync_all())
            .map_err(|e| permission(&target, &e))?;
    }
    Ok(())
}

/// How long the new binary may take to name its version, or to verify its
/// install.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// The new binary must be a file that names `version`, and verify its
/// install.
fn check_binary(binary: &Path, version: &Version) -> Result<(), Problem> {
    let output = run_bounded(binary, "--version")?;
    let named = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || named.trim() != format!("dassie {version}") {
        return Err(format!(
            "the new binary calls itself {:?}, not dassie {version}",
            named.trim()
        ));
    }
    let verified = run_bounded(binary, "verify-install")?;
    if !verified.status.success() {
        return Err(format!(
            "the new binary failed verify-install:\n{}",
            String::from_utf8_lossy(&verified.stderr)
        ));
    }
    Ok(())
}

/// Runs `binary argument`, killing it after [`CHECK_TIMEOUT`].
fn run_bounded(binary: &Path, argument: &str) -> Result<Output, Problem> {
    let does_not_run = |e: io::Error| format!("the new binary does not run: {e}");
    let mut child = Command::new(binary)
        .arg(argument)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(does_not_run)?;
    let started = Instant::now();
    while child.try_wait().map_err(does_not_run)?.is_none() {
        if started.elapsed() > CHECK_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "the new binary's {argument} did not finish in time"
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().map_err(does_not_run)
}

/// Most entries an unpacked tree holds: each one the bundle admits brings
/// at most a directory per level of its path.
const MAX_TREE_ENTRIES: usize = bundle::MAX_ENTRIES * bundle::MAX_DEPTH;

/// Writes the unpacked tree's directories to disk, each after what is in
/// it, so that `current` never names a version a power cut could leave
/// half written.
fn sync_tree(root: &Path) -> Result<(), Problem> {
    // Breadth first, so every directory comes after its parent.
    let mut dirs = vec![root.to_path_buf()];
    let (mut next, mut seen) = (0, 0);
    while let Some(dir) = dirs.get(next).cloned() {
        next += 1;
        for entry in fs::read_dir(&dir).map_err(|e| permission(&dir, &e))? {
            let entry = entry.map_err(|e| permission(&dir, &e))?;
            seen += 1;
            if seen > MAX_TREE_ENTRIES {
                return Err(format!(
                    "{}: more than {MAX_TREE_ENTRIES} entries",
                    root.display()
                ));
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                dirs.push(entry.path());
            }
        }
    }
    dirs.iter().rev().try_for_each(|dir| sync_dir(dir))
}

fn sync_dir(dir: &Path) -> Result<(), Problem> {
    File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| permission(dir, &e))
}

/// Moves the unpacked `root` to `<base>/<version>`, replacing what was
/// there.
fn place(base: &Path, root: &Path, version: &Version) -> Result<PathBuf, Problem> {
    let dir = base.join(version.to_string());
    if dir.exists() {
        let old = base.join(format!(".replaced-{version}-{}", std::process::id()));
        fs::rename(&dir, &old).map_err(|e| permission(&dir, &e))?;
        let _ = fs::remove_dir_all(&old);
    }
    fs::rename(root, &dir).map_err(|e| permission(&dir, &e))?;
    sync_dir(base)?;
    Ok(dir)
}

/// What installing a version's units changed.
#[derive(Debug, Default)]
struct Units {
    /// Whether any unit file changed.
    changed: bool,
    /// The units that were not there before, to enable.
    added: Vec<String>,
}

/// Where systemd units go.
fn unit_dir() -> PathBuf {
    std::env::var_os("DASSIE_SYSTEMD_UNIT_DIR")
        .map_or_else(|| PathBuf::from(UNIT_DIR), PathBuf::from)
}

/// Installs the systemd units a version brings, each only if it changed,
/// noting each in `installed` as it goes, so that a failure part way can be
/// undone. An owner's changes belong in drop-ins, which this leaves alone.
fn install_units(units: &Path, installed: &mut Units) -> Result<(), Problem> {
    let Ok(entries) = fs::read_dir(units) else {
        return Ok(());
    };
    let dir = unit_dir();
    for entry in entries {
        let entry = entry.map_err(|e| permission(units, &e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let source = entry.path();
        let target = dir.join(&name);
        let content = fs::read(&source).map_err(|e| permission(&source, &e))?;
        let before = fs::read(&target).ok();
        if before.as_deref() == Some(content.as_slice()) {
            continue;
        }
        let staged = dir.join(format!(".{name}.dassie-update"));
        let write = || -> io::Result<()> {
            let mut file = File::create(&staged)?;
            io::Write::write_all(&mut file, &content)?;
            file.set_permissions(fs::Permissions::from_mode(0o644))?;
            file.sync_all()
        };
        write().map_err(|e| permission(&staged, &e))?;
        fs::rename(&staged, &target).map_err(|e| permission(&target, &e))?;
        installed.changed = true;
        if before.is_none() {
            installed.added.push(name);
        }
    }
    sync_dir(&dir)
}

/// Puts back the units and `current` of the version that ran, on disk:
/// the units the update added go, and the old version's come back where
/// they changed.
fn put_back(install: &Install, units: &Units) -> Result<(), Problem> {
    let dir = unit_dir();
    for name in &units.added {
        let path = dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(permission(&path, &error)),
        }
    }
    sync_dir(&dir)?;
    let old = install.base.join(install.version.to_string());
    install_units(&old.join("share/systemd"), &mut Units::default())?;
    switch(&install.base, &install.version)
}

/// Points `<base>/current` at `version`, in one `rename`, on disk.
fn switch(base: &Path, version: &Version) -> Result<(), Problem> {
    let staged = base.join(format!(".current-{}", std::process::id()));
    let _ = fs::remove_file(&staged);
    symlink(version.to_string(), &staged).map_err(|e| permission(&staged, &e))?;
    let current = base.join("current");
    fs::rename(&staged, &current).map_err(|e| permission(&current, &e))?;
    sync_dir(base)
}

/// Whether this system runs systemd (`sd_booted`), or a test stands in for
/// systemctl ([`systemctl_program`]).
fn systemd_running() -> bool {
    #[cfg(feature = "test-key")]
    if std::env::var_os("DASSIE_SYSTEMCTL").is_some() {
        return true;
    }
    Path::new("/run/systemd/system").is_dir()
}

/// The systemctl to run. A test build (the `test-key` feature) runs the
/// stand-in `DASSIE_SYSTEMCTL` names, if any, so that tests can restart and
/// roll back; no release has one.
fn systemctl_program() -> std::ffi::OsString {
    #[cfg(feature = "test-key")]
    if let Some(program) = std::env::var_os("DASSIE_SYSTEMCTL") {
        return program;
    }
    "systemctl".into()
}

fn systemctl(args: &[&str]) -> Result<(), Problem> {
    let status = Command::new(systemctl_program())
        .args(args)
        .status()
        .map_err(|e| format!("systemctl: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("systemctl {} failed ({status})", args.join(" ")))
    }
}

/// Removes every version but `keep`, and what an interrupted update or
/// install left.
fn prune(base: &Path, keep: &[&Version]) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let leftover = [".staging-", ".replaced-", ".install-", ".current-"]
            .iter()
            .any(|prefix| name.starts_with(prefix));
        let stale = Version::parse(&name).is_ok_and(|version| !keep.contains(&&version));
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if (leftover || stale) && kind.is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        } else if leftover && kind.is_symlink() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_is_synced_each_directory_after_its_contents() {
        let temp = tempfile::tempdir().unwrap();
        let deep = temp.path().join("dassie/share/systemd/a/b");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("file"), b"x").unwrap();
        assert_eq!(sync_tree(temp.path()), Ok(()));
    }

    // D16: the walk has a bound, which no bundle the updater admits
    // reaches.
    #[test]
    fn a_tree_past_its_bound_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        for n in 0..=MAX_TREE_ENTRIES {
            fs::write(temp.path().join(n.to_string()), b"").unwrap();
        }
        let refused = sync_tree(temp.path()).unwrap_err();
        assert!(refused.contains("more than 8192 entries"), "{refused}");
    }

    #[test]
    fn the_record_reads_back() {
        let record = LastUpdate {
            at_unix_ms: 1_792_108_800_000,
            outcome: "rolled-back".into(),
            target: Some("9.9.9".into()),
            problem: Some("systemctl restart dassie.service failed\n  (exit status: 1)".into()),
        };
        let text = record.to_text();
        assert_eq!(text.lines().count(), 4);
        let read = LastUpdate::parse(&text);
        assert_eq!(
            read.map(|read| read.problem),
            Some(Some(
                "systemctl restart dassie.service failed (exit status: 1)".into()
            ))
        );
        assert_eq!(LastUpdate::parse("outcome=updated\n"), None);
        assert_eq!(LastUpdate::parse("garbage"), None);
    }
}
