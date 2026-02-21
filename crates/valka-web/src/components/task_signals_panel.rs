use leptos::prelude::*;

use crate::api::types::{SendSignalRequest, TaskSignal};
use crate::components::ui::*;
use crate::server_fns::signals::{list_signals, send_signal};
use crate::utils::{format_date, truncate_id};

fn signal_status_class(status: &str) -> &'static str {
    match status {
        "PENDING" => "bg-zinc-500/10 text-zinc-400 border-zinc-500/20",
        "DELIVERED" => "bg-blue-500/10 text-blue-400 border-blue-500/20",
        "ACKNOWLEDGED" => "bg-emerald-500/10 text-emerald-400 border-emerald-500/20",
        _ => "bg-zinc-500/10 text-zinc-400 border-zinc-500/20",
    }
}

fn signal_status_icon(status: &str) -> &'static str {
    match status {
        "PENDING" => "\u{23f3}",
        "DELIVERED" => "\u{1f4e8}",
        "ACKNOWLEDGED" => "\u{2705}",
        _ => "\u{23f3}",
    }
}

#[component]
pub fn TaskSignalsPanel(task_id: String, task_status: String) -> impl IntoView {
    let (signal_name, set_signal_name) = signal(String::new());
    let (payload, set_payload) = signal(String::new());
    let (payload_error, set_payload_error) = signal(Option::<String>::None);

    let tid = task_id.clone();
    let signals_resource = Resource::new(
        move || tid.clone(),
        move |t| async move { list_signals(t).await.unwrap_or_default() },
    );

    let tid2 = task_id.clone();
    let send_action = Action::new(move |req: &SendSignalRequest| {
        let t = tid2.clone();
        let r = req.clone();
        async move { send_signal(t, r).await }
    });

    // Refetch after sending
    Effect::new(move || {
        send_action.value().get();
        signals_resource.refetch();
    });

    let can_send = matches!(
        task_status.as_str(),
        "PENDING" | "DISPATCHING" | "RUNNING" | "RETRY"
    );

    let handle_send = move |_: leptos::ev::MouseEvent| {
        let name = signal_name.get();
        if name.trim().is_empty() {
            return;
        }
        let payload_str = payload.get();
        let parsed_payload = if payload_str.trim().is_empty() {
            None
        } else {
            match serde_json::from_str::<serde_json::Value>(&payload_str) {
                Ok(v) => {
                    set_payload_error.set(None);
                    Some(v)
                }
                Err(_) => {
                    set_payload_error.set(Some("Invalid JSON".to_string()));
                    return;
                }
            }
        };

        send_action.dispatch(SendSignalRequest {
            signal_name: name.trim().to_string(),
            payload: parsed_payload,
        });
        set_signal_name.set(String::new());
        set_payload.set(String::new());
        set_payload_error.set(None);
    };

    let payload_error_cls = move || {
        if payload_error.get().is_some() {
            "border-red-500 font-mono text-xs".to_string()
        } else {
            "font-mono text-xs".to_string()
        }
    };

    view! {
        <div class="space-y-4">
            <h3 class="text-lg font-semibold text-foreground">"Signals"</h3>

            // Send Signal Form
            {if can_send {
                Some(view! {
                    <Card>
                        <CardHeader>
                            <CardTitle>"Send Signal"</CardTitle>
                        </CardHeader>
                        <CardContent>
                            <div class="flex flex-col gap-3 sm:flex-row sm:items-end">
                                <div class="flex-1 space-y-1.5">
                                    <label class="text-xs text-muted-foreground">"Signal Name"</label>
                                    <Input
                                        placeholder="e.g. pause, resume, shutdown"
                                        value=signal_name.get_untracked()
                                        on_input=Callback::new(move |v: String| set_signal_name.set(v))
                                    />
                                </div>
                                <div class="flex-1 space-y-1.5">
                                    <label class="text-xs text-muted-foreground">"Payload (JSON, optional)"</label>
                                    <Textarea
                                        placeholder=r#"{"key": "value"}"#
                                        value=payload.get_untracked()
                                        on_input=Callback::new(move |v: String| {
                                            set_payload.set(v);
                                            set_payload_error.set(None);
                                        })
                                        class=Signal::derive(payload_error_cls)
                                    />
                                    {move || payload_error.get().map(|e| view! {
                                        <p class="text-xs text-red-400">{e}</p>
                                    })}
                                </div>
                                <Button on_click=Callback::new(handle_send)>
                                    "Send"
                                </Button>
                            </div>
                        </CardContent>
                    </Card>
                })
            } else {
                None
            }}

            // Signals Table
            <Suspense fallback=move || view! {
                <div class="flex h-32 items-center justify-center text-sm text-muted-foreground">
                    "Loading signals..."
                </div>
            }>
                {move || {
                    let signal_list = signals_resource.get().unwrap_or_default();
                    if signal_list.is_empty() {
                        view! {
                            <div class="flex h-32 flex-col items-center justify-center rounded-lg border bg-card">
                                <svg xmlns="http://www.w3.org/2000/svg" class="h-8 w-8 text-muted-foreground/50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M12 12m-9 0a9 9 0 1 0 18 0a9 9 0 1 0 -18 0" />
                                    <path d="M12 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
                                    <path d="M12 3v4" />
                                    <path d="M12 17v4" />
                                    <path d="M3 12h4" />
                                    <path d="M17 12h4" />
                                </svg>
                                <p class="mt-2 text-sm text-muted-foreground">"No signals sent"</p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="overflow-hidden rounded-lg border">
                                <Table>
                                    <Thead>
                                        <Tr class="hover:bg-transparent">
                                            <Th>"ID"</Th>
                                            <Th>"Name"</Th>
                                            <Th>"Status"</Th>
                                            <Th>"Payload"</Th>
                                            <Th>"Created"</Th>
                                            <Th>"Delivered"</Th>
                                            <Th>"Acknowledged"</Th>
                                        </Tr>
                                    </Thead>
                                    <Tbody>
                                        {signal_list.into_iter().map(|sig: TaskSignal| {
                                            let status_cls = signal_status_class(&sig.status);
                                            let status_icon = signal_status_icon(&sig.status);
                                            let sig_id = truncate_id(&sig.id, 8);
                                            let sig_name = sig.signal_name.clone();
                                            let sig_status = sig.status.clone();
                                            let payload_str = sig.payload.as_ref().map(|p| p.to_string()).unwrap_or_else(|| "--".to_string());
                                            let created = format_date(&sig.created_at);
                                            let delivered = sig.delivered_at.as_deref().map(format_date).unwrap_or_else(|| "--".to_string());
                                            let acknowledged = sig.acknowledged_at.as_deref().map(format_date).unwrap_or_else(|| "--".to_string());
                                            view! {
                                                <Tr>
                                                    <Td class="font-mono text-xs text-muted-foreground">
                                                        {sig_id}
                                                    </Td>
                                                    <Td class="font-medium text-foreground">
                                                        {sig_name}
                                                    </Td>
                                                    <Td>
                                                        <span class=format!(
                                                            "inline-flex items-center gap-1 rounded-full border px-2.5 py-0.5 text-xs font-medium {status_cls}"
                                                        )>
                                                            {status_icon}
                                                            " "
                                                            {sig_status}
                                                        </span>
                                                    </Td>
                                                    <Td class="max-w-[200px] truncate font-mono text-xs text-muted-foreground">
                                                        {payload_str}
                                                    </Td>
                                                    <Td class="text-xs text-muted-foreground">
                                                        {created}
                                                    </Td>
                                                    <Td class="text-xs text-muted-foreground">
                                                        {delivered}
                                                    </Td>
                                                    <Td class="text-xs text-muted-foreground">
                                                        {acknowledged}
                                                    </Td>
                                                </Tr>
                                            }
                                        }).collect_view()}
                                    </Tbody>
                                </Table>
                            </div>
                        }.into_any()
                    }
                }}
            </Suspense>
        </div>
    }
}
