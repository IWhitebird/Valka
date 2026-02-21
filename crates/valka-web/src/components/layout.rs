use leptos::prelude::*;
use leptos_router::components::*;
use leptos_router::hooks::use_location;

#[component]
pub fn RootLayout() -> impl IntoView {
    view! {
        <div class="flex h-screen bg-background">
            <Sidebar />
            <main class="flex-1 overflow-auto p-8">
                <Outlet />
            </main>
        </div>
    }
}

#[component]
fn Sidebar() -> impl IntoView {
    let location = use_location();

    view! {
        <aside class="w-60 border-r border-sidebar-border bg-sidebar flex flex-col">
            // Logo
            <div class="h-16 flex items-center px-6 border-b border-sidebar-border">
                <a href="/" class="flex items-center gap-3">
                    <img src="/valka.svg" alt="Valka" class="h-7 w-7" />
                    <span class="text-lg font-semibold text-foreground tracking-tight">
                        "Valka"
                    </span>
                </a>
            </div>

            // Navigation
            <nav class="flex-1 px-3 py-4 space-y-1">
                <NavLink
                    href="/"
                    label="Dashboard"
                    pathname=location.pathname
                    exact=true
                >
                    <IconDashboard />
                </NavLink>
                <NavLink href="/tasks" label="Tasks" pathname=location.pathname exact=false>
                    <IconTasks />
                </NavLink>
                <NavLink
                    href="/workers"
                    label="Workers"
                    pathname=location.pathname
                    exact=false
                >
                    <IconWorkers />
                </NavLink>
                <NavLink
                    href="/dead-letters"
                    label="Dead Letters"
                    pathname=location.pathname
                    exact=false
                >
                    <IconDeadLetters />
                </NavLink>
                <NavLink
                    href="/events"
                    label="Events"
                    pathname=location.pathname
                    exact=false
                >
                    <IconEvents />
                </NavLink>
            </nav>

            // Footer
            <div class="px-6 py-4 border-t border-sidebar-border">
                <p class="text-xs text-muted-foreground">"Valka Task Queue"</p>
            </div>
        </aside>
    }
}

#[component]
fn NavLink(
    href: &'static str,
    label: &'static str,
    pathname: Memo<String>,
    exact: bool,
    children: Children,
) -> impl IntoView {
    let is_active = move || {
        let p = pathname.get();
        if exact { p == href } else { p.starts_with(href) }
    };

    view! {
        <a
            href=href
            class=move || {
                let base = "flex items-center gap-3 px-3 py-2 rounded-md text-sm font-medium transition-colors";
                if is_active() {
                    format!("{base} bg-sidebar-accent text-sidebar-accent-foreground")
                } else {
                    format!(
                        "{base} text-sidebar-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
                    )
                }
            }
        >
            {children()}
            {label}
        </a>
    }
}

#[component]
fn IconDashboard() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <rect width="7" height="9" x="3" y="3" rx="1" />
            <rect width="7" height="5" x="14" y="3" rx="1" />
            <rect width="7" height="9" x="14" y="12" rx="1" />
            <rect width="7" height="5" x="3" y="16" rx="1" />
        </svg>
    }
}

#[component]
fn IconTasks() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <path d="M12 2H2v10h10V2z" />
            <path d="M22 12H12v10h10V12z" />
            <path d="M17 2h5v5" />
            <path d="M2 17v5h5" />
        </svg>
    }
}

#[component]
fn IconWorkers() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <rect x="4" y="4" width="6" height="6" rx="1" />
            <rect x="14" y="4" width="6" height="6" rx="1" />
            <rect x="4" y="14" width="6" height="6" rx="1" />
            <rect x="14" y="14" width="6" height="6" rx="1" />
        </svg>
    }
}

#[component]
fn IconDeadLetters() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <circle cx="9" cy="12" r="1" />
            <circle cx="15" cy="12" r="1" />
            <path d="M8 20v2h8v-2" />
            <path d="m12.5 17-.5-1-.5 1h1z" />
            <path d="M16 20a2 2 0 0 0 1.56-3.25 8 8 0 1 0-11.12 0A2 2 0 0 0 8 20" />
        </svg>
    }
}

#[component]
fn IconEvents() -> impl IntoView {
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
        >
            <path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2" />
        </svg>
    }
}
