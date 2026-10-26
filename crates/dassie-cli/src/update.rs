//! `dassie update`: installs a newer release from the mirror
//! (spec/areas/release.md#dassie-update,
//! spec/decisions/release-integrity.md).
//!
//! The install is `/opt/dassie/<version>`, with `/opt/dassie/current`
//! pointing at the one that runs. The updater finds it from its own path,
//! which must be `<base>/<its version>/bin/dassie`. Then:
//!
//! 1. It asks `meta/<channel>` for the newest version, and stops unless
//!    that supersedes this one (`Version::supersedes`): a mirror cannot take
//!    a node back or to another channel. The owner can, with
//!    `--target-version` or `--channel`. Nor does it try again a version
//!    that did not start here, as `last-update` remembers, until the mirror
//!    offers another; the owner can, with `--force` or `--target-version`.
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
//!    version again; if the database cannot be put back, the node stays
//!    stopped, for the owner to choose which to keep.
//! 6. It keeps the version it came from, and removes older ones.
//!
//! One update runs at a time: each holds a lock on `<base>/.update.lock`,
//! which `install.sh` takes too.
//! Each records how it went in `<base>/last-update`, for `dassie status`.
//!
//! The original's updater verified nothing it downloaded, took any HTTP
//! status as the file, did not trim `meta/latest`, swapped `current` in two
//! steps, and refused every canary install (RT-18).

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use dassie_node::outbound::{OutboundClient, WebRequest};
use dassie_node::store::{
    self, DATABASE_FILE, PRE_MIGRATION_FILE, ROLLED_BACK_FILE, ROLLING_BACK_FILE, Restored,
};
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
    /// This version did not come up, and the one `current` named before,
    /// as it named it, runs again.
    RolledBack(Version, Problem, PathBuf),
    /// The mirror offers this version again, which was rolled back.
    Skipped(Version),
    /// The update stopped, or could not go back: the problem says which.
    Failed(Problem),
}

async fn update(options: Options) -> Result<(), Problem> {
    let install = locate()?;
    let key = release_key()?;
    let _lock = lock(&install.base)?;
    // Without a record that reads, every version is tried, as before one.
    let rolled_back = last_update(&install)
        .and_then(|last| last.rolled_back)
        .and_then(|last| Some((Version::parse(&last.version).ok()?, last.problem)));
    let version = rolled_back.as_ref().map(|(version, _)| version);
    let outcome = match attempt(&options, &install, &key, version).await {
        Ok(outcome) => outcome,
        Err(problem) => Outcome::Failed(problem),
    };
    if let Err(problem) = record(&install, &outcome, rolled_back.as_ref()) {
        eprintln!("Note: could not record how the update went: {problem}");
    }
    match outcome {
        Outcome::UpToDate | Outcome::Updated(_) | Outcome::Skipped(_) => Ok(()),
        Outcome::RolledBack(target, _, before) => Err(format!(
            "dassie {target} did not start, and {} runs again",
            before.display()
        )),
        Outcome::Failed(problem) => Err(problem),
    }
}

async fn attempt(
    options: &Options,
    install: &Install,
    key: &PublicKey,
    rolled_back: Option<&Version>,
) -> Result<Outcome, Problem> {
    let mirror = Mirror::new(options.mirror.as_deref())?;
    let Some(target) = choose(options, install, &mirror).await? else {
        return Ok(Outcome::UpToDate);
    };
    // What `current` names, to go back to should the update not go
    // through: not always the version running this, which may be an older
    // one kept. Where it names nothing, this one is what the owner runs.
    let before = fs::read_link(install.base.join("current"))
        .unwrap_or_else(|_| PathBuf::from(install.version.to_string()));
    // Tried again, a version that did not start would take the node down
    // at every run of the timer, until a fix is out.
    if rolled_back == Some(&target) && options.target.is_none() && !options.force {
        println!(
            "The mirror offers {target}, which did not start here, so {} runs on; \
             waiting for a newer version. `dassie update --force` tries {target} again.",
            before.display()
        );
        return Ok(Outcome::Skipped(target));
    }
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
        return Err(match put_back(&install.base, &units, &before) {
            Ok(()) => problem,
            Err(failed) => format!("{problem}, and putting back what was there failed: {failed}"),
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
            before.display()
        );
        if let Err(failed) = roll_back(&install.base, &units, &before, since) {
            return Err(format!(
                "dassie {target} did not start ({problem}), and going back to {} failed: {failed}",
                before.display()
            ));
        }
        return Ok(Outcome::RolledBack(target, problem, before));
    }
    // The version `current` named stays, for the owner to go back to, and
    // so does the one running this, which the owner kept.
    let came_from = before
        .file_name()
        .and_then(|name| Version::parse(&name.to_string_lossy()).ok());
    let mut keep = vec![&target, &install.version];
    keep.extend(came_from.as_ref());
    prune(&install.base, &keep);
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

/// Points `current` back at `before`, what it named before the update, with
/// that version's units, puts back the database from before the update if
/// the new version migrated it, and starts that version again. The units
/// the update added are disabled and go.
fn roll_back(base: &Path, units: &Units, before: &Path, since: SystemTime) -> Result<(), Problem> {
    // A unit that will not be disabled must not keep the old version down:
    // its file goes all the same.
    for unit in &units.added {
        if let Err(problem) = systemctl(&["disable", "--now", unit]) {
            eprintln!("Note: {problem}; removing {unit} all the same.");
        }
    }
    put_back(base, units, before)?;
    systemctl(&["daemon-reload"])?;
    // systemd may be restarting the failed node; the database must be left
    // alone while it is put back.
    systemctl(&["stop", SERVICE])?;
    // Started on a database that could not be put back, the old version
    // would refuse it, or find none; the owner chooses which to keep.
    restore_database(since)?;
    systemctl(&["start", SERVICE])
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
        Err(error) => Err(format!(
            "putting back the database did not finish ({error}), so {SERVICE} stays stopped; \
             the database from before the update may be back already. In {}, \
             {DATABASE_FILE} is the database, if it is there, {ROLLING_BACK_FILE} or \
             {ROLLED_BACK_FILE} the one the new version left, if it was moved aside, and \
             {PRE_MIGRATION_FILE} the copy from before the update, if it is not back yet. \
             Each database's SQLite side files (-wal, -shm, after its name) belong with it \
             and move with it. Put the one to keep in place as {DATABASE_FILE}, then run \
             `systemctl start {SERVICE}`",
            data_dir.display()
        )),
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
    /// The last rollback, which updates pass over until the mirror offers
    /// another version; kept by every record until an update installs one.
    pub rolled_back: Option<RolledBack>,
}

/// A version that did not start, and why.
#[derive(Debug, PartialEq, Eq)]
pub struct RolledBack {
    /// The version that did not start.
    pub version: String,
    /// Why, as the rollback recorded it, if it did.
    pub problem: Option<String>,
}

/// `problem` on one line, whatever it said.
fn one_line(problem: &str) -> String {
    problem.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl LastUpdate {
    /// The record, as lines of `key=value`.
    fn to_text(&self) -> String {
        let mut text = format!("at={}\noutcome={}\n", self.at_unix_ms, self.outcome);
        if let Some(target) = &self.target {
            text.push_str(&format!("target={target}\n"));
        }
        if let Some(problem) = &self.problem {
            text.push_str(&format!("problem={}\n", one_line(problem)));
        }
        if let Some(RolledBack { version, problem }) = &self.rolled_back {
            text.push_str(&format!("rolled-back={version}\n"));
            if let Some(problem) = problem {
                text.push_str(&format!("rolled-back-problem={}\n", one_line(problem)));
            }
        }
        text
    }

    /// Reads a record; `None` if it is not one.
    fn parse(text: &str) -> Option<Self> {
        let (mut at, mut outcome, mut target, mut problem) = (None, None, None, None);
        let (mut rolled_back, mut rolled_back_problem) = (None, None);
        for line in text.lines() {
            match line.split_once('=')? {
                ("at", value) => at = value.parse().ok(),
                ("outcome", value) => outcome = Some(value.to_owned()),
                ("target", value) => target = Some(value.to_owned()),
                ("problem", value) => problem = Some(value.to_owned()),
                ("rolled-back", value) => rolled_back = Some(value.to_owned()),
                ("rolled-back-problem", value) => rolled_back_problem = Some(value.to_owned()),
                _ => {}
            }
        }
        let outcome = outcome?;
        let rolled_back = match rolled_back {
            Some(version) => Some(RolledBack {
                version,
                problem: rolled_back_problem,
            }),
            // A rollback an updater recorded before it kept it apart.
            None if outcome == "rolled-back" => target.clone().map(|version| RolledBack {
                version,
                problem: problem.clone(),
            }),
            None => None,
        };
        Some(Self {
            at_unix_ms: at?,
            outcome,
            target,
            problem,
            rolled_back,
        })
    }
}

/// Writes how the update went to `<base>/last-update`, readable by all,
/// with the last rollback and why it was, `rolled_back` unless this update
/// rolled back or installed a version.
fn record(
    install: &Install,
    outcome: &Outcome,
    rolled_back: Option<&(Version, Option<Problem>)>,
) -> Result<(), Problem> {
    let at_unix_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let rolled_back = match outcome {
        Outcome::Updated(_) => None,
        Outcome::RolledBack(target, problem, _) => Some(RolledBack {
            version: target.to_string(),
            problem: Some(problem.clone()),
        }),
        Outcome::UpToDate | Outcome::Skipped(_) | Outcome::Failed(_) => {
            rolled_back.map(|(version, problem)| RolledBack {
                version: version.to_string(),
                problem: problem.clone(),
            })
        }
    };
    let (outcome, target, problem) = match outcome {
        Outcome::UpToDate => ("up-to-date", None, None),
        Outcome::Updated(target) => ("updated", Some(target), None),
        Outcome::RolledBack(target, problem, _) => {
            ("rolled-back", Some(target), Some(problem.clone()))
        }
        Outcome::Skipped(target) => ("skipped", Some(target), None),
        Outcome::Failed(problem) => ("failed", None, Some(problem.clone())),
    };
    let text = LastUpdate {
        at_unix_ms,
        outcome: outcome.into(),
        target: target.map(ToString::to_string),
        problem,
        rolled_back,
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

/// Holds `<base>/.update.lock` for as long as it lives. `install.sh` takes
/// the same lock with flock(1), so it must stay a flock(2) lock, which
/// `File::try_lock` is on Linux.
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
    client: OutboundClient,
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
        let client = OutboundClient::new(roots.as_deref(), true)?;
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

/// Puts back, on disk, `current` naming `before`, as it did, with that
/// version's units: the units the update added go, and that version's come
/// back where they changed.
fn put_back(base: &Path, units: &Units, before: &Path) -> Result<(), Problem> {
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
    install_units(
        &base.join(before).join("share/systemd"),
        &mut Units::default(),
    )?;
    point_current(base, before)
}

/// Points `<base>/current` at `version`, in one `rename`, on disk.
fn switch(base: &Path, version: &Version) -> Result<(), Problem> {
    point_current(base, Path::new(&version.to_string()))
}

/// Points `<base>/current` at `target`, in one `rename`, on disk.
fn point_current(base: &Path, target: &Path) -> Result<(), Problem> {
    let staged = base.join(format!(".current-{}", std::process::id()));
    let _ = fs::remove_file(&staged);
    symlink(target, &staged).map_err(|e| permission(&staged, &e))?;
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

    // spec/decisions/tigerstyle.md: the walk has a bound, which no bundle the
    // updater admits reaches.
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
            rolled_back: Some(RolledBack {
                version: "9.9.9".into(),
                problem: Some("systemctl restart\n failed".into()),
            }),
        };
        let text = record.to_text();
        assert_eq!(text.lines().count(), 6);
        let read = LastUpdate::parse(&text).unwrap();
        assert_eq!(
            read.problem.as_deref(),
            Some("systemctl restart dassie.service failed (exit status: 1)")
        );
        assert_eq!(
            read.rolled_back,
            Some(RolledBack {
                version: "9.9.9".into(),
                problem: Some("systemctl restart failed".into()),
            })
        );
        assert_eq!(LastUpdate::parse("outcome=updated\n"), None);
        assert_eq!(LastUpdate::parse("garbage"), None);
    }

    // A rollback recorded before records kept it apart names the version
    // as its target, and why as its problem.
    #[test]
    fn an_earlier_rollback_record_names_the_version_rolled_back() {
        let read = LastUpdate::parse("at=1\noutcome=rolled-back\ntarget=9.9.9\nproblem=x\n");
        assert_eq!(
            read.and_then(|read| read.rolled_back),
            Some(RolledBack {
                version: "9.9.9".into(),
                problem: Some("x".into()),
            })
        );
        let read = LastUpdate::parse("at=1\noutcome=updated\ntarget=9.9.9\n");
        assert_eq!(read.map(|read| read.rolled_back), Some(None));
    }
}
