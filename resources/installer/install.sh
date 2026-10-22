#!/bin/sh
# Installs Dassie (spec/areas/release.md#installsh,
# spec/areas/release.md#install-layout, spec/decisions/release-integrity.md):
#
#   curl --proto =https --tlsv1.2 -sSf https://sh.dassie.land | sh
#   curl ... | sh -s -- [--channel canary] [--target <version>] [uninstall]
#
# It installs a release into /opt/dassie/<version>, points
# /opt/dassie/current at it, links /usr/local/bin/dassie, and sets up the
# systemd units, which run the node as an unprivileged user on ports 80 and
# 443 and update it daily.
#
# Nothing is unpacked before it is verified: SHA256SUMS must carry the
# release key's minisign signature, for the version asked for, and the
# bundle must match its sum. The signature is checked with minisign, or
# else with OpenSSL (1.1.1 or later). The release key:
RELEASE_KEY="@RELEASE_KEY@"
#
# The whole script is one function, called on its last line, so a download
# cut short runs nothing.

main() {
  set -eu
  # What it installs must be readable by the node's own user, whatever the
  # umask of whoever runs it.
  umask 022
  MIRROR="${DASSIE_MIRROR_URL:-https://get.dassie.land}"
  CHANNEL="${DASSIE_CHANNEL:-latest}"
  CHANNEL_GIVEN="${DASSIE_CHANNEL:+1}"
  VERSION="${DASSIE_VERSION:-}"
  TARGETED="${DASSIE_VERSION:+1}"
  COMMAND=install
  BASE=/opt/dassie
  UNITS="dassie.service dassie-http.socket dassie-https.socket dassie-ipc.socket dassie-quic.socket dassie-feed.socket dassie-update.service dassie-update.timer"
  parse_arguments "$@"
  case "$RELEASE_KEY" in
    @*) fail E_NO_RELEASE_KEY "this installer was built without a release key" ;;
  esac
  case "$COMMAND" in
    install) install ;;
    uninstall) uninstall ;;
  esac
}

usage() {
  cat <<EOF
Usage: install.sh [options] [install|uninstall]

Options:
  -c, --channel <latest|canary>  the channel to install from (default: the
                                 installed version's channel, or latest)
  -t, --target <version>         this version rather than the channel's newest
  -m, --mirror <url>             the mirror (default: https://get.dassie.land)
  -h, --help                     show this help

Environment: DASSIE_MIRROR_URL, DASSIE_CHANNEL, DASSIE_VERSION.
EOF
}

parse_arguments() {
  while [ $# -gt 0 ]; do
    case "$1" in
      -h | --help) usage; exit 0 ;;
      -c | --channel) [ $# -ge 2 ] || fail E_SYNTAX "$1 needs a value"; CHANNEL=$2; CHANNEL_GIVEN=1; shift ;;
      -t | --target) [ $# -ge 2 ] || fail E_SYNTAX "$1 needs a value"; VERSION=$2; TARGETED=1; shift ;;
      -m | --mirror) [ $# -ge 2 ] || fail E_SYNTAX "$1 needs a value"; MIRROR=$2; shift ;;
      install | uninstall) COMMAND=$1 ;;
      *) usage >&2; fail E_SYNTAX "unknown argument: $1" ;;
    esac
    shift
  done
  case "$CHANNEL" in
    latest | canary) ;;
    *) fail E_SYNTAX "unknown channel: $CHANNEL" ;;
  esac
  case "$MIRROR" in
    https://*) MIRROR=${MIRROR%/} ;;
    *) fail E_SYNTAX "the mirror must be an https:// URL" ;;
  esac
}

# --- Output -----------------------------------------------------------------

color() {
  if [ -n "${NO_COLOR:-}" ]; then return 1; fi
  [ -t 1 ] && [ "${TERM:-dumb}" != dumb ]
}

step() {
  if color; then printf '\n\033[1;36m▶ %s\033[0m\n' "$*"; else printf '\n▶ %s\n' "$*"; fi
}

note() {
  printf '  %s\n' "$*"
}

fail() {
  code=$1
  shift
  if color; then printf '\n\033[1;31m! Error (%s)\033[0m %s\n' "$code" "$*" >&2; else printf '\n! Error (%s) %s\n' "$code" "$*" >&2; fi
  exit 1
}

# Runs a command as root, through sudo if need be, and shows it.
as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    printf '  # %s\n' "$*"
    "$@"
  else
    command -v sudo > /dev/null 2>&1 || fail E_NO_SUDO "run this as root, or install sudo"
    printf '  # sudo %s\n' "$*"
    sudo "$@"
  fi
}

# --- Checks -----------------------------------------------------------------

check_system() {
  [ "$(uname -s)" = Linux ] || fail E_UNSUPPORTED_OS "Dassie runs on Linux only"
  case "$(uname -m)" in
    x86_64 | x86-64 | x64 | amd64) ARCH=x64 ;;
    aarch64 | arm64) ARCH=arm64 ;;
    *) fail E_UNSUPPORTED_ARCH "there are no releases for $(uname -m)" ;;
  esac
  command -v systemctl > /dev/null 2>&1 || fail E_SYSTEMD_REQUIRED "Dassie needs systemd"
  state=$(systemctl is-system-running 2> /dev/null || true)
  # Still starting is fine: cloud-init runs scripts while the system boots.
  case "$state" in
    running | degraded | starting | initializing) ;;
    *) fail E_SYSTEMD_NOT_RUNNING "systemd is not running (it says: ${state:-nothing})" ;;
  esac
  for tool in tar sha256sum base64 awk mktemp flock; do
    command -v "$tool" > /dev/null 2>&1 || fail E_TOOL_REQUIRED "$tool is required"
  done
  if command -v curl > /dev/null 2>&1; then
    DOWNLOAD=curl
  elif command -v wget > /dev/null 2>&1; then
    DOWNLOAD=wget
  else
    fail E_CURL_OR_WGET_REQUIRED "curl or wget is required"
  fi
  if command -v minisign > /dev/null 2>&1; then
    VERIFIER=minisign
  elif command -v openssl > /dev/null 2>&1 && openssl pkeyutl -help 2>&1 | grep -q rawin; then
    VERIFIER=openssl
  else
    fail E_VERIFIER_REQUIRED "minisign, or OpenSSL 1.1.1 or later, is required to verify the release"
  fi
  # xz is smaller; gzip is everywhere.
  if command -v xz > /dev/null 2>&1; then COMPRESSION=xz; else COMPRESSION=gz; fi
}

# Fetches a URL into a file, HTTPS only, failing on any error status.
download() {
  case "$DOWNLOAD" in
    curl) curl --proto =https --tlsv1.2 --fail --silent --show-error --location \
      --retry 3 --retry-delay 5 --output "$2" "$1" ;;
    wget) wget --https-only --quiet --tries=3 --output-document="$2" "$1" ;;
  esac
}

valid_version() {
  case "$1" in
    "" | *[!0-9a-z.-]*) return 1 ;;
  esac
  printf '%s\n' "$1" | grep -Eq '^((0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)|canary-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{6}-[0-9a-f]{7,40})$'
}

channel_of() {
  case "$1" in
    canary-*) echo canary ;;
    *) echo latest ;;
  esac
}

# --- Verification -------------------------------------------------------------

# Verifies $TEMP/SHA256SUMS.minisig: the release key's signature of
# SHA256SUMS, with the trusted comment "dassie $VERSION".
verify_signature() {
  expected="dassie $VERSION"
  case "$VERIFIER" in
    minisign)
      comment=$(minisign -V -H -Q -P "$RELEASE_KEY" -m "$TEMP/SHA256SUMS" -x "$TEMP/SHA256SUMS.minisig" 2> /dev/null) ||
        fail E_SIGNATURE_INVALID "SHA256SUMS is not signed with the release key"
      ;;
    openssl) comment=$(verify_with_openssl) ;;
  esac
  [ "$comment" = "$expected" ] ||
    fail E_SIGNATURE_INVALID "SHA256SUMS is signed for another version ($comment), not $VERSION"
}

# minisign's format, checked with OpenSSL: the second line is ED, the key
# ID and the Ed25519 signature of the file's BLAKE2b-512; the fourth, the
# signature of that signature followed by the trusted comment. Prints the
# trusted comment.
verify_with_openssl() {
  work="$TEMP/openssl"
  mkdir "$work" || fail E_TEMP_DIRECTORY_FAILED "cannot make $work"
  printf '%s' "$RELEASE_KEY" | base64 -d > "$work/key" 2> /dev/null || fail E_SIGNATURE_INVALID "the release key is malformed"
  # SubjectPublicKeyInfo for Ed25519, then the key's last 32 bytes.
  { printf '\060\052\060\005\006\003\053\145\160\003\041\000'; tail -c 32 "$work/key"; } > "$work/key.der" ||
    fail E_SIGNATURE_INVALID "cannot read the release key"
  { echo '-----BEGIN PUBLIC KEY-----'; base64 < "$work/key.der" | tr -d '\n'; echo; echo '-----END PUBLIC KEY-----'; } > "$work/key.pem"
  sig="$TEMP/SHA256SUMS.minisig"
  [ "$(wc -l < "$sig")" -eq 4 ] || fail E_SIGNATURE_INVALID "SHA256SUMS.minisig is malformed"
  sed -n 2p "$sig" | base64 -d > "$work/signature" 2> /dev/null || fail E_SIGNATURE_INVALID "SHA256SUMS.minisig is malformed"
  sed -n 4p "$sig" | base64 -d > "$work/global" 2> /dev/null || fail E_SIGNATURE_INVALID "SHA256SUMS.minisig is malformed"
  [ "$(wc -c < "$work/signature")" -eq 74 ] && [ "$(wc -c < "$work/global")" -eq 64 ] ||
    fail E_SIGNATURE_INVALID "SHA256SUMS.minisig is malformed"
  [ "$(head -c 2 "$work/signature")" = ED ] || fail E_SIGNATURE_INVALID "SHA256SUMS.minisig uses another algorithm"
  # The key IDs, compared as base64: not every system has cmp.
  [ "$(head -c 10 "$work/signature" | tail -c 8 | base64)" = "$(head -c 10 "$work/key" | tail -c 8 | base64)" ] ||
    fail E_SIGNATURE_INVALID "SHA256SUMS is signed with another key"
  tail -c 64 "$work/signature" > "$work/signature.ed25519" || fail E_SIGNATURE_INVALID "cannot read SHA256SUMS.minisig"
  openssl dgst -blake2b512 -binary "$TEMP/SHA256SUMS" > "$work/hash" ||
    fail E_VERIFIER_REQUIRED "this OpenSSL cannot compute BLAKE2b-512; install minisign"
  openssl pkeyutl -verify -pubin -inkey "$work/key.pem" -rawin -in "$work/hash" \
    -sigfile "$work/signature.ed25519" > /dev/null 2>&1 ||
    fail E_SIGNATURE_INVALID "SHA256SUMS is not signed with the release key"
  line=$(sed -n 3p "$sig")
  comment=${line#trusted comment: }
  [ "$comment" != "$line" ] || fail E_SIGNATURE_INVALID "SHA256SUMS.minisig is malformed"
  { cat "$work/signature.ed25519"; printf '%s' "$comment"; } > "$work/global.message" ||
    fail E_SIGNATURE_INVALID "cannot read SHA256SUMS.minisig"
  openssl pkeyutl -verify -pubin -inkey "$work/key.pem" -rawin -in "$work/global.message" \
    -sigfile "$work/global" > /dev/null 2>&1 ||
    fail E_SIGNATURE_INVALID "the trusted comment of SHA256SUMS.minisig is not signed"
  printf '%s' "$comment"
}

verify_bundle() {
  expected=$(awk -v name="$BUNDLE" '$2 == name || $2 == "*" name { print $1 }' "$TEMP/SHA256SUMS")
  [ -n "$expected" ] || fail E_BUNDLE_VERIFY_FAILED "SHA256SUMS lists no $BUNDLE"
  actual=$(sha256sum "$TEMP/$BUNDLE" | awk '{ print $1 }')
  [ "$actual" = "$expected" ] || fail E_BUNDLE_VERIFY_FAILED "$BUNDLE does not match its signed sum: refusing it"
}

# The version installed, if its binary names one. One that names none is
# taken for no install: each binary named its version before it was
# installed, so the mirror cannot make one fall silent.
read_installed() {
  INSTALLED=
  if [ -x "$BASE/current/bin/dassie" ]; then
    INSTALLED=$("$BASE/current/bin/dassie" --version 2> /dev/null | sed -n 's/^dassie //p')
  fi
}

# Without --channel, a re-run keeps to the installed version's channel, as
# `dassie update` does.
choose_channel() {
  if [ -z "$CHANNEL_GIVEN" ] && [ -n "$INSTALLED" ]; then
    CHANNEL=$(channel_of "$INSTALLED")
  fi
}

# The mirror's meta/<channel> is not signed, so it does not choose the
# channel: what it names must be of the channel it was read from.
check_channel() {
  [ "$(channel_of "$VERSION")" = "$CHANNEL" ] ||
    fail E_VERSION_INVALID "the mirror's meta/$CHANNEL names $VERSION, which is not a $CHANNEL version"
}

# Nor does it choose the direction: a version of the other channel goes in
# only when asked for with --channel, and one older than the one installed,
# of the same channel, only with --target.
check_not_older() {
  if [ -n "$TARGETED" ] || [ -z "$INSTALLED" ]; then return 0; fi
  if [ "$(channel_of "$VERSION")" != "$(channel_of "$INSTALLED")" ]; then
    [ -n "$CHANNEL_GIVEN" ] ||
      fail E_VERSION_OTHER_CHANNEL "the mirror offers $VERSION, of another channel than the installed $INSTALLED; use --channel $(channel_of "$VERSION") to move to it"
    return 0
  fi
  case "$VERSION" in
    canary-*)
      # The date and time of the commit order canaries.
      offered=$(printf '%s' "$VERSION" | cut -c1-24)
      now=$(printf '%s' "$INSTALLED" | cut -c1-24)
      ;;
    *) offered=$VERSION now=$INSTALLED ;;
  esac
  [ "$offered" != "$now" ] || return 0
  lowest=$(printf '%s\n%s\n' "$offered" "$now" | sort -V | head -n 1)
  [ "$lowest" != "$offered" ] ||
    fail E_VERSION_OLDER "the mirror offers $VERSION, older than the installed $INSTALLED; use --target to install it anyway"
}

# --- Lock -----------------------------------------------------------------------

# One at a time with `dassie update`, which holds the same lock, with
# flock(2), while it installs, switches and prunes: an update removes every
# version it does not keep, and what an install leaves half done. Held on
# file descriptor 9 until the script exits. Taken as soon as there is an
# install, which an update may be changing; a first install makes nothing
# before the release is verified.
lock() {
  if [ -n "${LOCKED:-}" ]; then return 0; fi
  command -v flock > /dev/null 2>&1 || fail E_TOOL_REQUIRED "flock is required"
  if [ ! -e "$BASE/.update.lock" ]; then
    as_root mkdir -p "$BASE"
    as_root touch "$BASE/.update.lock" || fail E_LOCK_FAILED "cannot make $BASE/.update.lock"
  fi
  [ -r "$BASE/.update.lock" ] || fail E_LOCK_FAILED "cannot read $BASE/.update.lock; run this as root"
  exec 9< "$BASE/.update.lock"
  flock -n 9 || fail E_UPDATE_RUNNING "an update is running; run this again once it is done"
  LOCKED=1
}

# --- Install --------------------------------------------------------------------

install() {
  check_system
  TEMP=$(mktemp -d -t dassie-installer-XXXXXXXXXX) || fail E_TEMP_DIRECTORY_FAILED "cannot make a temporary directory"
  trap 'rm -rf "$TEMP"' EXIT

  if [ -d "$BASE" ]; then lock; fi
  read_installed
  choose_channel
  if [ -z "$VERSION" ]; then
    step "Looking up the newest $CHANNEL version"
    download "$MIRROR/meta/$CHANNEL" "$TEMP/meta" || fail E_VERSION_FETCH_FAILED "cannot read $MIRROR/meta/$CHANNEL"
    VERSION=$(tr -d ' \r\n' < "$TEMP/meta")
  fi
  valid_version "$VERSION" || fail E_VERSION_INVALID "not a version: $VERSION"
  [ -n "$TARGETED" ] || check_channel
  note "dassie $VERSION"
  check_not_older

  step "Downloading and verifying dassie $VERSION for $ARCH"
  BUNDLE="dassie-$VERSION-linux-$ARCH.tar.$COMPRESSION"
  for file in SHA256SUMS SHA256SUMS.minisig "$BUNDLE"; do
    download "$MIRROR/$VERSION/$file" "$TEMP/$file" || fail E_BUNDLE_DOWNLOAD_FAILED "cannot download $MIRROR/$VERSION/$file"
  done
  verify_signature
  verify_bundle
  note "signed by the release key, and matching its sum"

  step "Unpacking"
  mkdir "$TEMP/unpacked"
  case "$COMPRESSION" in
    xz) tar -xJf "$TEMP/$BUNDLE" -C "$TEMP/unpacked" ;;
    gz) tar -xzf "$TEMP/$BUNDLE" -C "$TEMP/unpacked" ;;
  esac || fail E_BUNDLE_EXTRACTION_FAILED "cannot unpack $BUNDLE"
  unpacked="$TEMP/unpacked/dassie"
  [ -z "$(find "$TEMP/unpacked" ! -type f ! -type d)" ] && [ -x "$unpacked/bin/dassie" ] ||
    fail E_BUNDLE_EXTRACTION_FAILED "$BUNDLE does not hold what a release holds"
  [ "$("$unpacked/bin/dassie" --version)" = "dassie $VERSION" ] ||
    fail E_BUNDLE_EXTRACTION_FAILED "the binary in $BUNDLE is not dassie $VERSION"
  "$unpacked/bin/dassie" verify-install > /dev/null || fail E_SELF_TEST_FAILED "dassie $VERSION fails verify-install here"

  step "Installing into $BASE/$VERSION"
  lock
  install_files "$unpacked"
  install_units "$unpacked"

  step "Dassie $VERSION is installed"
  note "Next, set up the node:  sudo dassie init"
}

install_files() {
  as_root mkdir -p "$BASE"
  as_root rm -rf "$BASE/.install-$VERSION"
  as_root cp -R "$1" "$BASE/.install-$VERSION" || fail E_BUNDLE_INSTALL_FAILED "cannot copy into $BASE"
  # A version installed again is renamed aside first, so that current
  # never names nothing.
  if [ -e "$BASE/$VERSION" ]; then
    as_root mv -T "$BASE/$VERSION" "$BASE/.replaced-$VERSION-$$" || fail E_BUNDLE_INSTALL_FAILED "cannot replace $BASE/$VERSION"
  fi
  as_root mv -T "$BASE/.install-$VERSION" "$BASE/$VERSION" || fail E_BUNDLE_INSTALL_FAILED "cannot move into $BASE/$VERSION"
  as_root rm -rf "$BASE/.replaced-$VERSION-$$"
  # On disk before current names it; then one rename points current at it.
  as_root sync
  as_root ln -sfn "$VERSION" "$BASE/.current-$$"
  as_root mv -T "$BASE/.current-$$" "$BASE/current" || fail E_VERSION_SELECT_FAILED "cannot point $BASE/current at $VERSION"
  as_root sync
  as_root ln -sfn "$BASE/current/bin/dassie" /usr/local/bin/dassie ||
    fail E_DASSIE_COMMAND_SYMLINK_FAILED "cannot link /usr/local/bin/dassie"
  if ! getent group dassie-users > /dev/null 2>&1; then
    as_root groupadd --system dassie-users || fail E_GROUP_CREATION_FAILED "cannot create the group dassie-users"
  fi
  # Whoever ran this, through sudo or with sudo for each step, may use the
  # node's socket.
  if [ "$(id -u)" -ne 0 ]; then user=$(id -un); else user=${SUDO_USER:-root}; fi
  if [ "$user" != root ]; then
    as_root usermod -a -G dassie-users "$user" || fail E_GROUP_CREATION_FAILED "cannot add $user to dassie-users"
    note "$user may now use dassie on this machine, once logged in again"
  fi
}

install_units() {
  step "Setting up systemd"
  as_root cp "$1"/share/systemd/* /etc/systemd/system/ || fail E_SYSTEMD_SERVICE_INSTALL_FAILED "cannot install the units"
  as_root systemctl daemon-reload || fail E_SYSTEMD_DAEMON_RELOAD_FAILED "systemctl daemon-reload failed"
  as_root systemctl enable dassie-http.socket dassie-https.socket dassie-ipc.socket dassie-quic.socket dassie-feed.socket dassie.service dassie-update.timer ||
    fail E_SYSTEMD_SERVICE_ENABLE_FAILED "cannot enable the units"
  as_root systemctl start dassie-http.socket dassie-https.socket dassie-ipc.socket ||
    fail E_SYSTEMD_SERVICE_START_FAILED "cannot start the sockets"
  # The node can do without QUIC: peers then reach it over HTTPS.
  as_root systemctl start dassie-quic.socket ||
    note "UDP port 443 is taken: peers reach this node over HTTPS only"
  # Nor does it need its rate feed: it then publishes none.
  as_root systemctl start dassie-feed.socket ||
    note "UDP port 7768 is taken: the node publishes no rate feed"
  as_root systemctl restart dassie.service dassie-update.timer ||
    fail E_SYSTEMD_SERVICE_START_FAILED "cannot start dassie.service"
}

# --- Uninstall ------------------------------------------------------------------

# The original's uninstall left the updater's units and the link behind
# (RT-19, UI-13). The node's data, which holds its key and its books, and
# the group dassie-users stay on purpose
# (spec/areas/release.md#installsh).
uninstall() {
  if [ -d "$BASE" ]; then lock; fi
  step "Stopping Dassie"
  # shellcheck disable=SC2086 # the unit names are words
  as_root systemctl disable --now $UNITS 2> /dev/null || true
  step "Removing it"
  for unit in $UNITS; do
    as_root rm -f "/etc/systemd/system/$unit" || fail E_SYSTEMD_SERVICE_REMOVE_FAILED "cannot remove $unit"
  done
  as_root systemctl daemon-reload || fail E_SYSTEMD_DAEMON_RELOAD_FAILED "systemctl daemon-reload failed"
  as_root rm -rf "$BASE" || fail E_BUNDLE_REMOVE_FAILED "cannot remove $BASE"
  if [ -L /usr/local/bin/dassie ]; then
    as_root rm -f /usr/local/bin/dassie
  fi
  step "Dassie is uninstalled"
  note "The node's data stays in /var/lib/private/dassie, and the group dassie-users stays too."
}

main "$@"
