# 🦀 Sugie (杉枝)

> A powerful, highly configurable GitHub triage bot engineered in Rust for open-source organizations, mono-repos, and modern engineering teams. Native **Podman** containerized deployment with deep **Discord** & **Zulip** event synchronization.

[![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
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
| 💬 **Lark, Discord & Zulip Two-Way Chat Sync** | Full bidirectional chat integration: rich notifications for reports and triage events, plus interactive in-chat bot commands (`/status`, `/workload`, `/assign`, etc.). See [Chat Bots Configuration Guide](file:///home/pana/dev2/sugie/docs/CHAT_BOTS_CONFIGURATION_GUIDE.md). |
| ⚡ **Interactive Slash Commands** | Commands in comments: `/assign`, `/unassign`, `/label`, `/unlabel`, `/close`, `/reopen`, `/retest`, `/project`, `/move` (cross-repo issue transfer), `/duplicate`, and `/help`. Supports custom command prefix and aliases (`/l` for `/label`, `/a` for `/assign`). |

---

## 💬 Lark, Discord & Zulip Notification Events

Detailed setup instructions for all three platforms are documented in [`docs/CHAT_BOTS_CONFIGURATION_GUIDE.md`](file:///home/pana/dev2/sugie/docs/CHAT_BOTS_CONFIGURATION_GUIDE.md).

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

## 🦭 Podman Deployment Guide (No Docker Required)

Sugie is containerized specifically for **Podman** environments, supporting rootless execution and SELinux volume relabeling (`:Z`).

### Method 1: Using `podman-run.sh` (Recommended)

An all-in-one management script is provided:

```bash
# Initialize configuration files
./target/release/sugie init --output-dir .

# Start the bot (automatically builds container image if needed)
./podman-run.sh start

# View real-time logs
./podman-run.sh logs

# Check container status
./podman-run.sh status

# Stop or restart
./podman-run.sh stop
./podman-run.sh restart
```

### Method 2: Manual Podman CLI

```bash
# 1. Build the container image using the native Containerfile
podman build -t sugie:latest -f Containerfile .

# 2. Run the rootless container with volume relabeling
podman run -d \
  --name sugie-bot \
  --restart always \
  -p 8080:8080 \
  -v $(pwd)/data:/app/data:Z \
  -v $(pwd)/sugie.toml:/app/sugie.toml:ro,Z \
  -e RUST_LOG=info \
  -e SUGIE_DB_PATH=/app/data/sugie.db \
  sugie:latest serve --config /app/sugie.toml
```

### Method 3: Podman Quadlet / Systemd Service

For production servers on Linux (RHEL, Fedora, Rocky, Debian, Ubuntu), install the provided `sugie.container` file to run as a native systemd unit:

```bash
# User-level service (rootless)
mkdir -p ~/.config/containers/systemd
cp sugie.container ~/.config/containers/systemd/

systemctl --user daemon-reload
systemctl --user enable --now sugie.service

# Check service status
systemctl --user status sugie.service
```

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
| `/help` | - | `/help` | Displays command usage manual |

---

## ⚙️ Configuration Reference

### Global Server Configuration (`sugie.toml`)
Run `sugie init` to generate a pre-configured template with exhaustive comments.

### Repository-Level Overrides (`.github/sugie.toml`)
Place `.github/sugie.toml` in your repository's default branch to customize behavior per repository (e.g. custom component reviewer pools, unique stale thresholds, repository-specific size limits).

---

## 🧪 Testing

```bash
# Run all unit and integration tests
cargo test

# Run integration tests only
cargo test --test integration_tests
```

---

## 📄 License

Dual-licensed under MIT or Apache 2.0.
