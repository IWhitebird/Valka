use leptos::prelude::*;

use crate::api::types::DeadLetter;
use crate::components::ui::*;
use crate::server_fns::dead_letters::list_dead_letters;
use crate::utils::{format_date, truncate_id};

const PAGE_SIZE: i64 = 25;

#[component]
fn DeadLetterExpandedRow(dl: DeadLetter) -> impl IntoView {
    let input_str = dl
        .input
        .as_ref()
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
        .unwrap_or_else(|| "null".to_string());
    let metadata_str = dl
        .metadata
        .as_ref()
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
        .unwrap_or_else(|| "null".to_string());

    view! {
        <tr class="hover:bg-transparent">
            <td colspan="7" class="bg-muted/30 px-8 py-4">
                <div class="grid gap-4 lg:grid-cols-2">
                    <div>
                        <p class="mb-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                            "Input"
                        </p>
                        <pre class="overflow-x-auto rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground">
                            {input_str}
                        </pre>
                    </div>
                    <div>
                        <p class="mb-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">
                            "Metadata"
                        </p>
                        <pre class="overflow-x-auto rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground">
                            {metadata_str}
                        </pre>
                    </div>
                </div>
            </td>
        </tr>
    }
}

#[component]
pub fn DeadLettersPage() -> impl IntoView {
    let (offset, set_offset) = signal(0i64);
    let (queue_filter, set_queue_filter) = signal(String::new());
    let (expanded_id, set_expanded_id) = signal(Option::<i64>::None);

    let dead_letters = Resource::new(
        move || (queue_filter.get(), offset.get()),
        move |(q, o)| async move {
            let queue = if q.is_empty() { None } else { Some(q) };
            list_dead_letters(queue, Some(PAGE_SIZE), Some(o))
                .await
                .unwrap_or_default()
        },
    );

    let refetch = move |_: leptos::ev::MouseEvent| dead_letters.refetch();

    view! {
        <div class="space-y-6">
            <div class="flex items-center justify-between">
                <div>
                    <h1 class="text-2xl font-semibold tracking-tight text-foreground">
                        "Dead Letters"
                    </h1>
                    <p class="mt-1 text-sm text-muted-foreground">
                        "Tasks that have exhausted all retry attempts"
                    </p>
                </div>
                <Button variant=ButtonVariant::Outline on_click=Callback::new(refetch)>
                    "Refresh"
                </Button>
            </div>

            <div class="flex items-center gap-3">
                <Input
                    placeholder="Filter by queue..."
                    value=queue_filter.get_untracked()
                    on_input=Callback::new(move |v: String| {
                        set_queue_filter.set(v);
                        set_offset.set(0);
                    })
                    class="max-w-xs"
                />
            </div>

            <Suspense fallback=move || view! {
                <div class="flex h-64 items-center justify-center text-sm text-muted-foreground">
                    "Loading..."
                </div>
            }>
                {move || {
                    let dl_list = dead_letters.get().unwrap_or_default();
                    if dl_list.is_empty() {
                        view! {
                            <div class="flex h-64 flex-col items-center justify-center rounded-lg border">
                                <div class="mb-3 flex h-10 w-10 items-center justify-center rounded-lg bg-muted">
                                    <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5 text-muted-foreground" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <circle cx="9" cy="12" r="1" />
                                        <circle cx="15" cy="12" r="1" />
                                        <path d="M8 20v2h8v-2" />
                                        <path d="m12.5 17-.5-1-.5 1h1z" />
                                        <path d="M16 20a2 2 0 0 0 1.56-3.25 8 8 0 1 0-11.12 0A2 2 0 0 0 8 20" />
                                    </svg>
                                </div>
                                <p class="text-sm text-muted-foreground">"No dead letters"</p>
                                <p class="mt-1 text-xs text-muted-foreground/60">
                                    "Tasks that fail all retries will appear here"
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        let has_more = dl_list.len() as i64 >= PAGE_SIZE;
                        let current_offset = offset.get();
                        let count = dl_list.len();
                        view! {
                            <div>
                                <div class="rounded-lg border">
                                    <Table>
                                        <Thead>
                                            <Tr class="hover:bg-transparent">
                                                <Th>" "</Th>
                                                <Th>"Task ID"</Th>
                                                <Th>"Queue"</Th>
                                                <Th>"Task Name"</Th>
                                                <Th>"Attempts"</Th>
                                                <Th>"Error"</Th>
                                                <Th>"Failed At"</Th>
                                            </Tr>
                                        </Thead>
                                        <Tbody>
                                            {dl_list.into_iter().map(|dl| {
                                                let dl_id = dl.id;
                                                let dl_clone = dl.clone();
                                                let task_id_display = truncate_id(&dl.task_id, 8);
                                                let task_link = format!("/tasks/{}", dl.task_id);
                                                let task_link2 = task_link.clone();
                                                let task_link3 = task_link.clone();
                                                let task_link4 = task_link.clone();
                                                let task_link5 = task_link.clone();
                                                let queue_name = dl.queue_name.clone();
                                                let task_name = dl.task_name.clone();
                                                let attempt_count = dl.attempt_count.to_string();
                                                let error = dl.error_message.clone().unwrap_or_else(|| "--".to_string());
                                                let created = format_date(&dl.created_at);
                                                view! {
                                                    <Tr class="cursor-pointer">
                                                        <td class="w-8 px-2 py-3 align-middle">
                                                            <button
                                                                class="inline-flex items-center justify-center h-6 w-6 rounded hover:bg-accent"
                                                                on:click=move |e: leptos::ev::MouseEvent| {
                                                                    e.stop_propagation();
                                                                    set_expanded_id.update(|v| {
                                                                        *v = if *v == Some(dl_id) { None } else { Some(dl_id) };
                                                                    });
                                                                }
                                                            >
                                                                <svg
                                                                    xmlns="http://www.w3.org/2000/svg"
                                                                    class=move || {
                                                                        let base = "h-3 w-3 transition-transform";
                                                                        if expanded_id.get() == Some(dl_id) {
                                                                            format!("{base} rotate-180")
                                                                        } else {
                                                                            base.to_string()
                                                                        }
                                                                    }
                                                                    viewBox="0 0 24 24"
                                                                    fill="none"
                                                                    stroke="currentColor"
                                                                    stroke-width="2"
                                                                    stroke-linecap="round"
                                                                    stroke-linejoin="round"
                                                                >
                                                                    <path d="m6 9 6 6 6-6" />
                                                                </svg>
                                                            </button>
                                                        </td>
                                                        <td
                                                            class="px-4 py-3 align-middle font-mono text-xs text-muted-foreground cursor-pointer"
                                                            on:click=move |_| {
                                                                let _ = leptos_router::hooks::use_navigate()(&task_link, Default::default());
                                                            }
                                                        >
                                                            {task_id_display}
                                                        </td>
                                                        <td
                                                            class="px-4 py-3 align-middle text-foreground cursor-pointer"
                                                            on:click=move |_| {
                                                                let _ = leptos_router::hooks::use_navigate()(&task_link2, Default::default());
                                                            }
                                                        >
                                                            {queue_name}
                                                        </td>
                                                        <td
                                                            class="px-4 py-3 align-middle font-medium text-foreground cursor-pointer"
                                                            on:click=move |_| {
                                                                let _ = leptos_router::hooks::use_navigate()(&task_link3, Default::default());
                                                            }
                                                        >
                                                            {task_name}
                                                        </td>
                                                        <td
                                                            class="px-4 py-3 align-middle text-muted-foreground cursor-pointer"
                                                            on:click=move |_| {
                                                                let _ = leptos_router::hooks::use_navigate()(&task_link4, Default::default());
                                                            }
                                                        >
                                                            {attempt_count}
                                                        </td>
                                                        <td
                                                            class="px-4 py-3 align-middle max-w-xs truncate text-xs text-red-400 cursor-pointer"
                                                            on:click=move |_| {
                                                                let _ = leptos_router::hooks::use_navigate()(&task_link5, Default::default());
                                                            }
                                                        >
                                                            {error}
                                                        </td>
                                                        <Td class="text-xs text-muted-foreground">
                                                            {created}
                                                        </Td>
                                                    </Tr>
                                                    <Show when=move || expanded_id.get() == Some(dl_id)>
                                                        <DeadLetterExpandedRow dl=dl_clone.clone() />
                                                    </Show>
                                                }
                                            }).collect_view()}
                                        </Tbody>
                                    </Table>
                                </div>

                                <div class="mt-4 flex items-center justify-between">
                                    <p class="text-xs text-muted-foreground">
                                        {format!("Showing {} - {}", current_offset + 1, current_offset + count as i64)}
                                    </p>
                                    <div class="flex items-center gap-2">
                                        <Button
                                            variant=ButtonVariant::Outline
                                            disabled=Signal::derive(move || offset.get() == 0)
                                            on_click=Callback::new(move |_: leptos::ev::MouseEvent| {
                                                set_offset.update(|o| *o = (*o - PAGE_SIZE).max(0));
                                            })
                                        >
                                            "Previous"
                                        </Button>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            disabled=Signal::derive(move || !has_more)
                                            on_click=Callback::new(move |_: leptos::ev::MouseEvent| {
                                                set_offset.update(|o| *o += PAGE_SIZE);
                                            })
                                        >
                                            "Next"
                                        </Button>
                                    </div>
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </Suspense>
        </div>
    }
}
