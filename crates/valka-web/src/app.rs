use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::*;
use leptos_router::path;

use crate::components::layout::RootLayout;
use crate::pages::dashboard::DashboardPage;
use crate::pages::dead_letters::DeadLettersPage;
use crate::pages::events::EventsPage;
use crate::pages::task_detail::TaskDetailPage;
use crate::pages::tasks::TasksPage;
use crate::pages::workers::WorkersPage;

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/valka-web.css" />
        <Title text="Valka Dashboard" />
        <Router>
            <Routes fallback=|| view! { <NotFound /> }>
                <ParentRoute path=path!("/") view=RootLayout>
                    <Route path=path!("") view=DashboardPage />
                    <Route path=path!("tasks") view=TasksPage />
                    <Route path=path!("tasks/:task_id") view=TaskDetailPage />
                    <Route path=path!("workers") view=WorkersPage />
                    <Route path=path!("dead-letters") view=DeadLettersPage />
                    <Route path=path!("events") view=EventsPage />
                </ParentRoute>
            </Routes>
        </Router>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    view! {
        <div class="flex items-center justify-center min-h-screen">
            <div class="text-center">
                <h1 class="text-4xl font-bold text-foreground mb-2">"404"</h1>
                <p class="text-muted-foreground">"Page not found"</p>
                <a href="/" class="text-primary hover:underline mt-4 inline-block">
                    "Back to Dashboard"
                </a>
            </div>
        </div>
    }
}
