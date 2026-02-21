use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::components::task_detail_panel::TaskDetailPanel;
use crate::components::task_logs_viewer::TaskLogsViewer;
use crate::components::task_runs_table::TaskRunsTable;
use crate::components::task_signals_panel::TaskSignalsPanel;
use crate::components::ui::*;
use crate::server_fns::tasks::{delete_task, get_task, get_task_runs};

#[component]
pub fn TaskDetailPage() -> impl IntoView {
    let params = use_params_map();
    let task_id = move || params.get().get("task_id").unwrap_or_default();

    let (selected_run_id, set_selected_run_id) = signal(Option::<String>::None);

    let task = Resource::new(
        move || task_id(),
        move |id| async move { get_task(id).await.ok() },
    );

    let runs = Resource::new(
        move || task_id(),
        move |id| async move { get_task_runs(id).await.unwrap_or_default() },
    );

    // Auto-select latest run
    Effect::new(move || {
        let run_list = runs.get().unwrap_or_default();
        if !run_list.is_empty() && selected_run_id.get_untracked().is_none() {
            set_selected_run_id.set(Some(run_list[0].id.clone()));
        }
    });

    let delete = Action::new(move |id: &String| {
        let id = id.clone();
        async move { delete_task(id).await }
    });

    // Navigate after delete
    Effect::new(move || {
        if let Some(Ok(_)) = delete.value().get() {
            let _ = leptos_router::hooks::use_navigate()("/tasks", Default::default());
        }
    });

    view! {
        <Suspense fallback=move || view! {
            <div class="flex h-64 items-center justify-center text-sm text-muted-foreground">
                "Loading task..."
            </div>
        }>
            {move || {
                let maybe_task = task.get().flatten();
                match maybe_task {
                    None => view! {
                        <div class="flex h-64 items-center justify-center">
                            <div class="text-center">
                                <p class="text-sm text-muted-foreground">"Task not found"</p>
                                <a href="/tasks" class="text-primary hover:underline mt-2 inline-block text-sm">
                                    "\u{2190} Back to tasks"
                                </a>
                            </div>
                        </div>
                    }.into_any(),
                    Some(t) => {
                        let tid = t.id.clone();
                        let tid2 = t.id.clone();
                        let tid3 = t.id.clone();
                        let task_status = t.status.clone();
                        let run_list = runs.get().unwrap_or_default();
                        view! {
                            <div class="space-y-6">
                                // Back link + Delete
                                <div class="flex items-center justify-between">
                                    <a
                                        href="/tasks"
                                        class="inline-flex items-center gap-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground"
                                    >
                                        "\u{2190} Back to tasks"
                                    </a>
                                    <Button
                                        variant=ButtonVariant::Outline
                                        class="text-destructive hover:text-destructive"
                                        on_click=Callback::new({
                                            let tid = tid.clone();
                                            move |_: leptos::ev::MouseEvent| {
                                                delete.dispatch(tid.clone());
                                            }
                                        })
                                    >
                                        "Delete"
                                    </Button>
                                </div>

                                <TaskDetailPanel task=t />

                                <Separator />

                                <TaskSignalsPanel task_id=tid2 task_status=task_status />

                                <Separator />

                                <div class="space-y-4">
                                    <h3 class="text-lg font-semibold text-foreground">"Runs"</h3>
                                    <TaskRunsTable
                                        runs=run_list
                                        selected_run_id
                                        on_select_run=Callback::new(move |id: String| {
                                            set_selected_run_id.set(Some(id));
                                        })
                                    />
                                </div>

                                <Show when=move || selected_run_id.get().is_some()>
                                    <div class="space-y-4">
                                        <h3 class="text-lg font-semibold text-foreground">"Logs"</h3>
                                        <TaskLogsViewer
                                            task_id=tid3.clone()
                                            run_id=selected_run_id.get().unwrap_or_default()
                                        />
                                    </div>
                                </Show>
                            </div>
                        }.into_any()
                    }
                }
            }}
        </Suspense>
    }
}
