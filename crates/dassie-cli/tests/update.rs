//! `dassie update` as a process, against a mirror served here over TLS.
//!
//! The binary is built with the `test-key` feature, so it trusts the test
//! key of testdata/release, with which these tests sign. It runs from an
//! install laid out as the installer lays it out, in a temporary directory:
//! `<base>/<version>/bin/dassie` and `<base>/current`. The releases it is
//! offered hold a stand-in binary, a script that names its version and
//! passes `verify-install`. systemd is left alone (`--no-restart`), and the
//! units go to a directory of the test's.

#![cfg(feature = "test-key")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_release::layout::{self, Arch, Compression};
use dassie_release::testing::{TEST_SECRET_KEY, sign};
use flate2::write::GzEncoder;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const DASSIE: &str = env!("CARGO_BIN_EXE_dassie");
const PEBBLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/pebble");

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
    let certs = CertificateDer::pem_file_iter(format!("{PEBBLE}/cert.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(format!("{PEBBLE}/key.pem")).unwrap();
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
            .env("DASSIE_MIRROR_CA_FILE", format!("{PEBBLE}/root.pem"))
            .env("DASSIE_SYSTEMD_UNIT_DIR", self.dir.path().join("units"))
            .env_remove("DASSIE_MIRROR_URL")
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
    let name = layout::bundle(&parsed, Arch::current().unwrap(), Compression::Gz);
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

/// `meta/<channel>` is not signed, so a version it names must be of that
/// channel: else a mirror could take a node that switches channels to any
/// version, back included, or offer a canary as the newest release.
#[test]
fn refuses_a_version_of_another_channel() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "0.0.1", &release("0.0.1"), None);
    mirror.put("meta/canary", "0.0.1");
    refused(
        &install,
        &install.update(&mirror, &["--channel", "canary"]),
        "meta/canary names 0.0.1, which is not a canary version",
    );
    let canary = "canary-2026-10-16-120000-abcdef0";
    publish(&mirror, canary, &release(canary), None);
    refused(
        &install,
        &install.update(&mirror, &[]),
        &format!("meta/latest names {canary}, which is not a latest version"),
    );
}

#[test]
fn refuses_a_tampered_bundle() {
    let (mirror, install) = (MirrorServer::start(), Install::new());
    publish(&mirror, "9.9.9", &release("9.9.9"), None);
    let name = layout::bundle(
        &dassie_release::Version::parse("9.9.9").unwrap(),
        Arch::current().unwrap(),
        Compression::Gz,
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
