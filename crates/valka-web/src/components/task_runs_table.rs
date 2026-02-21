use leptos::prelude::*;

use crate::api::types::TaskRun;
use crate::components::task_status_badge::TaskStatusBadge;
use crate::components::ui::*;
use crate::utils::{format_date, truncate_id};

#[component]
pub fn TaskRunsTable(
    runs: Vec<TaskRun>,
    selected_run_id: ReadSignal<Option<String>>,
    on_select_run: Callback<String>,
) -> impl IntoView {
    if runs.is_empty() {
        return view! {
            <div class="flex h-32 flex-col items-center justify-center rounded-lg border bg-card">
                <svg xmlns="http://www.w3.org/2000/svg" class="h-8 w-8 text-muted-foreground/50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2" />
                </svg>
                <p class="mt-2 text-sm text-muted-foreground">"No runs yet"</p>
            </div>
        }
        .into_any();
    }

    view! {
        <div class="overflow-hidden rounded-lg border">
            <Table>
                <Thead>
                    <Tr class="hover:bg-transparent">
                        <Th>"Run ID"</Th>
                        <Th>"Attempt"</Th>
                        <Th>"Status"</Th>
                        <Th>"Worker"</Th>
                        <Th>"Started"</Th>
                        <Th>"Completed"</Th>
                        <Th>"Error"</Th>
                    </Tr>
                </Thead>
                <Tbody>
                    {runs.into_iter().map(|run| {
                        let run_id = run.id.clone();
                        let run_id_click = run.id.clone();
                        let on_select = on_select_run;
                        view! {
                            <tr
                                class=move || {
                                    let base = "transition-colors hover:bg-muted/50 cursor-pointer";
                                    if selected_run_id.get().as_deref() == Some(&run_id) {
                                        format!("{base} bg-accent")
                                    } else {
                                        base.to_string()
                                    }
                                }
                                on:click=move |_| on_select.run(run_id_click.clone())
                            >
                                <Td class="font-mono text-xs text-muted-foreground">
                                    {truncate_id(&run.id, 8)}
                                </Td>
                                <Td>
                                    {format!("#{}", run.attempt_number)}
                                </Td>
                                <Td>
                                    <TaskStatusBadge status=run.status.clone() />
                                </Td>
                                <Td class="font-mono text-xs text-muted-foreground">
                                    {run.worker_id.as_deref().map(|w| truncate_id(w, 8)).unwrap_or_else(|| "--".to_string())}
                                </Td>
                                <Td class="text-xs text-muted-foreground">
                                    {format_date(&run.started_at)}
                                </Td>
                                <Td class="text-xs text-muted-foreground">
                                    {run.completed_at.as_deref().map(format_date).unwrap_or_else(|| "--".to_string())}
                                </Td>
                                <Td class="max-w-[200px] truncate text-xs text-red-400">
                                    {run.error_message.clone().unwrap_or_else(|| "--".to_string())}
                                </Td>
                            </tr>
                        }
                    }).collect_view()}
                </Tbody>
            </Table>
        </div>
    }
    .into_any()
}
