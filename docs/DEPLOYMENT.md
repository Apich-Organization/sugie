# Deployment

Sugie is a single static-ish binary plus two config files and one SQLite
database. This guide covers a systemd host — a Raspberry Pi (4/5, 64-bit OS) is
the reference target — and the container alternative at the end.

Everything below assumes:

| Item | Value |
| --- | --- |
| Install directory | `/opt/sugie` |
| Service account | `sugie` (system account, no login shell) |
| systemd unit | `sugie.service` (enabled at boot) |
| Config | `/opt/sugie/sugie.toml` + `/opt/sugie/.env` |
| State | `/opt/sugie/data/` (database, config backups, security log) |

The unit keeps `WorkingDirectory=/opt/sugie`, which is what makes the relative
paths in `sugie.toml` (`db_path = "data/sugie.db"`) and the console's `.env`
editing work exactly as they do in development.

---

## 1. Quick start

From your workstation, with the repo checked out:

```bash
# Raspberry Pi: build there (different CPU architecture from your laptop)
./deploy/deploy.sh --host pi@192.168.1.50 --build-on-target

# Non-standard SSH port (e.g. a Pi behind a forwarded 2222)
./deploy/deploy.sh --host pi@192.168.1.50 --port 2222 --build-on-target
```

Building on the Pi needs a toolchain and OpenSSL headers, once:

```bash
sudo apt install build-essential pkg-config libssl-dev curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
```

(No Rust on the Pi? Build elsewhere and ship the binary with `--binary`, or use
the container path in section 8.)

`deploy.sh` copies the sources to a temp directory on the Pi, builds
`--release`, then runs `deploy/install.sh` under sudo. That script creates the
service account, lays out `/opt/sugie`, installs and **enables** the unit, starts
it and waits for `/health`.

First run leaves you with two files that need editing:

```bash
ssh pi@192.168.1.50
sudo -e /opt/sugie/sugie.toml   # repositories, port, notification routing
sudo -e /opt/sugie/.env         # GitHub App id, webhook secret, private key path
sudo systemctl restart sugie
```

Or edit both from the admin console (`http://<pi>:8800/admin` → Configuration),
which writes the same files and restarts on demand.

### Building somewhere else

```bash
# same architecture (x86_64 → x86_64)
./deploy/deploy.sh --host user@host

# pre-built aarch64 binary from a cross toolchain or CI
./deploy/deploy.sh --host pi@192.168.1.50 --binary ./sugie-aarch64
```

`deploy.sh` refuses to ship a binary whose architecture does not match the
target, because the failure mode otherwise is an `Exec format error` from
systemd with very little to go on.

---

## 2. What the installer does

`deploy/install.sh` is idempotent — run it again to upgrade. In order:

1. Creates the `sugie` system account if missing.
2. Creates `/opt/sugie` and `/opt/sugie/data` (mode 0750).
3. Backs up the current binary to `sugie.prev`, installs the new one atomically
   (`install` to a temp name, then `mv`).
4. Seeds `sugie.toml` / `.env` from `templates/` **only when missing** — an
   existing configuration is never overwritten.
5. Fixes ownership (`sugie:sugie`) and modes (`.env` 0600, `sugie.toml` 0640).
6. Writes `/etc/systemd/system/sugie.service`, `daemon-reload`, `enable`
   (this is the bit that survives a reboot), then `restart`.
7. Polls `http://127.0.0.1:<port>/health` and prints the unit status.

Options: `--prefix`, `--user`, `--unit`, `--binary`, `--port`, `--no-start`.

---

## 3. Reboot behaviour

`systemctl enable` creates the `multi-user.target.wants` symlink, so the service
starts on every boot without anyone logging in. Verify:

```bash
systemctl is-enabled sugie     # -> enabled
sudo reboot
# after it comes back:
systemctl is-active sugie      # -> active
journalctl -u sugie -b --no-pager | tail -20
```

The unit waits for `network-online.target`. On a Pi with a slow DHCP lease this
matters: without it the first GitHub call after boot can fail before the
interface has an address.

If the service is stuck in `activating` or dies immediately, the reason is
almost always one of: missing `.env` values, an unreadable private key, or a
port already in use — `journalctl -u sugie -n 50` says which.

---

## 4. Day-to-day operations

```bash
systemctl status sugie              # state + last log lines
journalctl -u sugie -f              # follow logs
journalctl -u sugie --since today   # today's logs
sudo systemctl restart sugie        # apply config changes made by hand
sudo systemctl stop sugie           # stop (does not disable boot start)
```

Logs go to the journal only (`SyslogIdentifier=sugie`); the service does not
write its own rotating log file, so `journalctl` is the single place to look.

---

## 5. Upgrades and rollback

```bash
./deploy/deploy.sh --host pi@192.168.1.50 --build-on-target
```

The installer keeps the previous binary:

```bash
# roll back to the build that was running before the last deploy
sudo cp /opt/sugie/sugie.prev /opt/sugie/sugie
sudo systemctl restart sugie
```

Database migrations are additive (`CREATE TABLE IF NOT EXISTS` + idempotent
`ALTER TABLE`), so an older binary still starts against a newer database. If you
downgrade across a release that *removed* a column, restore the backup instead.

---

## 6. Backups

All state is one SQLite file:

```
/opt/sugie/data/sugie.db        (+ sugie.db-wal, sugie.db-shm)
```

Copying the `.db` while the service runs can miss the WAL. Either stop the
service, or use SQLite's own backup:

```bash
sudo -u sugie sqlite3 /opt/sugie/data/sugie.db ".backup '/tmp/sugie-backup.db'"
```

Config backups taken by the admin console live in
`/opt/sugie/data/config-backups/`. The bootstrap admin password (first start
only) is written to `/opt/sugie/data/admin-initial-password.txt` and deleted on
the first successful login.

---

## 7. Network, TLS and passkeys

The bot listens on `host`/`port` from `sugie.toml` (default `0.0.0.0:8800`).

- **GitHub webhook**: point the App's webhook URL at `https://<public-host>/webhook`.
  GitHub requires HTTPS for webhooks, so a Pi on a LAN needs a tunnel or a
  reverse proxy with a certificate.
- **Firewall**: only the port you actually publish needs to be reachable.
  `sudo ufw allow 8800/tcp` for LAN-only access.

Passkeys (WebAuthn) only work in a secure context — HTTPS, or `localhost`.
Behind a proxy, tell Sugie what users actually type:

```ini
# /opt/sugie/.env
SUGIE_WEBAUTHN_RP_ID=pi.example.com
SUGIE_WEBAUTHN_RP_ORIGIN=https://pi.example.com
SUGIE_ADMIN_SECURE_COOKIE=1
```

`RP_ID` is a bare hostname (no scheme, no port). If it does not match the origin
users visit, registration fails with a browser-side error and nothing useful in
the logs. `SUGIE_ADMIN_SECURE_COOKIE=1` is only needed when the proxy does not
send `X-Forwarded-Proto: https`.

Minimal Caddy reverse proxy on the Pi:

```caddyfile
pi.example.com {
    reverse_proxy 127.0.0.1:8800
}
```

TOTP (`/admin/security`) works over plain HTTP and is the fallback when you do
not have TLS.

---

## 8. Container alternative (Podman)

Use this when you would rather not install a Rust toolchain on the Pi. The
container is a multi-stage build: `rust:1.99-bookworm` compiles, and the result
is copied into `debian:bookworm-slim` (127 MB image; the build needs ~2 GB of
disk and a few minutes on a Pi).

Four things bite people here, and the steps below handle all of them:

- a **rootful** service cannot see an image built by your login user;
- the image runs as uid 1000, so the mounted directory must be writable by it;
- the port inside the container is whatever `sugie.toml` says (8800);
- Quadlet units are *generated* — you `start` them, you never `enable` them.

### 8.1 Prepare the install directory

```bash
sudo install -d -m 0755 /opt/sugie /opt/sugie/data
sudo cp templates/sugie.toml   /opt/sugie/sugie.toml
sudo cp templates/.env.example /opt/sugie/.env
sudo chmod 600 /opt/sugie/.env
sudo -e /opt/sugie/sugie.toml   # repositories, port
sudo -e /opt/sugie/.env         # GitHub App id, webhook secret, private key
```

### 8.2 Build the image as root

```bash
sudo podman build -t localhost/sugie:latest -f Containerfile .
```

`sudo` matters. A rootless build stores the image under
`~/.local/share/containers`, which the system-wide service (running as root)
cannot see — it fails with "image not found" even though `podman images` shows
it in your shell.

### 8.3 Give the mounted directory to the container's uid

```bash
sudo chown -R 1000:1000 /opt/sugie
```

The image ends with `USER sugie` (uid 1000, created in the Containerfile), and a
rootful container's uid 1000 *is* the host's uid 1000. Without this the bot
starts and immediately fails to open `data/sugie.db`, and the admin console
cannot rewrite `.env`.

### 8.4 Install and start the Quadlet unit

```bash
sudo cp sugie.container /etc/containers/systemd/sugie.container
sudo systemctl daemon-reload
sudo systemctl start sugie.service
systemctl is-enabled sugie.service    # -> generated
systemctl status sugie.service
journalctl -u sugie -f
```

The unit is generated from the `.container` file at boot by Quadlet, so there is
no `enable` step: the `[Install] WantedBy=multi-user.target` in the file is what
brings it up after a reboot. Verify with `sudo reboot` and `systemctl is-active sugie`.

### 8.5 Upgrading

```bash
git pull
sudo podman build -t localhost/sugie:latest -f Containerfile .
sudo systemctl restart sugie.service
```

The database and config live in `/opt/sugie` on the host, so they survive image
rebuilds. Reclaim the build cache when disk is tight (`podman build` leaves
several GB behind on a Pi):

```bash
sudo podman system prune -f
sudo podman rmi rust:1.99-bookworm    # ~1.4 GB, only needed to build again
```

### Rootless instead of system-wide

If you prefer running everything as your own user, put the unit in
`~/.config/containers/systemd/`, use `systemctl --user`, and enable lingering so
the unit starts without a login:

```bash
loginctl enable-linger "$USER"
systemctl --user daemon-reload && systemctl --user start sugie.service
```

Rootless containers see your user's image storage, so build without `sudo`, and
`chown -R "$(id -u):$(id -g)" /opt/sugie` instead of uid 1000.

### Manual runs (development)

`podman-run.sh` wraps the same image for a quick local loop
(`start|stop|restart|logs|status|build`). It mounts `sugie.toml`, `.env` and
`data/` from the current directory, derives the published port from
`sugie.toml`, and runs with `--userns=keep-id:uid=1000` so the container user
matches yours. It is not a substitute for the Quadlet unit on a server that
reboots unattended.

---

## 9. Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| `status=203/EXEC` | Binary not executable or wrong architecture — check `uname -m` matches the build. |
| `status=1/FAILURE` at once, log says "Failed to read .env" | `.env` missing or unreadable by `sugie`; `ls -l /opt/sugie/.env` should be `-rw------- sugie sugie`. |
| Console shows "Admin console bootstrapped…" password warning every boot | A fresh database is being created each start — `db_path` points somewhere unwritable. |
| `/health` 200 but GitHub deliveries get 503 | `SUGIE_GITHUB_WEBHOOK_SECRET` is empty; deliveries are refused on purpose. |
| Passkey registration fails in the browser | `SUGIE_WEBAUTHN_RP_ID` / `RP_ORIGIN` do not match the URL, or the page is not HTTPS. |
| Service starts, then restarts in a loop | Port already taken (`ss -ltnp | grep 8800`), or the DB file is owned by another user. |
| `cargo build --release` killed on the Pi | Out of memory — `CARGO_BUILD_JOBS=2` (what `deploy.sh` uses) or add swap. |
