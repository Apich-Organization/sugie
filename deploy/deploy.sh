#!/usr/bin/env bash
# Build Sugie and install it on a remote systemd host (e.g. a Raspberry Pi).
#
#   ./deploy/deploy.sh --host pi@192.168.1.50
#   ./deploy/deploy.sh --host pi@192.168.1.50 --build-on-target
#   ./deploy/deploy.sh --host pi@192.168.1.50 --binary ./sugie-aarch64
#
# The remote side runs deploy/install.sh, which creates the service account,
# lays out /opt/sugie, enables the systemd unit for boot and restarts it.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

HOST=""
BINARY=""
BUILD_ON_TARGET=0
PREFIX="/opt/sugie"
SERVICE_USER="sugie"
UNIT_NAME="sugie"
START_ARGS=""
SSH_OPTS=(-o BatchMode=no)

usage() {
    cat <<'EOF'
Usage: ./deploy/deploy.sh --host USER@HOST [options]

  --host USER@HOST   Target machine (required), e.g. pi@192.168.1.50
  --build-on-target  Build the release binary on the target instead of here.
                     Use this when the target architecture differs (aarch64 Pi).
  --binary PATH      Ship a pre-built binary instead of building.
  --prefix DIR       Install directory on the target (default: /opt/sugie)
  --user NAME        Service account on the target (default: sugie)
  --unit NAME        systemd unit name (default: sugie)
  --no-start         Install and enable, but do not start the service now
  -h, --help         Show this help

Sudo on the target is prompted over an interactive SSH session, so run this
from a terminal rather than a non-interactive job.
EOF
}

log()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[!]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[x]\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --host)            HOST="${2:?--host needs USER@HOST}"; shift 2 ;;
        --binary)          BINARY="${2:?--binary needs a path}"; shift 2 ;;
        --build-on-target) BUILD_ON_TARGET=1; shift ;;
        --prefix)          PREFIX="${2:?--prefix needs a path}"; shift 2 ;;
        --user)            SERVICE_USER="${2:?--user needs a name}"; shift 2 ;;
        --unit)            UNIT_NAME="${2:?--unit needs a name}"; shift 2 ;;
        --no-start)        START_ARGS="--no-start"; shift ;;
        -h|--help)         usage; exit 0 ;;
        *)                 die "Unknown option: $1 (try --help)" ;;
    esac
done

[ -n "$HOST" ] || { usage; exit 1; }

ssh_cmd() { ssh "${SSH_OPTS[@]}" "$HOST" "$@"; }

# ------------------------------------------------------------------- preflight
log "Checking $HOST"
ssh_cmd 'command -v systemctl >/dev/null' || die "$HOST has no systemd"
TARGET_ARCH="$(ssh_cmd 'uname -m')"
LOCAL_ARCH="$(uname -m)"
log "Local arch $LOCAL_ARCH, target arch $TARGET_ARCH"

STAGE="$(ssh_cmd 'mktemp -d /tmp/sugie-deploy.XXXXXX')"
# Clean the staging directory even when the install fails.
trap 'ssh "${SSH_OPTS[@]}" "$HOST" "rm -rf $STAGE" >/dev/null 2>&1 || true' EXIT

# ---------------------------------------------------------------------- build
if [ -n "$BINARY" ]; then
    [ -f "$BINARY" ] || die "Binary not found: $BINARY"
    log "Shipping pre-built binary $BINARY"
elif [ "$BUILD_ON_TARGET" -eq 1 ]; then
    log "Building release binary on $HOST (this takes a while on a Pi)"
    # rustup installs cargo into ~/.cargo/bin, which a non-interactive SSH shell
    # does not have on PATH; a login shell finds it.
    ssh_cmd "bash -lc 'command -v cargo'" >/dev/null \
        || die "No Rust toolchain on $HOST. Install it with:
     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
    # `sugie-mark.svg` is embedded with include_str!, and the templates are too
    # (`sugie init` writes them out), so both are build inputs.
    tar czf - -C "$REPO_DIR" Cargo.toml Cargo.lock src templates sugie-mark.svg \
        | ssh_cmd "tar xzf - -C '$STAGE'"
    # Cap the job count: a 4 GB Pi OOMs on a default `cargo build --release`.
    ssh_cmd "bash -lc 'cd \"$STAGE\" && CARGO_BUILD_JOBS=2 cargo build --release'" \
        || die "Build failed on the target. On a fresh Pi the usual cause is the
     missing C toolchain or OpenSSL headers:
       sudo apt install build-essential pkg-config libssl-dev"
    ssh_cmd "mv -f '$STAGE/target/release/sugie' '$STAGE/sugie'"
    BINARY=""
else
    if [ "$LOCAL_ARCH" != "$TARGET_ARCH" ]; then
        die "Architecture mismatch ($LOCAL_ARCH != $TARGET_ARCH).
     Re-run with --build-on-target, or cross-compile and pass --binary."
    fi
    log "Building release binary locally"
    ( cd "$REPO_DIR" && cargo build --release )
    BINARY="$REPO_DIR/target/release/sugie"
fi

# ---------------------------------------------------------------------- stage
log "Uploading to $HOST:$STAGE"
if [ -n "$BINARY" ]; then
    tar czf - -C "$(dirname "$BINARY")" "$(basename "$BINARY")" \
        | ssh_cmd "tar xzf - -C '$STAGE'"
    # Normalise the name so install.sh finds it regardless of the source name.
    if [ "$(basename "$BINARY")" != "sugie" ]; then
        ssh_cmd "mv -f '$STAGE/$(basename "$BINARY")' '$STAGE/sugie'"
    fi
fi
tar czf - -C "$REPO_DIR" deploy/install.sh deploy/sugie.service templates \
    | ssh_cmd "tar xzf - -C '$STAGE'"

# -------------------------------------------------------------------- install
log "Running the installer on $HOST (sudo password may be requested)"
# -t so sudo can prompt; the installer prints a status summary at the end.
ssh -t "${SSH_OPTS[@]}" "$HOST" \
    "sudo bash '$STAGE/deploy/install.sh' \
        --binary '$STAGE/sugie' \
        --prefix '$PREFIX' \
        --user '$SERVICE_USER' \
        --unit '$UNIT_NAME' $START_ARGS"

log "Done. Follow the logs with: ssh $HOST journalctl -u $UNIT_NAME -f"
