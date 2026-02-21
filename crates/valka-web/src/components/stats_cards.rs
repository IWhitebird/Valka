use leptos::prelude::*;

use crate::api::types::Task;
use crate::components::ui::{Card, CardContent, CardHeader, CardTitle};

#[component]
pub fn StatsCards(tasks: Vec<Task>) -> impl IntoView {
    let total = tasks.len();
    let pending = tasks.iter().filter(|t| t.status == "PENDING").count();
    let running = tasks
        .iter()
        .filter(|t| t.status == "RUNNING" || t.status == "DISPATCHING")
        .count();
    let completed = tasks.iter().filter(|t| t.status == "COMPLETED").count();
    let failed = tasks
        .iter()
        .filter(|t| t.status == "FAILED" || t.status == "DEAD_LETTER")
        .count();

    let cards = vec![
        ("Total Tasks", total, "text-foreground"),
        ("Pending", pending, "text-primary"),
        ("Running", running, "text-info"),
        ("Completed", completed, "text-success"),
        ("Failed", failed, "text-destructive"),
    ];

    view! {
        <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-4">
            {cards
                .into_iter()
                .map(|(label, count, color)| {
                    view! {
                        <Card>
                            <CardHeader>
                                <CardTitle>{label}</CardTitle>
                            </CardHeader>
                            <CardContent>
                                <p class=format!("text-2xl font-bold {color}")>
                                    {count.to_string()}
                                </p>
                            </CardContent>
                        </Card>
                    }
                })
                .collect_view()}
        </div>
    }
}
