use leptos::prelude::*;

use crate::components::queue_overview::QueueOverview;
use crate::components::stats_cards::StatsCards;
use crate::components::ui::Skeleton;
use crate::server_fns::tasks::list_tasks;

#[component]
pub fn DashboardPage() -> impl IntoView {
    let tasks = Resource::new(|| (), |_| list_tasks(None, None, Some(500), None));

    view! {
        <div class="space-y-8">
            <div>
                <h1 class="text-2xl font-bold text-foreground">"Dashboard"</h1>
                <p class="text-muted-foreground mt-1">"Overview of your task queue"</p>
            </div>

            <Suspense fallback=move || {
                view! {
                    <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-4">
                        <Skeleton class="h-24".to_string() />
                        <Skeleton class="h-24".to_string() />
                        <Skeleton class="h-24".to_string() />
                        <Skeleton class="h-24".to_string() />
                        <Skeleton class="h-24".to_string() />
                    </div>
                }
            }>
                {move || {
                    tasks
                        .get()
                        .map(|result| match result {
                            Ok(tasks) => {
                                let tasks2 = tasks.clone();
                                view! {
                                    <div class="space-y-8">
                                        <StatsCards tasks=tasks />
                                        <QueueOverview tasks=tasks2 />
                                    </div>
                                }
                                    .into_any()
                            }
                            Err(e) => {
                                view! {
                                    <div class="rounded-lg border border-destructive/50 bg-destructive/10 p-4">
                                        <p class="text-destructive text-sm">{e.to_string()}</p>
                                    </div>
                                }
                                    .into_any()
                            }
                        })
                }}
            </Suspense>
        </div>
    }
}
