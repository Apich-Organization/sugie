# 🦀 Sugie (杉枝)

> A powerful, highly configurable GitHub triage bot engineered in Rust for open-source organizations, mono-repos, and modern engineering teams. Native **Podman** containerized deployment with deep **Discord** & **Zulip** event synchronization.

[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Podman Ready](https://img.shields.io/badge/container-podman%20native-purple.svg)](https://podman.io)
[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

---

## 🌟 Key Features & Deep Configurability

| Capability | Configurable Features & Logic |
| :--- | :--- |
| 🔐 **GitHub App + Webhook Auth** | Standard GitHub App architecture with RS256 JWT generation, expiring installation token caching, and constant-time HMAC-SHA256 webhook signature verification (`X-Hub-Signature-256`). |
| 🏢 **Multi-Repo Dynamic Onboarding** | Listens to `installation_repositories` and `installation` events to automatically onboard new repositories added to an organization without restarting the server. |
| 🏷️ **Advanced Labeling Engine** | • **Path Globs**: `globset` matching with positive and negative patterns (`ignore_patterns`).<br>• **Conventional Commits**: maps PR title prefixes (`feat:`, `fix:`, `docs:`) to labels.<br>• **Size-based Labeling**: calculates changed lines into `size/XS`, `size/S`, `size/M`, `size/L`, `size/XL` with custom thresholds.<br>• **Draft Handling**: automatically tags draft PRs with `work-in-progress` and removes it when marked ready.<br>• **Template Words & Regex**: targeted to title, body, or both. |
| 🔍 **Duplicate Issue/PR Detection** | • Tokenized n-gram Jaccard, Jaro-Winkler, and combined distance algorithms.<br>• **Auto-Close**: automatically close obvious duplicates when similarity exceeds `auto_close_threshold` (e.g. 92%).<br>• Fully customizable comment templates with `{author}`, `{matched_number}`, `{similarity}`, `{matched_title}`. |
| 👥 **CODEOWNERS & Component Routing** | Parses standard `.github/CODEOWNERS` and custom TOML rules. Supports component-specific reviewer pools and maintainer assignment. |
| ⏳ **Tiered Stale Lifecycle Engine** | • 3-stage lifecycle: **Warn** after `warn_days` → **Close** after `close_days` → **Lock** after `lock_days`.<br>• **Configurable Interval**: background check frequency in hours.<br>• **Target Filters**: selectively enable for issues, PRs, or both.<br>• Exempt labels (`pinned`, `security`), exempt authors (`dependabot`), and exempt milestones.<br>• Dynamic comment templates with runtime variable interpolation. |
| 👋 **First-Time Contributor Greeting** | Separate customizable welcome templates and labels for first-time Issues vs first-time PRs with `{author}` interpolation. |
| 📋 **GitHub Projects v2 Board Integration** | Uses GitHub GraphQL API (`addProjectV2ItemById` & `updateProjectV2ItemFieldValue`) to route items onto organization project boards with separate project numbers for Issues and PRs. |
| ✍️ **DCO Commit Sign-off Validation** | Verifies `Signed-off-by` trailers (`git commit -s`), updates commit status (`sugie/dco`), provides fix instructions, and allows bot author exemptions (`dependabot[bot]`, `renovate[bot]`). |
| 🚦 **CI Results Sticky Summary** | Aggregates check runs and check suites. Posts and updates a single sticky summary table on PRs with status, failure summaries, and direct logs links. |
| ⚖️ **Reviewer Load Balancing** | Distributes PR reviews using workload tracking (`least_busy` or `round_robin`), path-based component pools, configurable `reviewers_per_pr`, out-of-office exclusions, and capacity limits. |
| 🚀 **Contributor Growth Tracking** | Tracks contributor milestone thresholds in persistent SQLite storage. Celebrates milestones and alerts maintainers to consider granting triage or committer rights. |
| 📊 **Weekly & Monthly Triage Reports** | Aggregates metrics: new vs closed issues, close rate, time-to-first-response (TTFR), active contributors, net code lines added/deleted, and commit counts. Formats markdown digests and posts to GitHub issues, Discord, and Zulip on a configurable schedule. |
| 💬 **Lark, Discord & Zulip Two-Way Chat Sync** | Full bidirectional chat integration: rich notifications for reports and triage events, plus interactive in-chat bot commands (`/status`, `/workload`, `/assign`, etc.). See [Chat Bots Configuration Guide](docs/CHAT_BOTS_CONFIGURATION_GUIDE.md). |
| ⚡ **Interactive Slash Commands** | Commands in comments: `/assign`, `/unassign`, `/label`, `/unlabel`, `/close`, `/reopen`, `/retest`, `/project`, `/move` (cross-repo issue transfer), `/duplicate`, `/title`, `/milestone`, `/lock`, `/unlock`, and `/help`. Supports custom command prefix and aliases (`/l` for `/label`, `/a` for `/assign`). |

---

## 💬 Lark, Discord & Zulip Notification Events

Detailed setup instructions for all three platforms are documented in [`docs/CHAT_BOTS_CONFIGURATION_GUIDE.md`](docs/CHAT_BOTS_CONFIGURATION_GUIDE.md).

Every event can be toggled individually in `sugie.toml`:

```toml
[notifications.events]
weekly_report = true      # Scheduled weekly metrics report
monthly_report = true     # Scheduled monthly metrics report
assigned = true           # Assigning reviewers/maintainers
unassigned = true         # Removing assignment
issue_opened = true       # New issue opened
issue_closed = true       # Issue closed
pr_opened = true          # New pull request opened
pr_merged = true          # Pull request merged
label_changed = true      # Labels added/removed
stale_warned = true       # Item marked as stale
stale_closed = true       # Stale item closed
ci_failure = true         # CI checks failed
duplicate_detected = true # Potential duplicate found
dco_failed = true         # Commits missing sign-off
growth_milestone = true   # Contributor reached merged PR threshold
cross_repo_move = true    # Issue migrated across repos
```

### Discord Configuration
Supports a main webhook and a dedicated reports channel:
```toml
[notifications.discord]
webhook_url = "https://discord.com/api/webhooks/.../alerts"
reports_webhook_url = "https://discord.com/api/webhooks/.../reports"
username = "sugie-bot"
```

### Zulip Configuration
Supports dedicated stream topics for clear separation of concerns:
```toml
[notifications.zulip]
site_url = "https://your-zulip.example.com"
bot_email = "sugie-bot@your-zulip.example.com"
bot_api_key = "secret-key"
stream = "github-triage"
topic_triage = "triage"
topic_reports = "reports"
topic_alerts = "alerts"
topic_assigns = "assignments"
```

---

## 🚀 Deployment (systemd, Raspberry Pi friendly)

The recommended deployment is a release binary under a systemd unit, enabled so
it comes back after a reboot. One command from your workstation:

```bash
# Option A: Build on the Pi (remote release compilation)
./deploy/deploy.sh --host pi@192.168.1.50 --build-on-target

# Option B: Cross-compile on your workstation with `cross` and deploy
cross build --bin sugie --release --features vendored-openssl --target aarch64-unknown-linux-gnu
./deploy/deploy.sh --host pi@192.168.1.50 --binary target/aarch64-unknown-linux-gnu/release/sugie
```

That builds `--release` on the target (a Pi is a different CPU architecture),
installs into `/opt/sugie`, creates the `sugie` service account, installs
`/etc/systemd/system/sugie.service`, **enables** it for boot, starts it and waits
for `/health`. Re-running it upgrades in place and keeps the previous binary as
`sugie.prev` for rollback.

Then fill in the two config files (or use the admin console → Configuration):

```bash
sudo -e /opt/sugie/sugie.toml   # repositories, port, routing
sudo -e /opt/sugie/.env         # GitHub App id, webhook secret, private key
sudo systemctl restart sugie
```

Full guide — layout, upgrades, backups, TLS/passkeys, troubleshooting:
**[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)**.

```bash
systemctl status sugie          # state + recent logs
journalctl -u sugie -f          # follow logs
systemctl is-enabled sugie      # -> enabled (starts on reboot)
```

### Container alternative (Podman)

A container path is also supported for hosts that would rather not install a
Rust toolchain. Use the Quadlet unit so systemd owns the lifecycle — a plain
`podman run --restart always` does not come back reliably after a reboot:

```bash
# 1. config lives on the host, not in the image
sudo install -d -m 0755 /opt/sugie/data
sudo cp templates/sugie.toml /opt/sugie/sugie.toml
sudo cp templates/.env.example /opt/sugie/.env && sudo chmod 600 /opt/sugie/.env

# 2. build as root: a rootless build is invisible to a system-wide service
sudo podman build -t localhost/sugie:latest -f Containerfile .

# 3. the image runs as uid 1000, so the mounts must belong to it
sudo chown -R 1000:1000 /opt/sugie

# 4. Quadlet unit: generated by systemd, started here, enabled by [Install]
sudo cp sugie.container /etc/containers/systemd/sugie.container
sudo systemctl daemon-reload && sudo systemctl start sugie.service
systemctl is-enabled sugie.service    # -> generated
```

Edit `/opt/sugie/sugie.toml` and `/opt/sugie/.env` before starting, and keep
`PublishPort` in the unit equal to `port` in `sugie.toml` (8800).
`podman-run.sh` wraps the same image for local development
(`start|stop|restart|logs|status|build`). Details and the rootless variant:
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#8-container-alternative-podman).

---

## ⌨️ Slash Commands Cheat Sheet

| Command | Alias | Example | Description |
| :--- | :---: | :--- | :--- |
| `/assign` | `/a` | `/assign @alice @bob` or `/a` | Assigns specified user(s) or self |
| `/unassign` | - | `/unassign @bob` | Removes assignment |
| `/label` | `/l` | `/label +bug +area-core -wip` | Adds (`+`) or removes (`-`) labels |
| `/unlabel` | - | `/unlabel wip` | Removes a label |
| `/close` | `/c` | `/close completed` | Closes issue or PR with reason |
| `/reopen` | - | `/reopen` | Reopens closed issue or PR |
| `/retest` | `/r` | `/retest` | Retriggers failed CI check suites (PRs only) |
| `/project` | `/p` | `/project 1 In Progress` | Adds item to GitHub Project v2 board |
| `/move` | `/m` | `/move other-org/target-repo` | **Cross-repo migration**: Copies issue, transfers labels, references author, and closes original |
| `/duplicate`| - | `/duplicate #123` | Closes issue as duplicate of #123 |
| `/title` | - | `/title Fix crash on Safari` | Renames the issue or PR (max 256 chars) |
| `/milestone` | - | `/milestone v1.4` or `/milestone none` | Sets a milestone by title or `#num`; `none` clears it |
| `/lock` | - | `/lock too heated` | Locks the conversation (`off-topic`, `too heated`, `resolved`, `spam`) |
| `/unlock` | - | `/unlock` | Unlocks the conversation |
| `/help` | - | `/help` | Displays command usage manual |

---

## 🖥️ Web Admin Console

The bot serves a self-hosted admin console at **`/admin`** on the same port as
the webhook listener — no extra process, no separate build pipeline, no SSH.

```bash
sugie serve --config ./sugie.toml
# → http://localhost:8800/admin
```

On first start a bootstrap `owner` account is created and its generated password
is written to `data/admin-initial-password.txt` (mode `0600`). The file is
deleted as soon as the first session is minted.

| Screen | What it does |
| :--- | :--- |
| **Dashboard** (`/admin`) | Per-repository issue / PR / commit / merged counters, cache age, repository sync |
| **Repositories** (`/admin/repos`) | Every onboarded repository with its live counters and installation id |
| **Console** (`/admin/chat`) | Pick a repository, type a slash command, watch the output stream over SSE; the whole conversation (command + reply) is persisted per user and reloaded after a restart, 20 turns per page with **Load older**; **Stop** cancels a running command; long lists page with `/issues 50 2` and a **Show more** button; history is kept in full by default, with an optional per-user cap an admin can set |
| **Chat Bots** (`/admin/chats`) | Every exchange with the bot from Zulip / Lark / Discord / the generic API — command, reply, platform, sender, channel, latency — filterable by platform, sender, date, status and free text; history is kept in full by default, with an optional row cap an admin can set |
| **Configuration** (`/admin/config`) | Structured forms for `.env` and `sugie.toml` plus raw TOML editing, validation before save, automatic timestamped backups and one-click restore |
| **Users** (`/admin/users`) | Create / edit / disable accounts, assign roles, per-repository and per-command grants, global 2FA enforcement |
| **Audit** (`/admin/audit`) | Login, admin-action and command history with filters and CSV export |
| **Security** (`/admin/security`) | TOTP enrolment (QR), WebAuthn passkeys, session revocation |

### Permissions

| Role | Execute commands | Edit config | Manage users | Purge audit |
| :--- | :---: | :---: | :---: | :---: |
| `owner` | ✅ | ✅ | ✅ (incl. other owners) | ✅ |
| `admin` | ✅ | ✅ | ✅ (non-owner accounts) | ✅ |
| `maintainer` | ✅ (granted repos only) | — | — | — |
| `viewer` | — | — | — | — |

Repositories and *restricted* commands are additionally gated per user: an
account only sees and acts on the repositories it has been granted, and a
command marked `restricted_commands` in `sugie.toml` needs an explicit
per-command grant.

### Security model

- Argon2id password hashing, opaque 256-bit session tokens stored hashed.
- Session cookies are `HttpOnly`, `SameSite=Strict`, `Secure`, and use the
  `__Host-` prefix when served over TLS.
- CSRF protection on every state-changing request (Origin / `Sec-Fetch-Site`
  validation), strict CSP, `X-Frame-Options: DENY`, HSTS.
- Login lockout with timing-equalised failures; optional TOTP and WebAuthn.
  Global 2FA enforcement is **off on a fresh install** so the first owner can use
  the console immediately; the dashboard shows a warning until it is enabled on
  the Users page.
- Config writes are validated, backed up, and applied atomically
  (tmp + fsync + rename) with `0600` permissions; secrets are masked in the UI.
- Every admin action and command is written to the audit log.
- `SUGIE_GITHUB_WEBHOOK_SECRET` is **required**: while it is empty, `/webhook`
  answers `503` and logs the reason, because HMAC would otherwise accept a
  zero-length key that anyone can compute. Set it to the GitHub App's webhook
  secret.
- The Lark, Discord and Zulip inbound routes verify with their own credentials
  (`SUGIE_LARK_VERIFICATION_TOKEN` / `SUGIE_LARK_ENCRYPT_KEY`,
  `SUGIE_DISCORD_PUBLIC_KEY`, `SUGIE_ZULIP_OUTGOING_TOKEN`). `SUGIE_CHAT_API_TOKEN`
  does not cover them; each unset secret is named in a startup warning.
- `/status` reports a repository **count** only — the inventory is available to
  signed-in console users who are allowed to see it.
- The generic chat bridge endpoint `POST /api/chat/command` can run `/comment`,
  `/assign`, `/close` and the rest of the chat commands. Set
  `SUGIE_CHAT_API_TOKEN` to require the matching `X-Sugie-Chat-Token` header;
  when it is unset the endpoint stays open (historical behaviour) and the
  service logs a warning at startup.

```bash
# Recommended: a long random shared secret
SUGIE_CHAT_API_TOKEN=$(openssl rand -hex 32)

# Then every call from the bridge must carry it:
curl -X POST http://localhost:8800/api/chat/command \
  -H "X-Sugie-Chat-Token: $SUGIE_CHAT_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"command":"issues ensia 25 2","sender":"bridge"}'
```

See [`docs/ADMIN_CONSOLE_PLAN.md`](docs/ADMIN_CONSOLE_PLAN.md) for the design and
[`docs/REVIEW_2026-10.md`](docs/REVIEW_2026-10.md) for the security review.

> Changing `.env` or `sugie.toml` from the console writes the file immediately,
> but some values (tokens, ports, notification credentials) are only read at
> startup — use **Restart service** on the config page to apply them.

---

## ⚙️ Configuration Reference

### Global Server Configuration (`sugie.toml`)
Run `sugie init` to generate a pre-configured template with exhaustive comments.

### Repository-Level Overrides (`.github/sugie.toml`)
Place `.github/sugie.toml` in your repository's default branch to customize behavior per repository (e.g. custom component reviewer pools, unique stale thresholds, repository-specific size limits).

---

## 🧪 Building & Testing

`webauthn-rs` links OpenSSL, so a system OpenSSL 3 (`pkg-config` + `libssl-dev`
on Debian, `openssl@3` on macOS) is required. To build a self-contained binary
without it, enable the vendored feature:

```bash
cargo build --release --features vendored-openssl
```

### Cross-compilation (e.g. Raspberry Pi / 64-bit ARM)

To cross-compile for Raspberry Pi (`aarch64-unknown-linux-gnu`) on an x86_64 host using [`cross`](https://github.com/cross-rs/cross), use `--features vendored-openssl` to build OpenSSL from source within the container:

```bash
cross build --bin sugie --release --features vendored-openssl --target aarch64-unknown-linux-gnu
```

The output binary will be located at `target/aarch64-unknown-linux-gnu/release/sugie`.

### Running Tests

```bash
# Run all unit and integration tests
cargo test

# Run integration tests only
cargo test --test integration_tests
```

---

## 📄 License

Dual-licensed under MIT or Apache 2.0.
