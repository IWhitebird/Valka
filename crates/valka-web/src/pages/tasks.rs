use leptos::prelude::*;

use crate::api::types::CreateTaskRequest;
use crate::components::task_create_dialog::TaskCreateDialog;
use crate::components::task_filters::TaskFilters;
use crate::components::task_table::TaskTable;
use crate::components::ui::*;
use crate::server_fns::tasks::{clear_all_tasks, create_task, delete_task, list_tasks};

const PAGE_SIZE: i64 = 25;

#[component]
pub fn TasksPage() -> impl IntoView {
    let (queue_name, set_queue_name) = signal(String::new());
    let (status, set_status) = signal(String::new());
    let (offset, set_offset) = signal(0i64);
    let (dialog_open, set_dialog_open) = signal(false);

    // Track mutations to trigger refetch
    let (version, set_version) = signal(0u32);

    let tasks = Resource::new(
        move || {
            (
                queue_name.get(),
                status.get(),
                offset.get(),
                version.get(),
            )
        },
        move |(q, s, o, _)| {
            let queue = if q.is_empty() { None } else { Some(q) };
            let stat = if s.is_empty() { None } else { Some(s) };
            async move { list_tasks(queue, stat, Some(PAGE_SIZE), Some(o)).await }
        },
    );

    let refetch = move || set_version.update(|v| *v += 1);

    let delete_action = Action::new(move |task_id: &String| {
        let id = task_id.clone();
        async move {
            let _ = delete_task(id).await;
        }
    });

    let create_action = Action::new(move |req: &CreateTaskRequest| {
        let req = req.clone();
        async move {
            let _ = create_task(req).await;
        }
    });

    let clear_action = Action::new(move |_: &()| async move {
        let _ = clear_all_tasks().await;
    });

    // Refetch on mutation completion
    Effect::new(move || {
        delete_action.value().get();
        refetch();
    });
    Effect::new(move || {
        create_action.value().get();
        refetch();
    });
    Effect::new(move || {
        clear_action.value().get();
        refetch();
    });

    view! {
        <div class="space-y-6">
            <div class="flex items-center justify-between">
                <div>
                    <h1 class="text-2xl font-bold text-foreground">"Tasks"</h1>
                    <p class="text-muted-foreground mt-1">"Manage your task queue"</p>
                </div>
                <div class="flex items-center gap-3">
                    <Button
                        variant=ButtonVariant::Outline
                        on_click=Callback::new(move |_| { let _ = clear_action.dispatch(()); })
                    >
                        "Clear All"
                    </Button>
                    <Button on_click=Callback::new(move |_| set_dialog_open.set(true))>
                        "Create Task"
                    </Button>
                </div>
            </div>

            <TaskFilters
                queue_name=queue_name
                set_queue_name=set_queue_name
                status=status
                set_status=set_status
            />

            <Card class="p-0".to_string()>
                <Suspense fallback=move || {
                    view! {
                        <div class="p-8">
                            <Skeleton class="h-64".to_string() />
                        </div>
                    }
                }>
                    {move || {
                        tasks
                            .get()
                            .map(|result| match result {
                                Ok(task_list) => {
                                    let has_more = task_list.len() as i64 >= PAGE_SIZE;
                                    let current_offset = offset.get_untracked();
                                    view! {
                                        <div>
                                            <TaskTable
                                                tasks=task_list
                                                on_delete=Callback::new(move |id: String| {
                                                    let _ = delete_action.dispatch(id);
                                                })
                                            />
                                            <div class="flex items-center justify-between border-t border-border px-4 py-3">
                                                <p class="text-sm text-muted-foreground">
                                                    {format!(
                                                        "Showing from offset {}",
                                                        current_offset,
                                                    )}
                                                </p>
                                                <div class="flex items-center gap-2">
                                                    <Button
                                                        variant=ButtonVariant::Outline
                                                        class="text-xs px-3 py-1 h-8".to_string()
                                                        disabled=Signal::derive(move || {
                                                            offset.get() == 0
                                                        })
                                                        on_click=Callback::new(move |_| {
                                                            set_offset
                                                                .update(|o| *o = (*o - PAGE_SIZE).max(0))
                                                        })
                                                    >
                                                        "Previous"
                                                    </Button>
                                                    <Button
                                                        variant=ButtonVariant::Outline
                                                        class="text-xs px-3 py-1 h-8".to_string()
                                                        disabled=Signal::derive(move || !has_more)
                                                        on_click=Callback::new(move |_| {
                                                            set_offset.update(|o| *o += PAGE_SIZE)
                                                        })
                                                    >
                                                        "Next"
                                                    </Button>
                                                </div>
                                            </div>
                                        </div>
                                    }
                                        .into_any()
                                }
                                Err(e) => {
                                    view! {
                                        <div class="p-4">
                                            <p class="text-destructive text-sm">{e.to_string()}</p>
                                        </div>
                                    }
                                        .into_any()
                                }
                            })
                    }}
                </Suspense>
            </Card>

            <TaskCreateDialog
                open=dialog_open
                set_open=set_dialog_open
                on_create=Callback::new(move |req: CreateTaskRequest| {
                    create_action.dispatch(req);
                })
            />
        </div>
    }
}
