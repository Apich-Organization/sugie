//! Leptos SSR shell: document scaffold, top navigation and shared UI atoms.

use crate::web::perms::CurrentUser;
use leptos::prelude::*;

pub mod pages;

/// Render a component tree to a complete HTML document.
pub fn render(view: impl IntoView + 'static) -> String {
    let body = view.into_view().to_html();
    // One allocation instead of `format!` copying the whole document again.
    let mut out = String::with_capacity(body.len() + 16);
    out.push_str("<!DOCTYPE html>");
    out.push_str(&body);
    out
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
pub fn AmbientBg() -> impl IntoView {
    view! {
        <div class="ambient-bg" aria-hidden="true">
            <div class="orb orb-1"></div>
            <div class="orb orb-2"></div>
            <div class="orb orb-3"></div>
            <div class="orb orb-4"></div>
            <svg
                class="ambient-bezier"
                viewBox="0 0 1440 900"
                fill="none"
                 preserveAspectRatio="xMidYMid slice"
                xmlns="http://www.w3.org/2000/svg"
            >
                <defs>
                    <linearGradient id="bz-ribbon-1" x1="0%" y1="0%" x2="100%" y2="100%">
                        <stop offset="0%" stop-color="#8b68e4" stop-opacity="0.22"/>
                        <stop offset="50%" stop-color="#b47cf8" stop-opacity="0.14"/>
                        <stop offset="100%" stop-color="#6496f6" stop-opacity="0.04"/>
                    </linearGradient>
                    <linearGradient id="bz-ribbon-2" x1="100%" y1="0%" x2="0%" y2="100%">
                        <stop offset="0%" stop-color="#c47ef6" stop-opacity="0.18"/>
                        <stop offset="55%" stop-color="#8b68e4" stop-opacity="0.12"/>
                        <stop offset="100%" stop-color="#6e50c0" stop-opacity="0.02"/>
                    </linearGradient>
                    <linearGradient id="bz-stroke-1" x1="0%" y1="50%" x2="100%" y2="50%">
                        <stop offset="0%" stop-color="#8b68e4" stop-opacity="0.0"/>
                        <stop offset="25%" stop-color="#9e78f0" stop-opacity="0.45"/>
                        <stop offset="65%" stop-color="#c788f8" stop-opacity="0.52"/>
                        <stop offset="100%" stop-color="#6a8ef6" stop-opacity="0.0"/>
                    </linearGradient>
                    <linearGradient id="bz-stroke-2" x1="0%" y1="50%" x2="100%" y2="50%">
                        <stop offset="0%" stop-color="#6a8ef6" stop-opacity="0.0"/>
                        <stop offset="35%" stop-color="#a87ef2" stop-opacity="0.38"/>
                        <stop offset="75%" stop-color="#8b68e4" stop-opacity="0.42"/>
                        <stop offset="100%" stop-color="#b47cf8" stop-opacity="0.0"/>
                    </linearGradient>
                </defs>
                <path
                    class="bz-wave bz-wave-1"
                    d="M -120,260 C 240,60 520,440 880,220 C 1160,50 1340,320 1560,180 L 1560,960 L -120,960 Z"
                    fill="url(#bz-ribbon-1)"
                />
                <path
                    class="bz-wave bz-wave-2"
                    d="M -120,580 C 310,360 640,740 1020,480 C 1260,316 1410,510 1560,420 L 1560,960 L -120,960 Z"
                    fill="url(#bz-ribbon-2)"
                />
                <path
                    class="bz-line bz-line-1"
                    d="M -100,240 C 260,40 540,460 900,210 C 1180,20 1350,310 1540,170"
                    stroke="url(#bz-stroke-1)"
                    stroke-width="2"
                    stroke-linecap="round"
                />
                <path
                    class="bz-line bz-line-2"
                    d="M -100,430 C 220,610 590,230 960,450 C 1220,605 1380,350 1540,460"
                    stroke="url(#bz-stroke-2)"
                    stroke-width="1.6"
                    stroke-linecap="round"
                />
                <path
                    class="bz-line bz-line-3"
                    d="M -80,680 C 340,460 670,820 1050,560 C 1280,400 1420,580 1540,510"
                    stroke="url(#bz-stroke-1)"
                    stroke-width="1.25"
                    stroke-dasharray="8 10"
                    stroke-linecap="round"
                />
            </svg>
        </div>
    }
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
        None => (
            "Not signed in".to_string(),
            "—".to_string(),
            "?".to_string(),
        ),
    };

    let nav_views = items
        .into_iter()
        .map(|(key, href, label)| {
            let cls = if key == active { "active" } else { "" };
            let current = if key == active { Some("page") } else { None };
            view! { <a href=href class=cls aria-current=current>{label}</a> }
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
                // Applied before first paint: otherwise dark-mode users see a
                // flash of the light theme while app.js loads at the end of body.
                <script>
                    "try{var t=localStorage.getItem('sugie_theme');if(!t&&window.matchMedia&&window.matchMedia('(prefers-color-scheme: dark)').matches){t='dark';}if(t){document.documentElement.setAttribute('data-theme',t);}}catch(e){}"
                </script>
            </head>
            <body>
                <script>
                    "try{var s=sessionStorage.getItem('sugie_scroll_'+location.pathname);if(s){window.scrollTo(0,parseInt(s,10));}}catch(e){}"
                </script>
                <AmbientBg/>
                <div class="topbar">
                    <a href="/admin" class="brand">
                        <img class="logo" src="/admin/assets/logo.svg" alt="Sugie"/>
                        <span>"Sugie"<small>" Admin"</small></span>
                    </a>
                    <nav class="nav">{nav_views}</nav>
                    <div class="topbar-right">
                        <button class="icon-btn" data-theme-toggle="" title="Toggle theme" aria-label="Toggle colour theme">"◐"</button>
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
            <svg
                class="stat-bezier"
                viewBox="0 0 240 64"
                fill="none"
                preserveAspectRatio="none"
                aria-hidden="true"
            >
                <path
                    class="stat-bz-area"
                    d="M 0,52 C 48,48 76,18 128,30 C 172,40 202,8 240,14 L 240,64 L 0,64 Z"
                />
                <path
                    class="stat-bz-stroke"
                    d="M 0,52 C 48,48 76,18 128,30 C 172,40 202,8 240,14"
                    stroke-width="2"
                    stroke-linecap="round"
                />
            </svg>
        </div>
    }
}

#[component]
pub fn Flash(ok: bool, text: String) -> impl IntoView {
    if text.is_empty() {
        view! { <span></span> }.into_any()
    } else {
        let cls = if ok { "alert ok" } else { "alert err" };
        // role="status" makes redirect flash messages audible to screen readers.
        let role = if ok { "status" } else { "alert" };
        view! { <div class=cls role=role aria-live="polite">{text}</div> }.into_any()
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
