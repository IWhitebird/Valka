use leptos::prelude::*;

use crate::components::event_stream::EventStream;

#[component]
pub fn EventsPage() -> impl IntoView {
    view! {
        <div class="space-y-6">
            <div>
                <h1 class="text-2xl font-semibold tracking-tight text-foreground">"Events"</h1>
                <p class="mt-1 text-sm text-muted-foreground">
                    "Live stream of task queue events via SSE"
                </p>
            </div>
            <EventStream />
        </div>
    }
}
