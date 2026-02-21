use leptos::prelude::*;
use std::collections::HashMap;

use crate::api::types::Task;
use crate::components::ui::{Card, CardContent, CardHeader, CardTitle};

#[component]
pub fn QueueOverview(tasks: Vec<Task>) -> impl IntoView {
    let mut queues: HashMap<String, usize> = HashMap::new();
    for task in &tasks {
        *queues.entry(task.queue_name.clone()).or_default() += 1;
    }
    let total = tasks.len().max(1) as f64;
    let mut sorted: Vec<_> = queues.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Queue Overview"</CardTitle>
            </CardHeader>
            <CardContent>
                {if sorted.is_empty() {
                    view! { <p class="text-muted-foreground text-sm">"No tasks yet"</p> }
                        .into_any()
                } else {
                    view! {
                        <div class="space-y-3">
                            {sorted
                                .into_iter()
                                .map(|(queue, count)| {
                                    let pct = (count as f64 / total * 100.0) as u32;
                                    view! {
                                        <div>
                                            <div class="flex justify-between text-sm mb-1">
                                                <span class="text-foreground font-medium">
                                                    {queue.clone()}
                                                </span>
                                                <span class="text-muted-foreground">
                                                    {count.to_string()}
                                                </span>
                                            </div>
                                            <div class="h-2 rounded-full bg-muted overflow-hidden">
                                                <div
                                                    class="h-full rounded-full bg-primary transition-all"
                                                    style=format!("width: {}%", pct)
                                                ></div>
                                            </div>
                                        </div>
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                        .into_any()
                }}
            </CardContent>
        </Card>
    }
}
