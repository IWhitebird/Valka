use leptos::prelude::*;

use crate::api::types::Task;
use crate::components::task_status_badge::TaskStatusBadge;
use crate::components::ui::*;
use crate::server_fns::tasks::cancel_task;
use crate::utils::format_date;

#[component]
fn DetailRow(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <div class="flex items-center gap-3 py-2.5 border-b border-border last:border-b-0">
            <span class="w-36 shrink-0 text-sm text-muted-foreground">{label}</span>
            <span class="text-sm">{children()}</span>
        </div>
    }
}

#[component]
fn JsonBlock(label: &'static str, data: Option<serde_json::Value>) -> impl IntoView {
    let content = match &data {
        None => "null".to_string(),
        Some(v) => serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()),
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{label}</CardTitle>
            </CardHeader>
            <CardContent>
                <pre class="overflow-x-auto rounded-md border bg-muted/30 px-4 py-3 font-mono text-sm">
                    {content}
                </pre>
            </CardContent>
        </Card>
    }
}

#[component]
pub fn TaskDetailPanel(task: Task) -> impl IntoView {
    let task_id = task.id.clone();
    let cancel = Action::new(move |_: &()| {
        let id = task_id.clone();
        async move { cancel_task(id).await }
    });

    let can_cancel =
        task.status == "PENDING" || task.status == "DISPATCHING" || task.status == "RUNNING";

    let scheduled_at = task.scheduled_at.clone();
    let task_name = task.task_name.clone();
    let task_full_id = task.id.clone();
    let status = task.status.clone();
    let queue_name = task.queue_name.clone();
    let idempotency_key = task
        .idempotency_key
        .clone()
        .unwrap_or_else(|| "--".to_string());
    let created = format_date(&task.created_at);
    let updated = format_date(&task.updated_at);
    let attempts = format!("{} / {}", task.attempt_count, task.max_retries);
    let timeout = format!("{}s", task.timeout_seconds);
    let priority = task.priority.to_string();
    let error_msg = task.error_message.clone();

    view! {
        <div class="space-y-6">
            // Header
            <div class="flex items-start justify-between">
                <div>
                    <h2 class="text-xl font-semibold text-foreground">{task_name}</h2>
                    <p class="mt-1 font-mono text-sm text-muted-foreground">{task_full_id}</p>
                </div>
                <div class="flex items-center gap-3">
                    <TaskStatusBadge status />
                    {if can_cancel {
                        Some(view! {
                            <Button
                                variant=ButtonVariant::Destructive
                                on_click=Callback::new(move |_| { let _ = cancel.dispatch(()); })
                            >
                                "Cancel"
                            </Button>
                        })
                    } else {
                        None
                    }}
                </div>
            </div>

            // Details Card
            <Card>
                <CardHeader>
                    <CardTitle>"Details"</CardTitle>
                </CardHeader>
                <CardContent>
                    <DetailRow label="Queue">{queue_name}</DetailRow>
                    <DetailRow label="Priority">{priority}</DetailRow>
                    <DetailRow label="Attempts">{attempts}</DetailRow>
                    <DetailRow label="Timeout">{timeout}</DetailRow>
                    <DetailRow label="Idempotency Key">{idempotency_key}</DetailRow>
                    <DetailRow label="Created">{created}</DetailRow>
                    <DetailRow label="Updated">{updated}</DetailRow>
                    {scheduled_at.map(|sa| {
                        let formatted = format_date(&sa);
                        view! {
                            <DetailRow label="Scheduled At">{formatted}</DetailRow>
                        }
                    })}
                </CardContent>
            </Card>

            // Error Section
            {error_msg.map(|err| view! {
                <Card class="border-destructive/30 bg-destructive/5">
                    <CardHeader>
                        <CardTitle>"Error"</CardTitle>
                    </CardHeader>
                    <CardContent>
                        <p class="font-mono text-sm text-destructive/90">{err}</p>
                    </CardContent>
                </Card>
            })}

            <Separator />

            // JSON Blocks
            <div class="grid gap-6 lg:grid-cols-2">
                <JsonBlock label="Input" data=task.input.clone() />
                <JsonBlock label="Output" data=task.output.clone() />
            </div>
            <JsonBlock label="Metadata" data=task.metadata.clone() />
        </div>
    }
}
