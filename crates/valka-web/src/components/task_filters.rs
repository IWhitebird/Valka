use leptos::prelude::*;

use crate::components::ui::{Input, Select};

#[component]
pub fn TaskFilters(
    queue_name: ReadSignal<String>,
    set_queue_name: WriteSignal<String>,
    status: ReadSignal<String>,
    set_status: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-4">
            <div class="w-64">
                <Input
                    placeholder="Filter by queue name..."
                    value=queue_name.get_untracked()
                    on_input=Callback::new(move |v: String| set_queue_name.set(v))
                />
            </div>
            <div class="w-48">
                <Select
                    value=status.get_untracked()
                    on_change=Callback::new(move |v: String| set_status.set(v))
                >
                    <option value="">"All Statuses"</option>
                    <option value="PENDING">"Pending"</option>
                    <option value="DISPATCHING">"Dispatching"</option>
                    <option value="RUNNING">"Running"</option>
                    <option value="COMPLETED">"Completed"</option>
                    <option value="FAILED">"Failed"</option>
                    <option value="RETRY">"Retry"</option>
                    <option value="DEAD_LETTER">"Dead Letter"</option>
                    <option value="CANCELLED">"Cancelled"</option>
                </Select>
            </div>
        </div>
    }
}
