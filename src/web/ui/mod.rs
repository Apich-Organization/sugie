//! Leptos SSR shell: document scaffold, top navigation and shared UI atoms.

use crate::web::perms::CurrentUser;
use leptos::prelude::*;

pub mod pages;

/// Render a component tree to a complete HTML document.
pub fn render(view: impl IntoView + 'static) -> String {
    let body = view.into_view().to_html();
    format!("<!DOCTYPE html>{body}")
}

fn nav_items(user: &Option<CurrentUser>) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut items = vec![
        ("dashboard", "/admin", "Dashboard"),
        ("repos", "/admin/repos", "Repositories"),
        ("chat", "/admin/chat", "Console"),
    ];
    if user.as_ref().map(|u| u.can_manage()).unwrap_or(false) {
        items.push(("config", "/admin/config", "Configuration"));
        items.push(("users", "/admin/users", "Users"));
        items.push(("notify", "/admin/notify", "Notify"));
        items.push(("audit", "/admin/audit", "Audit Logs"));
    }
    items.push(("security", "/admin/security", "2FA & Passkeys"));
    items
}

#[component]
pub fn Shell(
    title: String,
    active: String,
    user: Option<CurrentUser>,
    children: Children,
) -> impl IntoView {
    let items = nav_items(&user);
    let full_title = format!("{title} · Sugie Admin");
    let (display, role_label, initials) = match &user {
        Some(u) => (
            u.display_name.clone(),
            u.role.label().to_string(),
            u.display_name.chars().next().unwrap_or('?').to_string(),
        ),
        None => ("Not signed in".to_string(), "—".to_string(), "?".to_string()),
    };

    let nav_views = items
        .into_iter()
        .map(|(key, href, label)| {
            let cls = if key == active { "active" } else { "" };
            view! { <a href=href class=cls>{label}</a> }
        })
        .collect_view();

    view! {
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>{full_title}</title>
                <link rel="icon" type="image/svg+xml" href="/admin/assets/logo.svg"/>
                <link rel="stylesheet" href="/admin/assets/app.css"/>
            </head>
            <body>
                <div class="topbar">
                    <a href="/admin" class="brand">
                        <img class="logo" src="/admin/assets/logo.svg" alt="Sugie"/>
                        <span>"Sugie"<small>" Admin"</small></span>
                    </a>
                    <nav class="nav">{nav_views}</nav>
                    <div class="topbar-right">
                        <button class="icon-btn" data-theme-toggle="" title="Toggle theme">"◐"</button>
                        <div class="user-chip">
                            <span class="avatar">{initials}</span>
                            <span>{display}" · "{role_label}</span>
                        </div>
                        <form method="post" action="/admin/logout" style="margin:0">
                            <button class="btn sm ghost" type="submit">"Sign out"</button>
                        </form>
                    </div>
                </div>
                <main class="page">{children()}</main>
                <footer class="foot">"Sugie Triage Bot · Admin Console · Leptos SSR"</footer>
                <script src="/admin/assets/app.js"></script>
            </body>
        </html>
    }
}

#[component]
pub fn StatCard(label: String, value: String, sub: String, accent: bool) -> impl IntoView {
    let cls = if accent { "stat accent" } else { "stat" };
    view! {
        <div class=cls>
            <div class="label">{label}</div>
            <div class="value">{value}</div>
            <div class="sub">{sub}</div>
        </div>
    }
}

#[component]
pub fn Flash(ok: bool, text: String) -> impl IntoView {
    if text.is_empty() {
        view! { <span></span> }.into_any()
    } else {
        let cls = if ok { "alert ok" } else { "alert err" };
        view! { <div class=cls>{text}</div> }.into_any()
    }
}

pub fn number(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
