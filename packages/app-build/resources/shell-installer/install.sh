#!/bin/sh

main() {
  set -u

  # Configuration options with defaults
  # ----------------------------------------------------------------------------

  : "${DASSIE_MIRROR_URL:="https://get.dassie.land"}"
  : "${DASSIE_VERSION:="latest"}"
  DASSIE_ACTION="install"

  # Gather information about the system
  # ----------------------------------------------------------------------------

  UNAME_SYSTEM="$(uname -s)"

  UNAME_MACHINE="$(uname -m)"

  test -t 1 && IS_TTY=1 || IS_TTY=0

  HAS_COLOR="$(supports_color "$IS_TTY")"

  # Parse command-line options
  # ----------------------------------------------------------------------------

  shift
  while [ -n "${1-}" ]; do
    case "$1" in
      -h | -\? | --help)
        show_help
        exit 1
        ;;
      -t | --target) # Specify target version
        if [ -n "${2-}" ]; then
          DASSIE_VERSION="$2"
          shift
        else
          error "E_SYNTAX" "--target requires a non-empty option argument."
        fi
        ;;
      --target=*)
        DASSIE_VERSION="${1#*=}"
        ;;
      -m | --mirror) # Specify Dassie mirror URL
        if [ -n "${2-}" ]; then
          DASSIE_MIRROR_URL="$2"
          shift
        else
          error "E_SYNTAX" "--mirror requires a non-empty option argument."
        fi
        ;;
      --mirror=*)
        DASSIE_MIRROR_URL="${1#*=}"
        ;;
      install)
        DASSIE_ACTION="install"
        ;;
      uninstall)
        DASSIE_ACTION="uninstall"
        ;;
      *)
        error "E_SYNTAX" "Unknown option: ${1-}"
        ;;
    esac

    shift
  done

  case "$DASSIE_ACTION" in
    install)
      install
      ;;
    uninstall)
      uninstall
      ;;
  esac
}

install() {
  echo ""
  echo "$(color "0;1;7;34") Dassie Installer $(color "0")"

  # Validate system requirements
  # ----------------------------------------------------------------------------

  # Map uname operating system to operating system identifier in the bundle URL
  case "$(to_lowercase "$UNAME_SYSTEM")" in
    linux)
      DASSIE_OS=linux
      ;;

    *)
      error "E_UNSUPPORTED_OS" "Your operating system ($UNAME_SYSTEM) is not supported by this installer."
      ;;
  esac

  # Map uname machine to architecture identifier in the bundle URL
  case "$(to_lowercase "$UNAME_MACHINE")" in
    x86_64 | x86-64 | x64 | amd64)
      DASSIE_ARCHITECTURE=x64
      ;;

    aarch64 | arm64)
      DASSIE_ARCHITECTURE=arm64
      ;;

    armv7l | armv8l)
      DASSIE_ARCHITECTURE=arm7l
      ;;

    *)
      error "E_UNSUPPORTED_ARCH" "Your system architecture ($UNAME_MACHINE) is not supported by this installer."
      ;;
  esac

  # Check that systemd is installed and running
  verify_systemd

  # Check that at least one of curl or wget are installed
  if ! quiet command -v curl && ! quiet command -v wget; then
    error "E_CURL_OR_WGET_REQUIRED" "Either curl or wget is required to download Dassie."
  fi

  # Check that tar is installed
  if ! quiet command -v tar; then
    error "E_TAR_REQUIRED" "Tar is required to extract Dassie."
  fi

  # Download and extract Dassie bundle
  # ----------------------------------------------------------------------------

  # Get latest version from the mirror if "latest" was specified
  if [ "$DASSIE_VERSION" = "latest" ]; then
    step "Determining latest Dassie version..."
    DASSIE_VERSION="$(download "${DASSIE_MIRROR_URL}/meta/latest" -)" \
      || error "E_VERSION_FETCH_FAILED" "Failed to determine latest Dassie version."
  fi


  # Make sure we clean up the temp directory, even if the script exits prematurely
  PREVIOUS_DIRECTORY="$(pwd)"
  trap 'cd "$PREVIOUS_DIRECTORY"; rm -rf "$TEMP_DIRECTORY"' EXIT

  DASSIE_BUNDLE_NAME="dassie-${DASSIE_VERSION}-${DASSIE_OS}-${DASSIE_ARCHITECTURE}.tar.xz"
  DASSIE_BUNDLE_URL="${DASSIE_MIRROR_URL}/${DASSIE_VERSION}/${DASSIE_BUNDLE_NAME}"

  step "Creating temporary directory..."
  TEMP_DIRECTORY="$(run mktemp -d -t dassie-installer-XXXXXXXXXX)"
  run cd "$TEMP_DIRECTORY" \
    || error "E_TEMP_DIRECTORY_FAILED" "Failed to set up temporary directory."

  step "Downloading ${DASSIE_BUNDLE_NAME}..."
  download "$DASSIE_BUNDLE_URL" "${DASSIE_BUNDLE_NAME}" \
    || error "E_BUNDLE_DOWNLOAD_FAILED" "Failed to download ${DASSIE_BUNDLE_URL}"

  step "Extracting ${DASSIE_BUNDLE_NAME}..."
  run tar -xf "${DASSIE_BUNDLE_NAME}" -C . \
    || error "E_BUNDLE_EXTRACTION_FAILED" "Failed to extract ${DASSIE_BUNDLE_NAME}"

  step "Installing Dassie to /opt/dassie..."
  run_root mkdir -p "/opt/dassie" \
    || error "E_BUNDLE_INSTALL_FAILED" "Failed to create directory /opt/dassie/${DASSIE_VERSION}."
  if [ -d "/opt/dassie/${DASSIE_VERSION}" ]; then
    run_root rm -rf "/opt/dassie/${DASSIE_VERSION}" \
      || error "E_BUNDLE_INSTALL_FAILED" "Failed to remove existing Dassie installation at /opt/dassie/${DASSIE_VERSION}."
  fi
  run_root cp -r "dassie" "/opt/dassie/${DASSIE_VERSION}" \
    || error "E_BUNDLE_INSTALL_FAILED" "Failed to install Dassie to /opt/dassie/${DASSIE_VERSION}."
  if [ -L "/opt/dassie/current" ]; then
    run_root rm "/opt/dassie/current" \
      || error "E_VERSION_SELECT_FAILED" "Failed to remove symlink to current Dassie version."
  fi
  run_root ln -s "/opt/dassie/${DASSIE_VERSION}" /opt/dassie/current \
    || error "E_VERSION_SELECT_FAILED" "Failed to create symlink to select current Dassie version."
  run_root ln -sf /opt/dassie/current/bin/dassie /usr/local/bin/dassie \
    || error "E_DASSIE_COMMAND_SYMLINK_FAILED" "Failed to create symlink to Dassie command."

  if ! quiet getent group dassie-users; then
    step "Creating dassie-users group..."
    run_root groupadd dassie-users \
      || error "E_GROUP_CREATION_FAILED" "Failed to create dassie-users group."
  else
    step "Skip creation of dassie-users group - already exists"
  fi

  ADDED_GROUP_MEMBERSHIP=0
  if [ "$(id -u)" -ne 0 ]; then
    if ! id -nG "$(whoami)" | grep -qw dassie-users; then
      step "Adding $(whoami) to dassie-users group..."
      run_root usermod -a -G dassie-users "$(whoami)"
      ADDED_GROUP_MEMBERSHIP=1
    else
      step "Skip adding current user to dassie-users group - already a member"
    fi
  else
    step "Skip adding current user to dassie-users group - user is root"
  fi

  step "Installing systemd units..."
  run_root cp dassie/share/systemd/* /etc/systemd/system/ \
    || error "E_SYSTEMD_SERVICE_INSTALL_FAILED" "Failed to install systemd units."

  step "Enabling systemd service..."
  run_root systemctl enable dassie.service dassie-update.timer \
    || error "E_SYSTEMD_SERVICE_ENABLE_FAILED" "Failed to enable systemd service."

  step "Reloading systemd configuration..."
  run_root systemctl daemon-reload \
    || error "E_SYSTEMD_DAEMON_RELOAD_FAILED" "Failed to reload systemd configuration."

  step "Starting systemd service..."
  run_root systemctl restart dassie.service dassie-update.timer \
    || error "E_SYSTEMD_SERVICE_START_FAILED" "Failed to start systemd service."

  step "Cleaning up..."
  run cd "$PREVIOUS_DIRECTORY"
  run rm -rf "$TEMP_DIRECTORY" \
    || error "E_CLEANUP_FAILED" "Failed to remove temporary directory."

  step "Verify Dassie installation..."
  run_root dassie verify-install \
    || error "E_SELF_TEST_FAILED" "Dassie installation verification failed. Please look for any error messages above to understand why it failed."

  echo ""
  echo "$(color "0;1;7;32") + $(color "0;1;32") Dassie installed successfully! $(color "0")"

  if [ "$ADDED_GROUP_MEMBERSHIP" -eq 1 ]; then
    echo ""
    echo "$(color "0;1;7;36") i $(color "0;1") The user $(color "0;1;32")$(whoami)$(color "0;1") has been added to the $(color "0;1;33")dassie-users$(color "0;1") group.$(color "0")"
    echo ""
    echo "$(color "0;1;31")This means that you will need to log out and back in again before you can use the $(color "0;1;32")dassie$(color "0;1;31") command-line tool.$(color "0")"
    echo ""
    echo "$(color "0;2")You can also add other users to the dassie-users group to give them access to Dassie. You can do this by running:$(color "0")"
    echo "$(color "0;2")\$ $(color "0;33")sudo $(color "0;32")usermod $(color "0;2")-a -G dassie-users <username>$(color "0")"
  fi
  echo ""
  echo "$(color "0;1;7;36") i $(color "0;1") Next: Configure Dassie's web interface$(color "0")"
  echo ""
  echo "$(color "0;2")You can do this by running:$(color "0")"
  echo "$(color "0;2")\$ $(color "0;32")dassie $(color "0;2")init$(color "0")"
  echo ""
}

uninstall() {
  echo ""
  echo "$(color "0;1;7;34") Dassie Uninstaller $(color "0")"

  # Check that systemd is installed and running
  verify_systemd

  step "Stopping systemd units..."
  run_root systemctl stop dassie.service dassie-http.socket dassie-https.socket dassie-ipc.socket \
    || error "E_SYSTEMD_SERVICE_STOP_FAILED" "Failed to stop systemd units."

  step "Disabling systemd units..."
  run_root systemctl disable dassie.service dassie-http.socket dassie-https.socket dassie-ipc.socket dassie-update.timer \
    || error "E_SYSTEMD_SERVICE_DISABLE_FAILED" "Failed to disable systemd units."

  step "Removing systemd units..."
  run_root rm -f /etc/systemd/system/dassie.service /etc/systemd/system/dassie-http.socket /etc/systemd/system/dassie-https.socket /etc/systemd/system/dassie-ipc.socket /etc/systemd/system/dassie-update.timer \
    || error "E_SYSTEMD_SERVICE_REMOVE_FAILED" "Failed to remove systemd units."

  step "Reloading systemd configuration..."
  run_root systemctl daemon-reload \
    || error "E_SYSTEMD_DAEMON_RELOAD_FAILED" "Failed to reload systemd configuration."

  step "Removing Dassie installation..."
  run_root rm -rf /opt/dassie \
    || error "E_BUNDLE_REMOVE_FAILED" "Failed to remove Dassie installation."
}

supports_color() {
  IS_TTY="$1"

  # Check if color support is forced via environment variable
  if [ -n "${FORCE_COLOR-}" ]; then
    case "$(to_lowercase "$FORCE_COLOR")" in
      true | yes | y | 1 | on | enabled)
        echo 1
        return
        ;;
      false | no | n | 0 | off | disabled)
        echo 0
        return
        ;;
      *)
        error "E_UNEXPECTED_FORCE_COLOR_VALUE" "The environment variable \$FORCE_COLOR has an unexpected value, use \"FORCE_COLOR=1\" or \"FORCE_COLOR=0\"."
        ;;
    esac
  fi

  # Check if colors are turned off explicitly
  # See: https://no-color.org/
  if [ -n "${NO_COLOR-}" ]; then
    echo 0
    return
  fi

  # Check if we are running in a terminal
  if [ "$IS_TTY" = "1" ]; then
    if [ "${TERM-}" = "dumb" ]; then
      echo 0
      return
    fi

    echo 1
    return
  fi

  # Default to no color support
  echo 0
}

verify_systemd() {
  if ! quiet command -v systemctl; then
    error "E_SYSTEMD_REQUIRED" "Systemd is required to run Dassie."
  fi

  case "$(systemctl is-system-running)" in
    running | degraded) ;;
    *)
      error "E_SYSTEMD_NOT_RUNNING" "Systemd is not running."
      ;;
  esac
}

# Print a CSI ANSI escape sequence
color() {
  if [ "$HAS_COLOR" = "1" ]; then
    printf "\e[%sm" "$1"
  fi
}

to_lowercase() {
  echo "$1" | tr '[:upper:]' '[:lower:]'
}

show_help() {
  echo "Usage: curl --tlsv1.2 -sSf https://sh.dassie.land | sh -s -- [options] [command]"
  echo ""
  echo "Options:"
  echo "  -h, --help               Show this help message"
  echo "  -t, --target <version>   Specify target version (default: latest)"
  echo "  -m, --mirror <url>       Specify Dassie mirror URL"
  echo "                           (default: https://get.dassie.land)"
  echo ""
  echo "Commands:"
  echo "  install                  Install Dassie (default)"
  echo "  uninstall                Uninstall Dassie"
  echo ""
  echo "Examples:"
  echo "  curl --tlsv1.2 -sSf https://sh.dassie.land | sh"
  echo "  curl --tlsv1.2 -sSf https://sh.dassie.land | sh -s -- --target 1.0.0"
  echo "  curl --tlsv1.2 -sSf https://sh.dassie.land | sh -s -- --target latest"
  echo "  curl --tlsv1.2 -sSf https://sh.dassie.land | sh -s -- -m https://example.com"
  echo "  curl --tlsv1.2 -sSf https://sh.dassie.land | sh -s -- uninstall"
  echo ""
}

# Print an error message and exit
error() {
  CODE=$1
  shift

  printf "\n$(color "0;1;2;7;31") ! $(color "0;1;31") Error (%s)\n\n$(color "0;31")%s$(color "0")\n" "$CODE" "$@" >&2
  exit 1
}

# Run a command but discard any output
quiet() {
  "$@" >/dev/null 2>&1
}

# Print a step header
step() {
  printf "\n$(color "0;1;7;34") > $(color "0;1") %s$(color "0")\n" "$*" >&2
}

# Print a command to be run and then run it
run() {
  COMMAND=$1
  shift

  printf "$(color "0;1")\$ $(color "0;1;32")%s $(color "0;2")%s$(color "0")\n" "$COMMAND" "$*" >&2
  "$COMMAND" "$@"
}

# Print a command to be run as root and then run it, use sudo if necessary
run_root() {
  COMMAND=$1
  shift

  if [ "$(id -u)" = "0" ]; then
    printf "$(color "0;1;33")# $(color "0;1;32")%s $(color "0;2")%s$(color "0")\n" "$COMMAND" "$*" >&2
    "$COMMAND" "$@"
  elif quiet command -v sudo; then
    printf "$(color "0;1;33")# $(color "0;33")sudo $(color "0;1;32")%s $(color "0;2")%s$(color "0")\n" "$COMMAND" "$*" >&2
    sudo "$COMMAND" "$@"
  else
    error "E_NO_SUDO" "No superuser privileges. You need to either run the script as root or make sure sudo is available."
  fi
}

# Download a file
download() {
  URL=$1
  OUTPUT=$2

  if command -v curl >/dev/null 2>&1; then
    run curl --tlsv1.2 --proto "=https" --retry 3 --retry-delay 5 --fail --show-error "$URL" --output "$OUTPUT"
  elif command -v wget >/dev/null 2>&1; then
    run wget --https-only --max-redirect=0 --quiet --show-progress --output-document="$OUTPUT" "$URL"
  else
    error "E_NO_DOWNLOAD_TOOL" "Neither curl nor wget are available. Please install either to proceed."
  fi
}

main "$0" "$@"
