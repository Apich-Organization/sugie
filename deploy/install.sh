#!/usr/bin/env bash
# Install (or upgrade) Sugie on a systemd host. Run as root on the target.
#
#   sudo ./deploy/install.sh --binary ./sugie
#
# Idempotent: re-running it upgrades the binary in place, keeps the existing
# `sugie.toml` and `.env`, and restarts the service.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PREFIX="/opt/sugie"
SERVICE_USER="sugie"
UNIT_NAME="sugie"
UNIT_DIR="${SUGIE_UNIT_DIR:-/etc/systemd/system}"
BINARY_SRC=""
APP_PORT=""
START=1

usage() {
    cat <<'EOF'
Usage: sudo ./install.sh [options]

  --binary PATH   Binary to install (default: ./sugie next to this script)
  --prefix DIR    Install directory (default: /opt/sugie)
  --user NAME     Service account (default: sugie)
  --unit NAME     systemd unit name (default: sugie)
  --port N        Set `port` in sugie.toml (and use it for the health check)
  --no-start      Install files and enable the unit, but do not start it now
  -h, --help      Show this help

Environment overrides (advanced): SUGIE_UNIT_DIR (default /etc/systemd/system).

Layout created under --prefix:

  sugie            the binary
  sugie.toml       bot configuration (never overwritten once it exists)
  .env             secrets, mode 0600 (never overwritten once it exists)
  data/            SQLite database, config backups, security log
EOF
}

log()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[!]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[x]\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --binary)   BINARY_SRC="${2:?--binary needs a path}"; shift 2 ;;
        --prefix)   PREFIX="${2:?--prefix needs a path}"; shift 2 ;;
        --user)     SERVICE_USER="${2:?--user needs a name}"; shift 2 ;;
        --unit)     UNIT_NAME="${2:?--unit needs a name}"; shift 2 ;;
        --port)     APP_PORT="${2:?--port needs a number}"; shift 2 ;;
        --no-start) START=0; shift ;;
        -h|--help)  usage; exit 0 ;;
        *)          die "Unknown option: $1 (try --help)" ;;
    esac
done

[ "$(id -u)" -eq 0 ] || die "Must run as root: sudo $0 $*"
command -v systemctl >/dev/null || die "systemd not found; this installer targets systemd hosts"

[ -n "$BINARY_SRC" ] || BINARY_SRC="$SCRIPT_DIR/sugie"
[ -f "$BINARY_SRC" ] || die "Binary not found: $BINARY_SRC (build it or pass --binary)"
[ -f "$SCRIPT_DIR/sugie.service" ] || die "Unit template not found: $SCRIPT_DIR/sugie.service"

# ---------------------------------------------------------------- service user
if id "$SERVICE_USER" >/dev/null 2>&1; then
    log "Service account '$SERVICE_USER' already exists"
else
    log "Creating system account '$SERVICE_USER'"
    useradd --system --home-dir "$PREFIX" --shell /usr/sbin/nologin \
        --user-group "$SERVICE_USER"
fi

# --------------------------------------------------------------------- layout
log "Installing into $PREFIX"
install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$PREFIX"
install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$PREFIX/data"

# Replace the binary atomically: a running service keeps its inode, and a failed
# copy cannot leave a half-written executable behind. The previous build is kept
# so a bad upgrade is one `cp` away from being rolled back.
if [ -f "$PREFIX/sugie" ]; then
    cp -f "$PREFIX/sugie" "$PREFIX/sugie.prev"
fi
install -m 0755 -o root -g root "$BINARY_SRC" "$PREFIX/.sugie.new"
mv -f "$PREFIX/.sugie.new" "$PREFIX/sugie"

# --------------------------------------------------------------------- config
TEMPLATES="$SCRIPT_DIR/templates"
[ -d "$TEMPLATES" ] || TEMPLATES="$SCRIPT_DIR/../templates"

if [ -f "$PREFIX/sugie.toml" ]; then
    log "Keeping existing sugie.toml"
else
    if [ -f "$TEMPLATES/sugie.toml" ]; then
        install -m 0640 -o "$SERVICE_USER" -g "$SERVICE_USER" \
            "$TEMPLATES/sugie.toml" "$PREFIX/sugie.toml"
        warn "Installed the example sugie.toml — edit $PREFIX/sugie.toml (repositories, port) before going live."
    else
        die "No sugie.toml and no template to copy. Create $PREFIX/sugie.toml first."
    fi
fi

if [ -f "$PREFIX/.env" ]; then
    log "Keeping existing .env"
else
    if [ -f "$TEMPLATES/.env.example" ]; then
        install -m 0600 -o "$SERVICE_USER" -g "$SERVICE_USER" \
            "$TEMPLATES/.env.example" "$PREFIX/.env"
        warn "Installed the example .env — fill in SUGIE_GITHUB_APP_ID, SUGIE_GITHUB_WEBHOOK_SECRET and the private key path in $PREFIX/.env."
    else
        die "No .env and no template to copy. Create $PREFIX/.env first (mode 0600)."
    fi
fi

# `--port` is explicit, so it wins over whatever the file already says. Done
# before the chown below: `sed -i` replaces the file and would otherwise leave it
# owned by root.
if [ -n "$APP_PORT" ]; then
    case "$APP_PORT" in
        '' | *[!0-9]*) die "--port must be a whole number, got '$APP_PORT'" ;;
    esac
    sed -i "s/^[[:space:]]*port[[:space:]]*=.*/port = $APP_PORT/" "$PREFIX/sugie.toml"
    if grep -qx "port = $APP_PORT" "$PREFIX/sugie.toml"; then
        log "Set port = $APP_PORT in $PREFIX/sugie.toml"
    else
        warn "sugie.toml has no top-level 'port = …' line — set the port by hand."
    fi
fi

# The console edits both files, so the service account owns them; secrets stay
# unreadable to everyone else.
chown -R "$SERVICE_USER:$SERVICE_USER" "$PREFIX"
chmod 0750 "$PREFIX"
chmod 0600 "$PREFIX/.env"
chmod 0640 "$PREFIX/sugie.toml"

# The service owns the directory but must not be able to rewrite its own
# executable: the recursive chown above would otherwise hand it that ability.
chown root:root "$PREFIX/sugie"
if [ -f "$PREFIX/sugie.prev" ]; then
    chown root:root "$PREFIX/sugie.prev"
fi
chmod 0755 "$PREFIX/sugie"

# ----------------------------------------------------------------------- unit
UNIT_PATH="$UNIT_DIR/$UNIT_NAME.service"
log "Writing $UNIT_PATH"
install -d -m 0755 "$UNIT_DIR"
sed -e "s|^User=sugie$|User=$SERVICE_USER|" \
    -e "s|^Group=sugie$|Group=$SERVICE_USER|" \
    -e "s|/opt/sugie|$PREFIX|g" \
    "$SCRIPT_DIR/sugie.service" > "$UNIT_PATH"
chmod 0644 "$UNIT_PATH"

systemctl daemon-reload
# `enable` writes the symlink that makes the unit come back after a reboot.
systemctl enable "$UNIT_NAME.service" >/dev/null
log "Enabled $UNIT_NAME.service for boot"

if [ "$START" -eq 1 ]; then
    log "Starting $UNIT_NAME.service"
    # restart works for both first install and upgrade
    systemctl restart "$UNIT_NAME.service"
else
    log "Skipping start (--no-start); run: systemctl start $UNIT_NAME.service"
fi

# ---------------------------------------------------------------- health check
PORT="$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9]\+\).*/\1/p' "$PREFIX/sugie.toml" | head -1)"
PORT="${PORT:-8800}"

if [ "$START" -eq 1 ]; then
    if ! command -v curl >/dev/null; then
        log "curl not installed; skipping the health check"
    else
        printf 'Waiting for http://127.0.0.1:%s/health ' "$PORT"
        healthy=0
        for _ in $(seq 1 30); do
            if curl -fsS -m 2 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
                healthy=1
                echo " ok"
                break
            fi
            # Stop waiting the moment the unit gives up: 30 s of dots tells the
            # operator less than the reason it died.
            if ! systemctl is-active --quiet "$UNIT_NAME.service"; then
                echo " unit is not running"
                break
            fi
            printf '.'
            sleep 1
        done
        if [ "$healthy" -eq 0 ]; then
            warn "Health check did not pass — inspect: journalctl -u $UNIT_NAME -n 50"
        fi
    fi
fi

echo
log "Status"
systemctl --no-pager --lines=0 status "$UNIT_NAME.service" || true
echo
printf '  enabled: %s\n' "$(systemctl is-enabled "$UNIT_NAME.service" 2>/dev/null || echo unknown)"
printf '  active : %s\n' "$(systemctl is-active "$UNIT_NAME.service" 2>/dev/null || echo unknown)"
echo
echo "  Logs      : journalctl -u $UNIT_NAME -f"
echo "  Restart   : systemctl restart $UNIT_NAME"
echo "  Config    : $PREFIX/sugie.toml and $PREFIX/.env (or the admin console)"
echo "  Console   : http://<host>:$PORT/admin"
