//! Role model and the two-level (repository + command) access checks used by
//! every mutating admin action.

use crate::web::db::UserRow;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Role {
    Viewer,
    Maintainer,
    Admin,
    Owner,
}

impl Role {
    pub fn parse(raw: &str) -> Role {
        match raw.trim().to_ascii_lowercase().as_str() {
            "owner" => Role::Owner,
            "admin" => Role::Admin,
            "maintainer" => Role::Maintainer,
            _ => Role::Viewer,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Maintainer => "maintainer",
            Role::Viewer => "viewer",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Role::Owner => "Owner",
            Role::Admin => "Admin",
            Role::Maintainer => "Maintainer",
            Role::Viewer => "Viewer",
        }
    }
}

/// A logged-in principal with its resolved grants.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub repos: Vec<String>,
    pub commands: Vec<String>,
    pub totp_enabled: bool,
    pub two_fa_policy: String,
    pub passkey_count: i64,
    pub needs_2fa_setup: bool,
}

impl CurrentUser {
    pub fn from_row(
        row: UserRow,
        repos: Vec<String>,
        commands: Vec<String>,
        global_2fa: bool,
    ) -> Self {
        let needs_2fa_setup = row.needs_2fa_setup(global_2fa);
        Self {
            id: row.id,
            display_name: if row.display_name.is_empty() {
                row.username.clone()
            } else {
                row.display_name
            },
            username: row.username,
            role: Role::parse(&row.role),
            repos,
            commands,
            totp_enabled: row.totp_enabled,
            two_fa_policy: row.two_fa_policy,
            passkey_count: row.passkey_count,
            needs_2fa_setup,
        }
    }

    pub fn is_admin(&self) -> bool {
        self.role >= Role::Admin
    }

    pub fn is_owner(&self) -> bool {
        self.role == Role::Owner
    }

    /// Can edit `.env` / `sugie.toml`, manage users, read the audit log.
    pub fn can_manage(&self) -> bool {
        self.is_admin()
    }

    /// Can submit commands from the console at all.
    pub fn can_execute(&self) -> bool {
        self.role >= Role::Maintainer
    }

    /// Admins reach every repository; everyone else needs an explicit grant.
    pub fn can_access_repo(&self, repo: &str) -> bool {
        if self.is_admin() {
            return true;
        }
        self.repos.iter().any(|r| r.eq_ignore_ascii_case(repo))
    }

    /// `restricted` comes from `sugie.toml` (`default_rules.commands.restricted_commands`).
    pub fn can_run(&self, command: &str, restricted: &[String]) -> bool {
        if !self.can_execute() {
            return false;
        }
        if self.is_admin() {
            return true;
        }
        let is_restricted = restricted
            .iter()
            .any(|r| r.trim_start_matches('/').eq_ignore_ascii_case(command));
        if !is_restricted {
            return true;
        }
        self.commands
            .iter()
            .any(|c| c.trim_start_matches('/').eq_ignore_ascii_case(command))
    }

    /// Repositories visible in the UI: everything for admins, only granted
    /// repositories for everyone else. Input order is preserved and the result is
    /// deduped case-insensitively.
    pub fn visible_repos(&self, all: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(all.len());
        for repo in all {
            if !self.can_access_repo(repo) {
                continue;
            }
            if out.iter().any(|seen| seen.eq_ignore_ascii_case(repo)) {
                continue;
            }
            out.push(repo.clone());
        }
        out
    }
}

/// Command catalogue surfaced in the UI (help text + restricted flag).
pub struct CommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    pub restricted: bool,
    pub admin_only: bool,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "help",
        usage: "/help",
        help: "List all available commands",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "status",
        usage: "/status",
        help: "Bot operational status and connected repositories",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "stats",
        usage: "/stats",
        help: "Issue / PR / commit statistics for current repository",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "issues",
        usage: "/issues [count] [page]",
        help: "List open issues (default 10, max 100 per page)",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "prs",
        usage: "/prs [count] [page]",
        help: "List open PRs (default 10, max 100 per page)",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "issue",
        usage: "/issue <num>",
        help: "Show one issue or PR in detail (state, labels, reviewers, checks)",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "checks",
        usage: "/checks <num>",
        help: "CI check-suite status for a pull request",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "labels",
        usage: "/labels",
        help: "List repository labels",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "milestones",
        usage: "/milestones",
        help: "List open milestones",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "rate",
        usage: "/rate",
        help: "Remaining GitHub API quota for this installation",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "users",
        usage: "/users",
        help: "List eligible GitHub assignees & reviewer pool",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "label",
        usage: "/label <num> <label...>",
        help: "Add labels to an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "unlabel",
        usage: "/unlabel <num> <label...>",
        help: "Remove labels from an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "assign",
        usage: "/assign <num> <user...>",
        help: "Assign an issue or PR to users",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "unassign",
        usage: "/unassign <num> <user...>",
        help: "Remove assignees from an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "reviewer",
        usage: "/reviewer <num> [set|+|-] <user...>",
        help: "List, add, remove or replace (set) PR reviewers",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "unreviewer",
        usage: "/unreviewer <num> <user...>",
        help: "Remove requested reviewers from a PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "workload",
        usage: "/workload",
        help: "Active review workload per reviewer in this repository",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "project",
        usage: "/project <num> <project> [status]",
        help: "Add an issue/PR to a GitHub Project v2 board",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "move",
        usage: "/move <num> <owner/repo>",
        help: "Move an issue to another repository",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "duplicate",
        usage: "/duplicate <num> <#original>",
        help: "Close an issue as a duplicate of another",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "close",
        usage: "/close <num> [reason]",
        help: "Close an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "reopen",
        usage: "/reopen <num>",
        help: "Reopen a closed issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "comment",
        usage: "/comment <num> <text>",
        help: "Post a comment on an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "title",
        usage: "/title <num> <new title>",
        help: "Rename an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "milestone",
        usage: "/milestone <num> <name|none>",
        help: "Set or clear the milestone of an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "lock",
        usage: "/lock <num> [off-topic|too heated|resolved|spam]",
        help: "Lock the conversation on an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "unlock",
        usage: "/unlock <num>",
        help: "Unlock the conversation on an issue or PR",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "retest",
        usage: "/retest <num>",
        help: "Re-trigger CI check runs",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "stale",
        usage: "/stale",
        help: "Run a stale issues/PRs sweep on current repository",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "report",
        usage: "/report [weekly|monthly]",
        help: "Generate repository metrics report",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "config",
        usage: "/config",
        help: "View active configuration summary",
        restricted: false,
        admin_only: false,
    },
    CommandSpec {
        name: "reload",
        usage: "/reload",
        help: "Validate configuration files on disk",
        restricted: true,
        admin_only: false,
    },
    CommandSpec {
        name: "restart",
        usage: "/restart",
        help: "Gracefully restart the service process",
        restricted: true,
        admin_only: true,
    },
];

pub fn command_spec(name: &str) -> Option<&'static CommandSpec> {
    let lower = name.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "assignees" | "collaborators" | "team" => "users",
        "reviewers" | "review" | "reassign" | "rereview" => "reviewer",
        "unreviewers" => "unreviewer",
        other => other,
    };
    COMMANDS
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(canonical))
}
