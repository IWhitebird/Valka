use leptos::prelude::*;

use crate::api::types::CreateTaskRequest;
use crate::components::ui::*;

#[component]
pub fn TaskCreateDialog(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    on_create: Callback<CreateTaskRequest>,
) -> impl IntoView {
    let (queue_name, set_queue_name) = signal(String::new());
    let (task_name, set_task_name) = signal(String::new());
    let (input_json, set_input_json) = signal(String::from("{}"));
    let (priority, set_priority) = signal(String::from("0"));
    let (max_retries, set_max_retries) = signal(String::from("3"));
    let (timeout, set_timeout) = signal(String::from("300"));

    let handle_submit = move |_| {
        let input = serde_json::from_str(&input_json.get()).ok();
        let req = CreateTaskRequest {
            queue_name: queue_name.get(),
            task_name: task_name.get(),
            input,
            priority: priority.get().parse().ok(),
            max_retries: max_retries.get().parse().ok(),
            timeout_seconds: timeout.get().parse().ok(),
            scheduled_at: None,
            idempotency_key: None,
            metadata: None,
        };
        on_create.run(req);
        set_open.set(false);
        // Reset
        set_queue_name.set(String::new());
        set_task_name.set(String::new());
        set_input_json.set("{}".to_string());
        set_priority.set("0".to_string());
        set_max_retries.set("3".to_string());
        set_timeout.set("300".to_string());
    };

    view! {
        <Show when=move || open.get()>
            // Backdrop
            <div
                class="fixed inset-0 z-50 bg-black/50"
                on:click=move |_| set_open.set(false)
            ></div>
            // Dialog
            <div class="fixed inset-0 z-50 flex items-center justify-center">
                <div class="w-full max-w-lg rounded-lg border border-border bg-card p-6 shadow-lg">
                    <h2 class="text-lg font-semibold text-foreground mb-4">"Create Task"</h2>

                    <div class="space-y-4">
                        <div>
                            <label class="text-sm text-muted-foreground block mb-1">
                                "Queue Name"
                            </label>
                            <Input
                                placeholder="e.g. default"
                                value=queue_name.get_untracked()
                                on_input=Callback::new(move |v| set_queue_name.set(v))
                            />
                        </div>
                        <div>
                            <label class="text-sm text-muted-foreground block mb-1">
                                "Task Name"
                            </label>
                            <Input
                                placeholder="e.g. process-image"
                                value=task_name.get_untracked()
                                on_input=Callback::new(move |v| set_task_name.set(v))
                            />
                        </div>
                        <div>
                            <label class="text-sm text-muted-foreground block mb-1">
                                "Input (JSON)"
                            </label>
                            <Input
                                value=input_json.get_untracked()
                                on_input=Callback::new(move |v| set_input_json.set(v))
                            />
                        </div>
                        <div class="grid grid-cols-3 gap-4">
                            <div>
                                <label class="text-sm text-muted-foreground block mb-1">
                                    "Priority"
                                </label>
                                <Input
                                    value=priority.get_untracked()
                                    on_input=Callback::new(move |v| set_priority.set(v))
                                />
                            </div>
                            <div>
                                <label class="text-sm text-muted-foreground block mb-1">
                                    "Max Retries"
                                </label>
                                <Input
                                    value=max_retries.get_untracked()
                                    on_input=Callback::new(move |v| set_max_retries.set(v))
                                />
                            </div>
                            <div>
                                <label class="text-sm text-muted-foreground block mb-1">
                                    "Timeout (s)"
                                </label>
                                <Input
                                    value=timeout.get_untracked()
                                    on_input=Callback::new(move |v| set_timeout.set(v))
                                />
                            </div>
                        </div>
                    </div>

                    <div class="flex justify-end gap-3 mt-6">
                        <Button
                            variant=ButtonVariant::Outline
                            on_click=Callback::new(move |_| set_open.set(false))
                        >
                            "Cancel"
                        </Button>
                        <Button on_click=Callback::new(handle_submit)>"Create"</Button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
