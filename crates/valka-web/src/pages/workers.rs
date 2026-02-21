use leptos::prelude::*;

use crate::components::ui::*;
use crate::components::worker_table::WorkerTable;
use crate::server_fns::workers::list_workers;

#[component]
pub fn WorkersPage() -> impl IntoView {
    let workers = Resource::new(|| (), |_| list_workers());

    view! {
        <div class="space-y-8">
            <div>
                <h1 class="text-2xl font-bold text-foreground">"Workers"</h1>
                <p class="text-muted-foreground mt-1">"Connected worker instances"</p>
            </div>

            <Suspense fallback=move || {
                view! {
                    <div class="space-y-4">
                        <div class="grid grid-cols-3 gap-4">
                            <Skeleton class="h-24".to_string() />
                            <Skeleton class="h-24".to_string() />
                            <Skeleton class="h-24".to_string() />
                        </div>
                        <Skeleton class="h-64".to_string() />
                    </div>
                }
            }>
                {move || {
                    workers
                        .get()
                        .map(|result| match result {
                            Ok(worker_list) => {
                                let total = worker_list.len();
                                let active_tasks: i32 = worker_list
                                    .iter()
                                    .map(|w| w.active_tasks)
                                    .sum();
                                let total_capacity: i32 = worker_list
                                    .iter()
                                    .map(|w| w.concurrency)
                                    .sum();
                                view! {
                                    <div class="space-y-6">
                                        // Summary cards
                                        <div class="grid grid-cols-3 gap-4">
                                            <Card>
                                                <CardHeader>
                                                    <CardTitle>"Total Workers"</CardTitle>
                                                </CardHeader>
                                                <CardContent>
                                                    <p class="text-2xl font-bold text-foreground">
                                                        {total.to_string()}
                                                    </p>
                                                </CardContent>
                                            </Card>
                                            <Card>
                                                <CardHeader>
                                                    <CardTitle>"Active Tasks"</CardTitle>
                                                </CardHeader>
                                                <CardContent>
                                                    <p class="text-2xl font-bold text-info">
                                                        {active_tasks.to_string()}
                                                    </p>
                                                </CardContent>
                                            </Card>
                                            <Card>
                                                <CardHeader>
                                                    <CardTitle>"Total Capacity"</CardTitle>
                                                </CardHeader>
                                                <CardContent>
                                                    <p class="text-2xl font-bold text-success">
                                                        {total_capacity.to_string()}
                                                    </p>
                                                </CardContent>
                                            </Card>
                                        </div>
                                        // Worker table
                                        <Card class="p-0".to_string()>
                                            <WorkerTable workers=worker_list />
                                        </Card>
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
