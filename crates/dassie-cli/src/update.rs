//! `dassie update`: installs a newer release from the mirror (spec 2 §12.4,
//! D11).
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
//!    at it with one `rename`, and restarts the node.
//! 5. It keeps the version it came from, and removes older ones.
//!
//! One update runs at a time: each holds a lock on `<base>/.update.lock`.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::time::{Duration, Instant};

use dassie_node::peer_client::{PeerClient, WebRequest};
use dassie_release::bundle::{self, Kind, Unpacking};
use dassie_release::layout::{self, Arch, Compression};
use dassie_release::{Channel, PublicKey, Sums, Version, verify_sums};
use sha2::{Digest as _, Sha256};

include!(concat!(env!("OUT_DIR"), "/release_key.rs"));

/// Where systemd units go, unless `DASSIE_SYSTEMD_UNIT_DIR` says otherwise.
const UNIT_DIR: &str = "/etc/systemd/system";
/// The node's service, restarted once `current` moved.
const SERVICE: &str = "dassie.service";
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
struct Install {
    /// `/opt/dassie`.
    base: PathBuf,
    /// The version running.
    version: Version,
}

async fn update(options: Options) -> Result<(), Problem> {
    let install = locate()?;
    let key = release_key()?;
    let _lock = lock(&install.base)?;
    let mirror = Mirror::new(options.mirror.as_deref())?;
    let Some(target) = choose(&options, &install, &mirror).await? else {
        return Ok(());
    };
    println!("Downloading dassie {target} from {}...", mirror.url);
    let bundle = mirror.bundle(&key, &target).await?;
    let staging = install
        .base
        .join(format!(".staging-{target}-{}", std::process::id()));
    let installed =
        stage(&bundle, &staging, &target).and_then(|root| place(&install.base, &root, &target));
    // The staging directory goes either way; what it held is in place.
    let _ = fs::remove_dir_all(&staging);
    let dir = installed?;
    let units = install_units(&dir.join("share/systemd"))?;
    switch(&install.base, &target)?;
    println!("Installed dassie {target}.");
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
        roll_back(&install)?;
        return Err(format!(
            "dassie {target} did not start, and {} runs again",
            install.version
        ));
    }
    prune(&install.base, &[&target, &install.version]);
    Ok(())
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

/// Points `current` back at the version that ran, with its units, and
/// restarts it.
fn roll_back(install: &Install) -> Result<(), Problem> {
    let dir = install.base.join(install.version.to_string());
    install_units(&dir.join("share/systemd"))?;
    switch(&install.base, &install.version)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["restart", SERVICE])
}

/// Finds the install from this binary's path.
fn locate() -> Result<Install, Problem> {
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
        Version::parse(text.trim()).map_err(|e| format!("meta/{}: {e}", channel.name()))
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

/// Writes the unpacked tree's directories to disk, so that `current`
/// never names a version a power cut could leave half written.
fn sync_tree(dir: &Path) -> Result<(), Problem> {
    for entry in fs::read_dir(dir).map_err(|e| permission(dir, &e))? {
        let entry = entry.map_err(|e| permission(dir, &e))?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            sync_tree(&entry.path())?;
        }
    }
    sync_dir(dir)
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

/// Installs the systemd units a version brings, each only if it changed.
/// An owner's changes belong in drop-ins, which this leaves alone.
fn install_units(units: &Path) -> Result<Units, Problem> {
    let Ok(entries) = fs::read_dir(units) else {
        return Ok(Units::default());
    };
    let dir = std::env::var_os("DASSIE_SYSTEMD_UNIT_DIR")
        .map_or_else(|| PathBuf::from(UNIT_DIR), PathBuf::from);
    let mut installed = Units::default();
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
    sync_dir(&dir)?;
    Ok(installed)
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

/// Whether this system runs systemd (`sd_booted`).
fn systemd_running() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

fn systemctl(args: &[&str]) -> Result<(), Problem> {
    let status = Command::new("systemctl")
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
