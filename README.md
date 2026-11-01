# Sugie

Sugie is a GitHub App that automates routine repository maintenance. It handles
labeling, reviewer assignment, duplicate detection, stale lifecycle, DCO checks,
contributor greetings and periodic triage reports, and exposes a small web
console for configuration and manual commands. It is written in Rust.

[![Rust 1.89+](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org)
[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

---

## What it does

| Area | Behaviour |
| :--- | :--- |
| **GitHub App integration** | Standard GitHub App flow: RS256 JWT, cached installation tokens, HMAC-SHA256 webhook verification (`X-Hub-Signature-256`). |
| **Repository onboarding** | Listens to `installation` and `installation_repositories` events, so repositories added to an installation are picked up without a restart. |
| **Labeling** | Path globs with include/exclude patterns, conventional-commit prefixes, size bands (`size/XS` … `size/XL`) with configurable thresholds, draft handling, and title/body keyword or regex rules. |
| **Duplicate detection** | Tokenized n-gram Jaccard and Jaro-Winkler similarity, with an optional auto-close threshold. Comment templates support `{author}`, `{matched_number}`, `{similarity}` and `{matched_title}`. |
| **CODEOWNERS routing** | Parses `.github/CODEOWNERS` and optional TOML rules for component-specific reviewer pools. |
| **Stale lifecycle** | Three stages (warn, close, lock) with configurable intervals, target filters, exempt labels/authors/milestones and templated comments. |
| **First-time contributors** | Separate welcome templates and labels for first issues and first pull requests. |
| **Projects v2** | Adds items to organisation project boards through the GraphQL API, with separate project numbers for issues and pull requests. |
| **DCO validation** | Checks `Signed-off-by` trailers, publishes a `sugie/dco` commit status, and can exempt bot authors. |
| **CI summaries** | Aggregates check runs and suites into a single sticky summary comment per pull request. |
| **Reviewer assignment** | Workload-aware or round-robin distribution, per-component pools, `reviewers_per_pr`, availability exclusions and capacity limits. |
| **Contributor milestones** | Tracks thresholds in SQLite and notifies maintainers when a contributor reaches one. |
| **Reports** | Weekly and monthly digests: new/closed issues, close rate, time to first response, active contributors, lines changed and commit counts, posted to GitHub, Discord and Zulip. |
| **Chat integration** | Notifications and in-chat commands for Lark, Discord and Zulip. See [`docs/CHAT_BOTS_CONFIGURATION_GUIDE.md`](docs/CHAT_BOTS_CONFIGURATION_GUIDE.md). |
| **Slash commands** | Comment commands such as `/assign`, `/label`, `/close`, `/move`, `/duplicate` and `/help`, with a configurable prefix and aliases. |

---

## Notification events

Setup for each platform is documented in
[`docs/CHAT_BOTS_CONFIGURATION_GUIDE.md`](docs/CHAT_BOTS_CONFIGURATION_GUIDE.md).
Every event can be toggled in `sugie.toml`:

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

Discord supports a main webhook plus a separate reports channel:

```toml
[notifications.discord]
webhook_url = "https://discord.com/api/webhooks/.../alerts"
reports_webhook_url = "https://discord.com/api/webhooks/.../reports"
username = "sugie-bot"
```

Zulip uses one stream with separate topics:

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

## Slash commands

| Command | Alias | Example | Description |
| :--- | :---: | :--- | :--- |
| `/assign` | `/a` | `/assign @alice @bob` or `/a` | Assigns the given users, or the caller |
| `/unassign` | - | `/unassign @bob` | Removes an assignment |
| `/label` | `/l` | `/label +bug +area-core -wip` | Adds (`+`) or removes (`-`) labels |
| `/unlabel` | - | `/unlabel wip` | Removes a label |
| `/close` | `/c` | `/close completed` | Closes an issue or pull request with a reason |
| `/reopen` | - | `/reopen` | Reopens a closed item |
| `/retest` | `/r` | `/retest` | Re-runs failed check suites (pull requests only) |
| `/project` | `/p` | `/project 1 In Progress` | Adds the item to a Projects v2 board |
| `/move` | `/m` | `/move other-org/target-repo` | Copies the issue to another repository, transfers labels, references the author and closes the original |
| `/duplicate` | - | `/duplicate #123` | Closes the issue as a duplicate of #123 |
| `/title` | - | `/title Fix crash on Safari` | Renames the item (up to 256 characters) |
| `/milestone` | - | `/milestone v1.4` or `/milestone none` | Sets a milestone by title or `#number`; `none` clears it |
| `/lock` | - | `/lock too heated` | Locks the conversation (`off-topic`, `too heated`, `resolved`, `spam`) |
| `/unlock` | - | `/unlock` | Unlocks the conversation |
| `/help` | - | `/help` | Prints the command reference |

---

## Admin console

The bot serves a console at `/admin` on the same port as the webhook listener,
from the same process.

```bash
sugie serve --config ./sugie.toml
# http://localhost:8800/admin
```

On first start an `admin` account with the `owner` role is created. Unless
`SUGIE_ADMIN_PASSWORD` is set, its generated password is written to
`data/admin-initial-password.txt` (mode `0600`) and the file is deleted after the
first successful login.

| Screen | Purpose |
| :--- | :--- |
| **Dashboard** (`/admin`) | Per-repository issue, pull request, commit and merge counters; cache age; repository sync |
| **Repositories** (`/admin/repos`) | Onboarded repositories with counters and installation id |
| **Console** (`/admin/chat`) | Runs slash commands against a selected repository and streams output over SSE. Conversations are stored per user and reloaded after a restart, 20 turns per page. Running commands can be stopped. |
| **Chat Bots** (`/admin/chats`) | Every exchange with the bot from Zulip, Lark, Discord and the generic API, filterable by platform, sender, date, status and free text |
| **Configuration** (`/admin/config`) | Structured forms for `.env` and `sugie.toml`, raw TOML editing, validation before saving, timestamped backups and restore |
| **Users** (`/admin/users`) | Account management, roles, per-repository and per-command grants, global 2FA enforcement |
| **Audit** (`/admin/audit`) | Login, admin-action and command history with filters and CSV export |
| **Security** (`/admin/security`) | TOTP enrolment, WebAuthn passkeys, session revocation |

Console and chat history are kept in full by default. Both have an optional row
cap that an administrator can set on the relevant page.

### Roles

| Role | Run commands | Edit configuration | Manage users | Purge audit |
| :--- | :---: | :---: | :---: | :---: |
| `owner` | Yes | Yes | Yes, including other owners | Yes |
| `admin` | Yes | Yes | Yes, non-owner accounts | Yes |
| `maintainer` | Yes, granted repositories only | No | No | No |
| `viewer` | No | No | No | No |

Repository access and restricted commands are granted per account: an account
only sees the repositories it has been granted, and a command listed in
`restricted_commands` needs an explicit per-command grant.

### Security

- Argon2id password hashing; opaque 256-bit session tokens, stored hashed.
- Session cookies are `HttpOnly`, `SameSite=Strict`, `Secure`, and use the
  `__Host-` prefix when served over TLS.
- CSRF protection on state-changing requests (Origin / `Sec-Fetch-Site`
  validation), strict CSP, `X-Frame-Options: DENY`, HSTS.
- Login lockout with timing-equalised failures; optional TOTP and WebAuthn.
  Global 2FA enforcement is off on a fresh install so the first owner can sign in
  immediately; the dashboard keeps a warning visible until it is enabled.
- Configuration writes are validated, backed up and applied atomically
  (temporary file, fsync, rename) with `0600` permissions. Secrets are masked in
  the UI.
- Admin actions and commands are written to the audit log.
- `SUGIE_GITHUB_WEBHOOK_SECRET` is required. While it is empty, `/webhook`
  returns `503` and logs the reason, because HMAC would otherwise accept a
  zero-length key that anyone can compute.
- The Lark, Discord and Zulip inbound routes verify with their own credentials
  (`SUGIE_LARK_VERIFICATION_TOKEN` / `SUGIE_LARK_ENCRYPT_KEY`,
  `SUGIE_DISCORD_PUBLIC_KEY`, `SUGIE_ZULIP_OUTGOING_TOKEN`).
  `SUGIE_CHAT_API_TOKEN` does not cover them; each unset secret is named in a
  startup warning.
- `/status` reports a repository count only.
- `POST /api/chat/command` can run any chat command, including `/comment`,
  `/assign` and `/close`. Set `SUGIE_CHAT_API_TOKEN` to require a matching
  `X-Sugie-Chat-Token` header. When it is unset the endpoint stays open, as it
  has historically, and the service logs a warning at startup.

```bash
# Recommended: a long random shared secret
SUGIE_CHAT_API_TOKEN=$(openssl rand -hex 32)

# Every call from the bridge then has to carry it:
curl -X POST http://localhost:8800/api/chat/command \
  -H "X-Sugie-Chat-Token: $SUGIE_CHAT_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"command":"issues ensia 25 2","sender":"bridge"}'
```

Changing `.env` or `sugie.toml` from the console writes the file immediately,
but some values (tokens, ports, notification credentials) are only read at
startup; use **Restart service** on the configuration page to apply them.

The design notes are in [`docs/ADMIN_CONSOLE_PLAN.md`](docs/ADMIN_CONSOLE_PLAN.md)
and the security review in [`docs/REVIEW_2026-10.md`](docs/REVIEW_2026-10.md).

---

## Configuration

`sugie init` writes a commented `sugie.toml` template. Secrets are read from
`.env` (or the process environment) and merged at startup.

Global settings live in `sugie.toml`. Per-repository overrides go in
`.github/sugie.toml` on the repository's default branch, and can adjust
component reviewer pools, stale thresholds and size limits for that repository
only.

---

## Building and testing

`webauthn-rs` links OpenSSL, so a system OpenSSL 3 is required (`pkg-config` and
`libssl-dev` on Debian, `openssl@3` on macOS). To build a self-contained binary
instead, enable the vendored feature:

```bash
cargo build --release --features vendored-openssl
```

```bash
cargo test                          # unit and integration tests
cargo test --test integration_tests # integration tests only
```

---

## Documentation

- [`docs/CHAT_BOTS_CONFIGURATION_GUIDE.md`](docs/CHAT_BOTS_CONFIGURATION_GUIDE.md) — Lark, Discord and Zulip setup
- [`docs/ADMIN_CONSOLE_PLAN.md`](docs/ADMIN_CONSOLE_PLAN.md) — console design notes
- [`docs/REVIEW_2026-10.md`](docs/REVIEW_2026-10.md) — security review
- [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md) — installation and operations notes

---

## License

Dual-licensed under MIT or Apache 2.0.
