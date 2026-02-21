use leptos::prelude::*;

use crate::api::types::Task;
use crate::components::task_status_badge::TaskStatusBadge;
use crate::components::ui::*;

#[component]
pub fn TaskTable(
    tasks: Vec<Task>,
    on_delete: Callback<String>,
) -> impl IntoView {
    view! {
        <Table>
            <Thead>
                <Tr>
                    <Th>"ID"</Th>
                    <Th>"Queue"</Th>
                    <Th>"Task Name"</Th>
                    <Th>"Status"</Th>
                    <Th>"Priority"</Th>
                    <Th>"Attempts"</Th>
                    <Th>"Created"</Th>
                    <Th>"Actions"</Th>
                </Tr>
            </Thead>
            <Tbody>
                {if tasks.is_empty() {
                    view! {
                        <Tr>
                            <Td class="text-center text-muted-foreground py-8".to_string()>
                                "No tasks found"
                            </Td>
                        </Tr>
                    }
                        .into_any()
                } else {
                    tasks
                        .into_iter()
                        .map(|task| {
                            let id = task.id.clone();
                            let delete_id = task.id.clone();
                            let detail_link = format!("/tasks/{}", task.id);
                            let short_id = if task.id.len() > 8 {
                                format!("{}...", &task.id[..8])
                            } else {
                                task.id.clone()
                            };
                            view! {
                                <tr
                                    class="transition-colors hover:bg-muted/50 cursor-pointer"
                                    on:click=move |_| {
                                        let _ = leptos_router::hooks::use_navigate()(&detail_link, Default::default());
                                    }
                                >
                                    <Td class="font-mono text-xs".to_string()>
                                        <span title=id>{short_id}</span>
                                    </Td>
                                    <Td>{task.queue_name}</Td>
                                    <Td>{task.task_name}</Td>
                                    <Td>
                                        <TaskStatusBadge status=task.status />
                                    </Td>
                                    <Td>{task.priority.to_string()}</Td>
                                    <Td>
                                        {format!("{}/{}", task.attempt_count, task.max_retries)}
                                    </Td>
                                    <Td class="text-xs text-muted-foreground".to_string()>
                                        {format_timestamp(&task.created_at)}
                                    </Td>
                                    <Td>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            class="text-xs px-2 py-1 h-7".to_string()
                                            on_click=Callback::new(move |e: leptos::ev::MouseEvent| {
                                                e.stop_propagation();
                                                on_delete.run(delete_id.clone())
                                            })
                                        >
                                            "Delete"
                                        </Button>
                                    </Td>
                                </tr>
                            }
                        })
                        .collect_view()
                        .into_any()
                }}
            </Tbody>
        </Table>
    }
}

fn format_timestamp(ts: &str) -> String {
    // Show just date + time portion
    if let Some(t) = ts.get(..19) {
        t.replace('T', " ")
    } else {
        ts.to_string()
    }
}
