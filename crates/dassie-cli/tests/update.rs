//! `dassie update` as a process, against a mirror served here over TLS.
//!
//! The binary is built with the `test-key` feature, so it trusts the test
//! key of testdata/release, with which these tests sign. It runs from an
//! install laid out as the installer lays it out, in a temporary directory:
//! `<base>/<version>/bin/dassie` and `<base>/current`. The releases it is
//! offered hold a stand-in binary, a script that names its version and
//! passes `verify-install`. systemd is left alone (`--no-restart`), or a
//! stand-in takes systemctl's place (`DASSIE_SYSTEMCTL`), and the units go
//! to a directory of the test's.

#![cfg(feature = "test-key")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{FileExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_release::layout::{self, Arch};
use dassie_release::testing::{TEST_SECRET_KEY, sign};
use flate2::write::GzEncoder;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const DASSIE: &str = env!("CARGO_BIN_EXE_dassie");
const TLS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls");

type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// How often a test tries what another test's child can briefly hold up,
/// and how long it waits in between.
const TRIES: u32 = 50;
const BETWEEN_TRIES: Duration = Duration::from_millis(20);

/// A mirror on localhost, serving whatever `files` holds.
struct MirrorServer {
    url: String,
    files: Files,
}

impl MirrorServer {
    fn start() -> Self {
        let files: Files = Arc::default();
        let served = Arc::clone(&files);
        let (port_tx, port_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(serve(served, port_tx));
        });
        let port: u16 = port_rx.recv().unwrap();
        Self {
            url: format!("https://localhost:{port}"),
            files,
        }
    }

    fn put(&self, path: &str, bytes: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_owned(), bytes.into());
    }
}

async fn serve(files: Files, port: std::sync::mpsc::Sender<u16>) {
    let certs = CertificateDer::pem_file_iter(format!("{TLS_DIR}/cert.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(format!("{TLS_DIR}/key.pem")).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    port.send(listener.local_addr().unwrap().port()).unwrap();
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let acceptor = acceptor.clone();
        let files = Arc::clone(&files);
        tokio::spawn(async move {
            let Ok(mut stream) = acceptor.accept(stream).await else {
                return;
            };
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buffer).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => request.extend_from_slice(&buffer[..n]),
                }
            }
            let line = String::from_utf8_lossy(&request);
            let path = line
                .split(' ')
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/')
                .to_owned();
            let body = files.lock().unwrap().get(&path).cloned();
            let (status, body) = match body {
                Some(body) => ("200 OK", body),
                None => ("404 Not Found", Vec::new()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            let _ = stream.shutdown().await;
        });
    }
}

/// An install as the installer leaves it, running this build of dassie.
struct Install {
    dir: TempDir,
    version: String,
}

impl Install {
    fn new() -> Self {
        // Beside the binary, which is linked rather than copied.
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let output = Command::new(DASSIE).arg("--version").output().unwrap();
        let version = String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .strip_prefix("dassie ")
            .unwrap()
            .to_owned();
        let bin = dir.path().join("opt").join(&version).join("bin");
        fs::create_dir_all(&bin).unwrap();
        if fs::hard_link(DASSIE, bin.join("dassie")).is_err() {
            fs::copy(DASSIE, bin.join("dassie")).unwrap();
        }
        symlink(&version, dir.path().join("opt/current")).unwrap();
        fs::create_dir(dir.path().join("units")).unwrap();
        Self { dir, version }
    }

    fn base(&self) -> PathBuf {
        self.dir.path().join("opt")
    }

    fn update(&self, mirror: &MirrorServer, args: &[&str]) -> Output {
        self.update_with_umask("022", mirror, args)
    }

    /// Runs the updater under `umask`, as an owner's shell might.
    fn update_with_umask(&self, umask: &str, mirror: &MirrorServer, args: &[&str]) -> Output {
        Command::new("sh")
            .args(["-c", &format!("umask {umask} && exec \"$0\" \"$@\"")])
            .arg(self.base().join("current/bin/dassie"))
            .arg("update")
            .args(["--mirror", &mirror.url, "--no-restart"])
            .args(args)
            .env("DASSIE_MIRROR_CA_FILE", format!("{TLS_DIR}/root.pem"))
            .env("DASSIE_SYSTEMD_UNIT_DIR", self.dir.path().join("units"))
            .env_remove("DASSIE_MIRROR_URL")
            .output()
            .unwrap()
    }

    /// The updater at `binary`, with what every run here sets.
    fn updater(&self, binary: &Path, mirror: &MirrorServer) -> Command {
        let mut command = Command::new(binary);
        command
            .arg("update")
            .args(["--mirror", &mirror.url])
            .env("DASSIE_MIRROR_CA_FILE", format!("{TLS_DIR}/root.pem"))
            .env("DASSIE_SYSTEMD_UNIT_DIR", self.dir.path().join("units"))
            .env_remove("DASSIE_MIRROR_URL");
        command
    }

    /// Runs the updater as the timer does, restarting the node, through
    /// the stand-in `systemctl`; the node's data is in `state/`.
    fn update_with_systemctl(&self, mirror: &MirrorServer, systemctl: &Path) -> Output {
        self.updater(&self.base().join("current/bin/dassie"), mirror)
            .env("DASSIE_SYSTEMCTL", systemctl)
            .env("DASSIE_STATE_DIRECTORY", self.dir.path().join("state"))
            .output()
            .unwrap()
    }

    fn current(&self) -> String {
        fs::read_link(self.base().join("current"))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    /// The names in the base directory, but the lock and the record.
    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.base())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != ".update.lock" && name != "last-update")
            .collect();
        names.sort();
        names
    }

    /// How the last update went, as it recorded it for `dassie status`.
    fn last_update(&self) -> String {
        fs::read_to_string(self.base().join("last-update")).unwrap()
    }

    /// Gives the running version `unit` as its `dassie.service`, installed.
    fn with_unit(&self, unit: &[u8]) {
        let units = self.base().join(&self.version).join("share/systemd");
        fs::create_dir_all(&units).unwrap();
        fs::write(units.join("dassie.service"), unit).unwrap();
        fs::write(self.dir.path().join("units/dassie.service"), unit).unwrap();
    }

    /// The unit `name` as installed, if it is.
    fn unit(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.dir.path().join("units").join(name)).ok()
    }
}

/// What a bundle holds: (path, content, mode); a `None` content is a
/// directory, and a mode of 0 a symbolic link to the content.
type Entry<'a> = (&'a str, Option<&'a [u8]>, u32);

fn stand_in(version: &str) -> String {
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo \"dassie {version}\" ;;\n  verify-install) echo ok ;;\n  *) exit 1 ;;\nesac\n"
    )
}

fn tarball(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), flate2::Compression::default()));
    for (path, content, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(*mode);
        header.set_mtime(0);
        match (content, mode) {
            (None, _) => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder
                    .append_data(&mut header, path, std::io::empty())
                    .unwrap();
            }
            (Some(target), 0) => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder
                    .append_link(&mut header, path, std::str::from_utf8(target).unwrap())
                    .unwrap();
            }
            (Some(bytes), _) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(bytes.len() as u64);
                builder.append_data(&mut header, path, *bytes).unwrap();
            }
        }
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// Publishes `version` with `bundle` for this platform, its sums signed
/// with `comment` (by default the version's), and names it on `latest`.
fn publish(mirror: &MirrorServer, version: &str, bundle: &[u8], comment: Option<&str>) {
    let parsed = dassie_release::Version::parse(version).unwrap();
    let name = layout::bundle(&parsed, Arch::current().unwrap());
    let digest: String = Sha256::digest(bundle)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let sums = format!("{digest}  {name}\n");
    let comment = comment.map_or_else(|| format!("dassie {version}"), str::to_owned);
    mirror.put(&format!("{version}/{name}"), bundle);
    mirror.put(
        &format!("{version}/SHA256SUMS.minisig"),
        sign(TEST_SECRET_KEY, sums.as_bytes(), &comment),
    );
    mirror.put(&format!("{version}/SHA256SUMS"), sums);
    mirror.put("meta/latest", version);
}

const UNIT: &[u8] = b"[Service]\nExecStart=/opt/dassie/current/bin/dassie daemon\n";
/// The unit of the version that runs, before an update.
const OLD_UNIT: &[u8] = b"[Service]\nExecStart=/opt/dassie/current/bin/dassie\n";

fn release(version: &str) -> Vec<u8> {
    let script = stand_in(version);
    tarball(&[
        ("dassie/", None, 0o755),
        ("dassie/bin/", None, 0o755),
        ("dassie/bin/dassie", Some(script.as_bytes()), 0o755),
        ("dassie/share/", None, 0o755),
        ("dassie/share/systemd/", None, 0o755),
        ("dassie/share/systemd/dassie.service", Some(UNIT), 0o644),
    ])
}

/// A stand-in for systemctl in `dir`, which logs each call to
/// `systemctl.log` beside it and fails `restart`, as when the new node does
/// not come up.
fn systemctl_failing_restart(dir: &Path) -> PathBuf {
    let path = dir.join("systemctl");
    fs::write(
        &path,
        "#!/bin/sh\necho \"$*\" >> \"$0.log\"\ncase \"$1\" in\n  restart) exit 1 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    // Once it has run here, no child another thread forked holds it open
    // for writing, and the updater can run it too.
    output_of_written(Command::new(&path).arg("is-system-running"));
    path
}

/// What the stand-in systemctl at `path` was asked, a call a line.
fn systemctl_calls(path: &Path) -> Vec<String> {
    fs::read_to_string(path.with_extension("log"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Runs `command`, which runs a file this process wrote. The tests run on
/// threads of one process, and a child another one forks while the file is
/// open for writing holds it open until the child execs; Linux runs no file
/// open for writing (ETXTBSY). So this tries again, a bounded number of
/// times.
fn output_of_written(command: &mut Command) -> Output {
    for _ in 1..TRIES {
        match command.output() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(BETWEEN_TRIES);
            }
            output => return output.unwrap(),
        }
    }
    command.output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Asserts that the update failed saying `why`, and left the install as
/// it was.
fn refused(install: &Install, output: &Output, why: &str) {
    assert!(!output.status.success(), "{}", text(output));
    assert!(text(output).contains(why), "{}", text(output));
    assert_eq!(install.current(), install.version);
    assert_eq!(
        install.entries(),
        vec![install.version.clone(), "current".to_owned()]
    );
    // Unless another update held the lock, and so recorded its own.
    if why != "another update is running" {
        let record = install.last_update();
        assert!(record.contains("\noutcome=failed\n"), "{record}");
        assert!(record.contains(why), "{record}");
    }
}

#[test]
fn installs_a_newer_release() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let output = install.update(&mirror, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("Installed dassie 9.9.9."),
        "{}",
        text(&output)
    );
    assert_eq!(install.current(), "9.9.9");
    let record = install.last_update();
    assert!(
        record.contains("\noutcome=updated\ntarget=9.9.9\n"),
        "{record}"
    );
    // The version it came from stays, for going back by hand.
    let mut expected = vec![
        "9.9.9".to_owned(),
        "current".into(),
        install.version.clone(),
    ];
    expected.sort();
    assert_eq!(install.entries(), expected);
    let binary = install.base().join("9.9.9/bin/dassie");
    assert_eq!(
        fs::metadata(&binary).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(install.dir.path().join("units/dassie.service")).unwrap(),
        UNIT
    );
    // The new binary is the stand-in: it runs and names its version.
    let named = Command::new(install.base().join("current/bin/dassie"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&named.stdout).trim(),
        "dassie 9.9.9"
    );
}

/// Under a strict umask, as on a hardened host, the install is still one
/// the node's own user can read and run.
#[test]
fn installs_for_others_to_run_whatever_the_umask() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let output = install.update_with_umask("077", &mirror, &[]);
    assert!(output.status.success(), "{}", text(&output));
    let mode = |path: &str| {
        fs::metadata(install.base().join(path))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    for dir in ["9.9.9", "9.9.9/bin", "9.9.9/share", "9.9.9/share/systemd"] {
        assert_eq!(mode(dir), 0o755, "{dir}");
    }
    assert_eq!(mode("9.9.9/bin/dassie"), 0o755);
    assert_eq!(mode("9.9.9/share/systemd/dassie.service"), 0o644);
    let unit = install.dir.path().join("units/dassie.service");
    assert_eq!(
        fs::metadata(unit).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn stays_when_up_to_date_or_offered_older() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    mirror.put("meta/latest", install.version.clone());
    let output = install.update(&mirror, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("Already up to date"),
        "{}",
        text(&output)
    );
    assert!(install.last_update().contains("\noutcome=up-to-date\n"));
    publish(&mirror, "0.0.1", &release("0.0.1"), None);
    let output = install.update(&mirror, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("not newer"), "{}", text(&output));
    assert_eq!(
        install.entries(),
        vec![install.version.clone(), "current".to_owned()]
    );
    // The owner may go back on purpose.
    let output = install.update(&mirror, &["--target-version", "0.0.1"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(install.current(), "0.0.1");
}

/// `meta/latest` names a release, and nothing else the mirror might write
/// there: no other shape names a directory the updater would install.
#[test]
fn refuses_what_is_not_a_release() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    mirror.put("meta/latest", "canary-2026-10-16-120000-abcdef0");
    refused(
        &install,
        &install.update(&mirror, &[]),
        "meta/latest: the version is not of the form 1.2.3",
    );
}

// The updater installs only a bundle whose signature verifies.
#[test]
fn refuses_a_tampered_bundle() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let name = layout::bundle(
        &dassie_release::Version::parse("9.9.9").unwrap(),
        Arch::current().unwrap(),
    );
    let mut tampered = release("9.9.9");
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    mirror.put(&format!("9.9.9/{name}"), tampered);
    refused(
        &install,
        &install.update(&mirror, &[]),
        "does not match its signed sum",
    );
}

#[test]
fn refuses_sums_not_signed_for_the_version() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), Some("dassie 9.9.8"));
    refused(
        &install,
        &install.update(&mirror, &[]),
        "signed for another version",
    );
}

#[test]
fn refuses_a_bad_signature_or_none() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let sums = mirror.files.lock().unwrap()["9.9.9/SHA256SUMS"].clone();
    let mut changed = sums.clone();
    changed[0] = if changed[0] == b'0' { b'1' } else { b'0' };
    mirror.put("9.9.9/SHA256SUMS", changed);
    refused(
        &install,
        &install.update(&mirror, &[]),
        "the signature does not match",
    );
    mirror.put("9.9.9/SHA256SUMS", sums);
    mirror
        .files
        .lock()
        .unwrap()
        .remove("9.9.9/SHA256SUMS.minisig");
    refused(&install, &install.update(&mirror, &[]), "answered 404");
}

#[test]
fn refuses_what_a_bundle_must_not_hold() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    let script = stand_in("9.9.9");
    let linked = tarball(&[
        ("dassie/", None, 0o755),
        ("dassie/bin/", None, 0o755),
        ("dassie/bin/dassie", Some(b"/bin/sh"), 0),
    ]);
    publish(&mirror, "9.9.9", &linked, None);
    refused(
        &install,
        &install.update(&mirror, &[]),
        "not a file or directory",
    );
    let outside = tarball(&[
        ("dassie/bin/dassie", Some(script.as_bytes()), 0o755),
        ("etc/cron.d/x", Some(b"x"), 0o644),
    ]);
    publish(&mirror, "9.9.9", &outside, None);
    refused(&install, &install.update(&mirror, &[]), "outside dassie/");
    let misnamed = release("9.9.8");
    publish(&mirror, "9.9.9", &misnamed, None);
    refused(
        &install,
        &install.update(&mirror, &[]),
        "calls itself \"dassie 9.9.8\"",
    );
}

#[test]
fn runs_one_update_at_a_time() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let lock = fs::File::create(install.base().join(".update.lock")).unwrap();
    lock.lock().unwrap();
    refused(
        &install,
        &install.update(&mirror, &[]),
        "another update is running",
    );
    drop(lock);
    // A child another test's thread forked while `lock` was open shares its
    // lock until the child execs, so the lock can outlive `drop` a little.
    let mut output = install.update(&mirror, &[]);
    for _ in 1..TRIES {
        if !text(&output).contains("another update is running") {
            break;
        }
        std::thread::sleep(BETWEEN_TRIES);
        output = install.update(&mirror, &[]);
    }
    assert!(output.status.success(), "{}", text(&output));
}

/// If the database cannot be put back, a rollback does not start the old
/// version, which would refuse it, or find none and start afresh; it says
/// where the database and its copies are.
#[test]
fn a_rollback_starts_no_node_without_a_database_it_can_run() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    install.with_unit(OLD_UNIT);
    // A database the new version migrated past this one, as the user
    // version at offset 60 of its header says, with no copy from before.
    let state = install.dir.path().join("state");
    drop(dassie_node::store::Store::open(&state).unwrap());
    fs::OpenOptions::new()
        .write(true)
        .open(state.join("dassie.sqlite3"))
        .unwrap()
        .write_all_at(&999_u32.to_be_bytes(), 60)
        .unwrap();
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let systemctl = systemctl_failing_restart(install.dir.path());
    let output = install.update_with_systemctl(&mirror, &systemctl);
    assert!(!output.status.success(), "{}", text(&output));
    for said in [
        "dassie.service stays stopped",
        "dassie.sqlite3.rolling-back",
        "dassie.sqlite3.rolled-back",
        "-wal, -shm",
        "dassie.sqlite3.pre-migration",
    ] {
        assert!(text(&output).contains(said), "{}", text(&output));
    }
    assert_eq!(install.current(), install.version);
    let record = install.last_update();
    assert!(record.contains("\noutcome=failed\n"), "{record}");
    assert!(record.contains("stays stopped"), "{record}");
    let calls = systemctl_calls(&systemctl);
    assert_eq!(
        calls.last().map(String::as_str),
        Some("stop dassie.service")
    );
}

/// When the new node does not come up, the version before runs again, with
/// its units.
#[test]
fn goes_back_when_the_new_node_does_not_start() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    install.with_unit(OLD_UNIT);
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let systemctl = systemctl_failing_restart(install.dir.path());
    let output = install.update_with_systemctl(&mirror, &systemctl);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains(&format!(
            "dassie 9.9.9 did not start, and {} runs again",
            install.version
        )),
        "{}",
        text(&output)
    );
    assert_eq!(install.current(), install.version);
    assert_eq!(install.unit("dassie.service").as_deref(), Some(OLD_UNIT));
    let record = install.last_update();
    assert!(
        record.contains("\noutcome=rolled-back\ntarget=9.9.9\n"),
        "{record}"
    );
    assert_eq!(
        systemctl_calls(&systemctl)[1..],
        [
            "daemon-reload",
            "restart dassie.service",
            "daemon-reload",
            "stop dassie.service",
            "start dassie.service",
        ]
    );
}

/// Rolls back `9.9.9`, which the mirror offers, as the new node does not
/// start; returns the stand-in systemctl, which logged it.
fn rolled_back_9_9_9(mirror: &MirrorServer, install: &Install) -> PathBuf {
    install.with_unit(OLD_UNIT);
    publish(mirror, "9.9.9", &release("9.9.9"), None);
    let systemctl = systemctl_failing_restart(install.dir.path());
    let output = install.update_with_systemctl(mirror, &systemctl);
    assert!(!output.status.success(), "{}", text(&output));
    assert_eq!(install.current(), install.version);
    systemctl
}

/// The timer does not try again, every night, a version that did not start
/// here, which would take the node down each time: it waits for another,
/// even across runs that fail.
#[test]
fn passes_over_a_rolled_back_version_until_another_is_offered() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    let systemctl = rolled_back_9_9_9(&mirror, &install);
    let calls = systemctl_calls(&systemctl).len();
    for _ in 0..2 {
        let output = install.update_with_systemctl(&mirror, &systemctl);
        assert!(output.status.success(), "{}", text(&output));
        assert!(
            text(&output).contains("The mirror offers 9.9.9, which did not start here"),
            "{}",
            text(&output)
        );
        assert!(text(&output).contains("--force"), "{}", text(&output));
        assert_eq!(install.current(), install.version);
        assert_eq!(systemctl_calls(&systemctl).len(), calls);
        let record = install.last_update();
        assert!(
            record.contains("\noutcome=skipped\ntarget=9.9.9\nrolled-back=9.9.9\n"),
            "{record}"
        );
        // Why 9.9.9 did not start stays, for `dassie status`.
        assert!(
            record.contains("\nrolled-back-problem=systemctl restart dassie.service failed"),
            "{record}"
        );
    }
    // A run that fails does not forget it.
    mirror.put("meta/latest", "not a version");
    let output = install.update_with_systemctl(&mirror, &systemctl);
    assert!(!output.status.success(), "{}", text(&output));
    let record = install.last_update();
    assert!(record.contains("\noutcome=failed\n"), "{record}");
    assert!(record.contains("\nrolled-back=9.9.9\n"), "{record}");
    assert!(
        record.contains("\nrolled-back-problem=systemctl restart dassie.service failed"),
        "{record}"
    );
    mirror.put("meta/latest", "9.9.9");
    let output = install.update_with_systemctl(&mirror, &systemctl);
    assert!(
        text(&output).contains("did not start here"),
        "{}",
        text(&output)
    );
    // A newer version is installed, and the record forgets 9.9.9.
    publish(&mirror, "9.9.10", &release("9.9.10"), None);
    let output = install.update(&mirror, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(install.current(), "9.9.10");
    let record = install.last_update();
    assert!(
        record.contains("\noutcome=updated\ntarget=9.9.10\n"),
        "{record}"
    );
    assert!(!record.contains("rolled-back"), "{record}");
}

/// The owner can try a rolled-back version again, by forcing it or by
/// naming it.
#[test]
fn the_owner_can_install_a_rolled_back_version() {
    for args in [&["--force"][..], &["--target-version", "9.9.9"]] {
        let (mirror, install) = (MirrorServer::start(), Install::new());
        rolled_back_9_9_9(&mirror, &install);
        let output = install.update(&mirror, args);
        assert!(output.status.success(), "{args:?}: {}", text(&output));
        assert_eq!(install.current(), "9.9.9", "{args:?}");
    }
}

/// Without a record that reads, the updater tries what the mirror offers,
/// as it did before it kept one.
#[test]
fn tries_a_rolled_back_version_again_without_a_record_that_reads() {
    for record in [
        "garbage",
        "at=1\noutcome=rolled-back\nrolled-back=not a version\n",
    ] {
        let (mirror, install) = (MirrorServer::start(), Install::new());
        let systemctl = rolled_back_9_9_9(&mirror, &install);
        fs::write(install.base().join("last-update"), record).unwrap();
        let output = install.update_with_systemctl(&mirror, &systemctl);
        assert!(
            text(&output).contains("dassie 9.9.9 did not start, and"),
            "{record}: {}",
            text(&output)
        );
    }
    let (mirror, install) = (MirrorServer::start(), Install::new());
    rolled_back_9_9_9(&mirror, &install);
    fs::remove_file(install.base().join("last-update")).unwrap();
    let output = install.update(&mirror, &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(install.current(), "9.9.9");
}

/// A release that brings a unit the running version does not have.
fn release_adding_a_unit(version: &str) -> Vec<u8> {
    let script = stand_in(version);
    tarball(&[
        ("dassie/", None, 0o755),
        ("dassie/bin/", None, 0o755),
        ("dassie/bin/dassie", Some(script.as_bytes()), 0o755),
        ("dassie/share/", None, 0o755),
        ("dassie/share/systemd/", None, 0o755),
        ("dassie/share/systemd/dassie.service", Some(UNIT), 0o644),
        (
            "dassie/share/systemd/dassie-new.timer",
            Some(b"[Timer]\n"),
            0o644,
        ),
    ])
}

/// If `current` cannot be switched, the version that runs keeps its units,
/// and has none the new version brought.
#[test]
fn puts_back_the_units_when_the_switch_fails() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    install.with_unit(OLD_UNIT);
    publish(&mirror, "9.9.9", &release_adding_a_unit("9.9.9"), None);
    // A directory where `current` is: no rename replaces it.
    fs::remove_file(install.base().join("current")).unwrap();
    fs::create_dir_all(install.base().join("current/in-the-way")).unwrap();
    let binary = install.base().join(&install.version).join("bin/dassie");
    let output = install
        .updater(&binary, &mirror)
        .arg("--no-restart")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("current"), "{}", text(&output));
    assert_eq!(install.unit("dassie.service").as_deref(), Some(OLD_UNIT));
    assert_eq!(install.unit("dassie-new.timer"), None);
    assert!(install.last_update().contains("\noutcome=failed\n"));
}

/// If a unit cannot be installed, `current` goes back to what it named,
/// with that version's units, even where the updater that ran is another
/// version the owner kept.
#[test]
fn puts_back_what_current_named_when_a_unit_cannot_be_installed() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    // The binary that runs is a kept version; `current` names a later one.
    install.with_unit(b"[Service]\n# the kept version's\n");
    let later = install.base().join("9.0.0");
    fs::create_dir_all(later.join("bin")).unwrap();
    fs::create_dir_all(later.join("share/systemd")).unwrap();
    fs::write(later.join("share/systemd/dassie.service"), OLD_UNIT).unwrap();
    fs::write(install.dir.path().join("units/dassie.service"), OLD_UNIT).unwrap();
    fs::remove_file(install.base().join("current")).unwrap();
    symlink("9.0.0", install.base().join("current")).unwrap();
    // A directory where one of the new units goes: no rename replaces it.
    fs::create_dir_all(install.dir.path().join("units/dassie-blocked.socket/x")).unwrap();
    let script = stand_in("9.9.9");
    let bundle = tarball(&[
        ("dassie/", None, 0o755),
        ("dassie/bin/", None, 0o755),
        ("dassie/bin/dassie", Some(script.as_bytes()), 0o755),
        ("dassie/share/", None, 0o755),
        ("dassie/share/systemd/", None, 0o755),
        ("dassie/share/systemd/dassie.service", Some(UNIT), 0o644),
        (
            "dassie/share/systemd/dassie-new.timer",
            Some(b"[Timer]\n"),
            0o644,
        ),
        (
            "dassie/share/systemd/dassie-blocked.socket",
            Some(b"[Socket]\n"),
            0o644,
        ),
    ]);
    publish(&mirror, "9.9.9", &bundle, None);
    let binary = install.base().join(&install.version).join("bin/dassie");
    let output = install
        .updater(&binary, &mirror)
        .arg("--no-restart")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("dassie-blocked.socket"),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("putting back"), "{}", text(&output));
    assert_eq!(install.current(), "9.0.0");
    assert_eq!(install.unit("dassie.service").as_deref(), Some(OLD_UNIT));
    assert_eq!(install.unit("dassie-new.timer"), None);
    assert!(install.last_update().contains("\noutcome=failed\n"));
}

/// A rollback takes out the units the new version added, disabled, as well
/// as putting back the old version's.
#[test]
fn a_rollback_takes_out_the_units_the_new_version_added() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    install.with_unit(OLD_UNIT);
    publish(&mirror, "9.9.9", &release_adding_a_unit("9.9.9"), None);
    let systemctl = systemctl_failing_restart(install.dir.path());
    let output = install.update_with_systemctl(&mirror, &systemctl);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("runs again"), "{}", text(&output));
    assert_eq!(install.current(), install.version);
    assert_eq!(install.unit("dassie.service").as_deref(), Some(OLD_UNIT));
    assert_eq!(install.unit("dassie-new.timer"), None);
    assert_eq!(
        systemctl_calls(&systemctl)[1..],
        [
            "daemon-reload",
            "enable dassie-new.timer",
            "restart dassie.service",
            "disable --now dassie-new.timer",
            "daemon-reload",
            "stop dassie.service",
            "start dassie.service",
        ]
    );
}

/// When the new node does not start, `current` goes back to what it named,
/// with that version's units, even where the updater that ran is another
/// version the owner kept, as it does when a unit cannot be installed.
#[test]
fn a_rollback_puts_back_what_current_named() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    // The binary that runs is a kept version; `current` names a later one.
    install.with_unit(b"[Service]\n# the kept version's\n");
    let later = install.base().join("9.0.0");
    fs::create_dir_all(later.join("bin")).unwrap();
    fs::create_dir_all(later.join("share/systemd")).unwrap();
    fs::write(later.join("share/systemd/dassie.service"), OLD_UNIT).unwrap();
    fs::write(install.dir.path().join("units/dassie.service"), OLD_UNIT).unwrap();
    fs::remove_file(install.base().join("current")).unwrap();
    symlink("9.0.0", install.base().join("current")).unwrap();
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let systemctl = systemctl_failing_restart(install.dir.path());
    let binary = install.base().join(&install.version).join("bin/dassie");
    let output = install
        .updater(&binary, &mirror)
        .env("DASSIE_SYSTEMCTL", &systemctl)
        .env("DASSIE_STATE_DIRECTORY", install.dir.path().join("state"))
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output));
    let record = install.last_update();
    assert!(record.contains("\noutcome=rolled-back\n"), "{record}");
    assert_eq!(install.current(), "9.0.0", "{}", text(&output));
    assert_eq!(install.unit("dassie.service").as_deref(), Some(OLD_UNIT));
    assert!(
        text(&output).contains("dassie 9.9.9 did not start, and 9.0.0 runs again"),
        "{}",
        text(&output)
    );
}

/// A stand-in for systemctl in `dir` that does what it is asked, and logs
/// each call as [`systemctl_failing_restart`]'s does.
fn systemctl_logging(dir: &Path) -> PathBuf {
    let path = dir.join("systemctl");
    fs::write(&path, "#!/bin/sh\necho \"$*\" >> \"$0.log\"\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    output_of_written(Command::new(&path).arg("is-system-running"));
    path
}

/// `--unit` for two nodes, `a@x` and `a@y`, as the market maker's VM names
/// its own.
fn two_units(install: &Install) -> Vec<String> {
    let state = install.dir.path().join("state");
    ["x", "y"]
        .iter()
        .flat_map(|node| {
            [
                "--unit".to_owned(),
                format!("a@{node}.service={}", state.join(node).display()),
            ]
        })
        .collect()
}

/// Named units, as the market maker's VM names its two nodes: the updater
/// restarts them rather than dassie.service, and installs none of the
/// release's units, which whoever names them does not run.
#[test]
fn restarts_the_units_it_is_named() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release_adding_a_unit("9.9.9"), None);
    let systemctl = systemctl_logging(install.dir.path());
    let output = install
        .updater(&install.base().join("current/bin/dassie"), &mirror)
        .env("DASSIE_SYSTEMCTL", &systemctl)
        .args(two_units(&install))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(install.current(), "9.9.9");
    assert_eq!(install.unit("dassie.service"), None);
    assert_eq!(install.unit("dassie-new.timer"), None);
    assert_eq!(
        systemctl_calls(&systemctl)[1..],
        ["restart a@x.service a@y.service"]
    );
    assert!(
        install
            .last_update()
            .contains("\noutcome=updated\ntarget=9.9.9\n")
    );
}

/// If a named unit does not come up, the updater goes back as it does for
/// dassie.service: `current` back, the units stopped, each node's database
/// put back if it must, and started again; the record says so. It puts
/// back none of the version's own units, which it never installed.
#[test]
fn goes_back_on_the_units_it_is_named() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let units = install.base().join(&install.version).join("share/systemd");
    fs::create_dir_all(&units).unwrap();
    fs::write(units.join("dassie.service"), UNIT).unwrap();
    let systemctl = systemctl_failing_restart(install.dir.path());
    let output = install
        .updater(&install.base().join("current/bin/dassie"), &mirror)
        .env("DASSIE_SYSTEMCTL", &systemctl)
        .args(two_units(&install))
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", text(&output));
    assert_eq!(install.current(), install.version);
    let record = install.last_update();
    assert!(
        record.contains("\noutcome=rolled-back\ntarget=9.9.9\n"),
        "{record}"
    );
    assert_eq!(install.unit("dassie.service"), None);
    assert_eq!(
        systemctl_calls(&systemctl)[1..],
        [
            "restart a@x.service a@y.service",
            "stop a@x.service a@y.service",
            "start a@x.service a@y.service",
        ]
    );
}

/// `--unit` names a service and an absolute data directory, each unit
/// once, and restarts what `--no-restart` would leave alone.
#[test]
fn refuses_a_unit_it_cannot_take() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    for args in [
        &["--unit", "a.service"][..],
        &["--unit", "a.timer=/var/lib/a"],
        &["--unit", ".service=/var/lib/a"],
        &["--unit", "a b.service=/var/lib/a"],
        &["--unit", "a.service=var/lib/a"],
        &["--unit", "a.service=/x", "--unit", "a.service=/y"],
        &["--unit", "a.service=/x", "--no-restart"],
    ] {
        let output = install
            .updater(&install.base().join("current/bin/dassie"), &mirror)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}: {}", text(&output));
        assert_eq!(install.current(), install.version, "{args:?}");
    }
}

/// An update keeps the version `current` named, for the owner to go back
/// to, as well as the one the owner kept that ran it.
#[test]
fn keeps_the_version_current_named() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    fs::create_dir_all(install.base().join("9.0.0/bin")).unwrap();
    fs::create_dir_all(install.base().join("8.0.0/bin")).unwrap();
    fs::remove_file(install.base().join("current")).unwrap();
    symlink("9.0.0", install.base().join("current")).unwrap();
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let binary = install.base().join(&install.version).join("bin/dassie");
    let output = install
        .updater(&binary, &mirror)
        .arg("--no-restart")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(install.current(), "9.9.9");
    let mut kept = vec![
        install.version.clone(),
        "9.0.0".into(),
        "9.9.9".into(),
        "current".into(),
    ];
    kept.sort();
    assert_eq!(install.entries(), kept);
}

/// The script the tests stand in for the binary runs, which needs a shell.
#[test]
fn stand_in_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path: &Path = &dir.path().join("dassie");
    let mut file = fs::File::create(path).unwrap();
    file.write_all(stand_in("1.2.3").as_bytes()).unwrap();
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    let output = output_of_written(Command::new(path).arg("--version"));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "dassie 1.2.3"
    );
}
