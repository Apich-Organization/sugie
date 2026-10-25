# 🦀 Sugie (杉枝)

> A powerful, production-ready GitHub triage bot engineered in Rust for open-source organizations and high-velocity engineering teams.

[![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

---

## 🌟 Key Features

| Capability | Description |
| :--- | :--- |
| 🔐 **GitHub App + Webhook Auth** | Standard GitHub App architecture with RS256 JWT generation, expiring installation token caching, and constant-time HMAC-SHA256 webhook signature verification (`X-Hub-Signature-256`). |
| 🏢 **Multi-Repo Dynamic Onboarding** | Listens to `installation_repositories` and `installation` events to automatically onboard new repositories added to an organization without restarting the server. |
| 🏷️ **TOML-Driven Labeling** | Path-based rules (`globset`), title/body keyword matching, regex rules, and issue template word detection. Supports organization-level defaults with repo-level `.github/sugie.toml` overrides. |
| 🔍 **Duplicate Issue/PR Detection** | Fast, tokenized n-gram Jaccard and string distance similarity algorithms. Flags duplicate issues above configurable thresholds with friendly links to existing issues. |
| 👥 **CODEOWNERS & Maintainer Routing** | Parses `.github/CODEOWNERS` and custom TOML maintainer rules. Automatically assigns maintainers or mentions them in comments based on changed files or keywords. |
| ⏳ **Tiered Stale Management** | 3-stage lifecycle engine: **Warn** after inactivity period → **Close** if inactive after warning → **Lock** conversation. Honors exempt labels (`pinned`, `security`, etc.). |
| 👋 **First-Time Contributor Greeting** | Detects first-time contributors (`author_association` or commit history), welcomes them with customizable onboarding messages, and adds triage tags (`first-time-contributor`). |
| 📋 **GitHub Projects v2 Board Integration** | Uses GitHub GraphQL API (`addProjectV2ItemById`) to automatically route new issues and PRs onto GitHub Project boards and set initial column status (e.g. `Triage`, `Backlog`). |
| ✍️ **DCO Commit Sign-off Validation** | Verifies that all commits in pull requests contain valid `Signed-off-by` trailers (`git commit -s`), updates commit status (`sugie/dco`), flags unsigned PRs, and provides fix instructions. |
| 🚦 **CI Results Sticky Summary** | Aggregates check runs and check suites. Posts and updates a single sticky summary table on PRs with status, failure summaries, and direct logs links. |
| ⚖️ **Reviewer Load Balancing** | Distributes PR review requests across team pools using workload tracking (`least_busy`), out-of-office exclusions, and active review capacity limits. |
| 🚀 **Contributor Growth Tracking** | Tracks contributor milestones (e.g., 3 merged PRs, 10 merged PRs) in persistent SQLite storage. Celebrates milestones and alerts maintainers to consider granting triage or committer rights. |
| 📊 **Weekly & Monthly Triage Reports** | Aggregates metrics: new vs closed issues, close rate, time-to-first-response (TTFR), active contributors, net code lines added/deleted, and commit counts. Formats markdown digests and posts to GitHub issues, Discord, and Zulip. |
| 💬 **Zulip & Discord Integrations** | Direct push notifications to Zulip streams/topics and Discord incoming webhooks with rich embeds for stale alerts, growth milestones, CI failures, and periodic reports. |
| ⚡ **Interactive Slash Commands** | Full command support in comments: `/assign`, `/unassign`, `/label`, `/unlabel`, `/close`, `/reopen`, `/retest`, `/project`, `/move` (cross-repo issue transfer), `/duplicate`, and `/help`. |

---

## 🚀 Quickstart

### 1. Create a GitHub App
1. Go to your GitHub Organization: **Settings → Developer Settings → GitHub Apps → New GitHub App**.
2. Set **Webhook URL** to your server: `https://your-domain.com/webhook` (or `/api/github/webhook`).
3. Set **Webhook Secret** (e.g. generate with `openssl rand -hex 20`).
4. Generate and download a **Private Key** (`.pem`).
5. Grant the following permissions:
   - **Repository Permissions**:
     - *Issues*: Read & Write
     - *Pull Requests*: Read & Write
     - *Commit Statuses*: Read & Write
     - *Checks*: Read & Write
     - *Contents*: Read (to fetch `.github/sugie.toml` and `CODEOWNERS`)
     - *Projects*: Read & Write
   - **Subscribe to events**:
     - `Issues`
     - `Issue comment`
     - `Pull request`
     - `Pull request review comment`
     - `Check suite`
     - `Check run`
     - `Installation`
     - `Installation repositories`
6. Install the App onto your organization's repositories.

---

### 2. Configure Sugie

Initialize configuration templates:
```bash
sugie init
```

This creates:
- `sugie.toml`: Global server and default triage configuration.
- `.env.example`: Environment variables template.
- `.github/sugie.toml`: Repository-specific override file.
- `.github/CODEOWNERS`: Example CODEOWNERS file.

Example `sugie.toml`:
```toml
host = "0.0.0.0"
port = 8080
db_path = "sugie.db"

[github]
app_id = 123456
webhook_secret = "your-github-webhook-secret"
private_key_path = "sugie.private-key.pem"

[notifications.discord]
webhook_url = "https://discord.com/api/webhooks/..."
notify_on_growth = true
notify_on_report = true

[notifications.zulip]
site_url = "https://your-zulip.org"
bot_email = "sugie-bot@your-zulip.org"
bot_api_key = "secret-api-key"
stream = "github-triage"
topic = "sugie-alerts"

[default_rules.labels]
enabled = true

[[default_rules.labels.path_rules]]
pattern = "crates/core/**"
labels = ["area-core"]

[[default_rules.labels.keyword_rules]]
keywords = ["crash", "panic", "segfault"]
target = "both"
labels = ["bug", "severity-high"]

[default_rules.duplicates]
enabled = true
threshold = 0.65
add_label = "possible-duplicate"

[default_rules.dco]
enabled = true
require_signoff = true
missing_label = "dco-missing"

[default_rules.reviewers]
enabled = true
pool = ["alice", "bob", "carol"]
max_active_reviews = 4
load_balancing = "least_busy"

[default_rules.growth]
enabled = true
[[default_rules.growth.thresholds]]
merged_prs = 3
message = "🎉 Contributor has reached 3 merged PRs! Consider inviting them to the Triage team."
```

---

### 3. Run Sugie

#### Using Cargo
```bash
# Validate configuration and credentials
sugie validate --config sugie.toml

# Start the webhook server and background schedulers
sugie serve --config sugie.toml
```

#### Using Docker Compose
```bash
docker compose up -d
```

---

## ⌨️ Slash Commands Cheat Sheet

Users and maintainers can trigger actions by commenting on Issues or PRs:

| Command | Example | Description |
| :--- | :--- | :--- |
| `/assign` | `/assign @alice @bob` or `/assign` | Assigns specified user(s) or self |
| `/unassign` | `/unassign @bob` or `/unassign` | Removes assignment |
| `/label` | `/label +bug +area-net -wip` | Adds (`+`) or removes (`-`) labels |
| `/unlabel` | `/unlabel wip` | Removes label |
| `/close` | `/close not planned` | Closes issue or PR with reason |
| `/reopen` | `/reopen` | Reopens closed issue or PR |
| `/retest` | `/retest` | Retriggers failed CI checks for PR |
| `/project` | `/project 1 In Progress` | Adds item to GitHub Project v2 board and sets status |
| `/move` | `/move other-org/target-repo` | **Cross-repo migration**: Copies issue to target repo, copies labels, links back, and closes original issue |
| `/duplicate` | `/duplicate #123` | Marks as duplicate of #123, adds label and closes issue |
| `/help` | `/help` | Displays interactive bot command help |

---

## 🛠️ CLI Operations

```bash
# Start webhook server
sugie serve --host 0.0.0.0 --port 8080

# Run one-off stale issue sweep
sugie stale --repo owner/repo

# Generate weekly report and print markdown
sugie report --repo owner/repo --weekly

# Generate monthly report and publish as GitHub Issue + broadcast to Discord/Zulip
sugie report --repo owner/repo --monthly --publish

# Validate configuration
sugie validate --config sugie.toml
```

---

## 🧪 Testing

Sugie includes full unit and integration test suites:

```bash
# Run all tests
cargo test

# Run integration tests
cargo test --test integration_tests
```

---

## 📄 License

Dual-licensed under MIT or Apache 2.0.
