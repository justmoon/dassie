#!/bin/sh

main() {
  set -u

  # Configuration options with defaults
  # ----------------------------------------------------------------------------

  : "${DASSIE_MIRROR_URL:="https://get.dassie.land"}"
  : "${DASSIE_VERSION:="latest"}"

  # Gather information about the system
  # ----------------------------------------------------------------------------

  UNAME_SYSTEM="$(uname -s)"

  UNAME_MACHINE="$(uname -m)"

  test -t 1 && IS_TTY=1 || IS_TTY=0

  HAS_COLOR="$(supports_color "$IS_TTY")"

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

  # TODO: Check for systemd
  # TODO: Check for curl

  # Download and extract Dassie bundle
  # ----------------------------------------------------------------------------

  # Get latest version from the mirror if "latest" was specified
  if [ "$DASSIE_VERSION" = "latest" ]; then
    step "Determining latest Dassie version..."
    DASSIE_VERSION="$(download "${DASSIE_MIRROR_URL}/meta/latest" -)" \
      || error "E_VERSION_FETCH_FAILED" "Failed to determine latest Dassie version."
  fi

  # Create a temporary directory to store the downloaded files
  TEMP_DIRECTORY="$(mktemp -d -t dassie-installer-XXXXXXXXXX)"

  # Make sure we clean up the temp directory, even if the script exits prematurely
  trap 'rm -rf "$TEMP_DIRECTORY"' EXIT

  DASSIE_BUNDLE_NAME="dassie-${DASSIE_VERSION}-${DASSIE_OS}-${DASSIE_ARCHITECTURE}.tar.xz"
  DASSIE_BUNDLE_URL="${DASSIE_MIRROR_URL}/${DASSIE_VERSION}/${DASSIE_BUNDLE_NAME}"

  step "Downloading ${DASSIE_BUNDLE_NAME}..."
  download "$DASSIE_BUNDLE_URL" "${TEMP_DIRECTORY}/${DASSIE_BUNDLE_NAME}" \
    || error "E_BUNDLE_DOWNLOAD_FAILED" "Failed to download ${DASSIE_BUNDLE_URL}"

  step "Extracting ${DASSIE_BUNDLE_NAME}..."
  run tar -xf "${TEMP_DIRECTORY}/${DASSIE_BUNDLE_NAME}" -C "$TEMP_DIRECTORY" \
    || error "E_BUNDLE_EXTRACTION_FAILED" "Failed to extract ${DASSIE_BUNDLE_NAME}"

  step "Installing Dassie to /opt/dassie..."
  run_root mkdir -p "/opt/dassie" \
    || error "E_BUNDLE_INSTALL_FAILED" "Failed to create directory /opt/dassie/${DASSIE_VERSION}."
  if [ -d "/opt/dassie/${DASSIE_VERSION}" ]; then
    run_root rm -rf "/opt/dassie/${DASSIE_VERSION}" \
      || error "E_BUNDLE_INSTALL_FAILED" "Failed to remove existing Dassie installation at /opt/dassie/${DASSIE_VERSION}."
  fi
  run_root cp -r "${TEMP_DIRECTORY}/dassie" "/opt/dassie/${DASSIE_VERSION}" \
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

  if [ "$(id -u)" -ne 0 ]; then
    if ! id -nG "$(whoami)" | grep -qw dassie-users; then
      step "Adding $(whoami) to dassie-users group..."
      run_root usermod -a -G dassie-users "$(whoami)"
    else
      step "Skip adding current user to dassie-users group - already a member"
    fi
  else
    step "Skip adding current user to dassie-users group - user is root"
  fi

  step "Installing systemd units..."
  run_root cp "${TEMP_DIRECTORY}"/dassie/share/systemd/* /etc/systemd/system/ \
    || error "E_SYSTEMD_SERVICE_INSTALL_FAILED" "Failed to install systemd units."

  step "Enabling systemd service..."
  run_root systemctl enable dassie.service \
    || error "E_SYSTEMD_SERVICE_ENABLE_FAILED" "Failed to enable systemd service."

  step "Reloading systemd configuration..."
  run_root systemctl daemon-reload \
    || error "E_SYSTEMD_DAEMON_RELOAD_FAILED" "Failed to reload systemd configuration."

  step "Starting systemd service..."
  run_root systemctl restart dassie.service \
    || error "E_SYSTEMD_SERVICE_START_FAILED" "Failed to start systemd service."

  step "Verify Dassie installation..."
  run dassie verify-install \
    || error "E_SELF_TEST_FAILED" "Dassie installation verification failed. Please look for any error messages above to understand why it failed."

  echo ""
  echo "$(color "0;1;7;32") + $(color "0;1;32") Dassie installed successfully! $(color "0")"
  echo ""
  echo "$(color "0;2")The next step is to configure Dassie's web interface. You can do this by running:$(color "0")"
  echo "$(color "0;32")dassie init$(color "0")"
  echo ""
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

# Print a CSI ANSI escape sequence
color() {
  if [ "$HAS_COLOR" = "1" ]; then
    printf "\e[%sm" "$1"
  fi
}

to_lowercase() {
  echo "$1" | tr '[:upper:]' '[:lower:]'
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
