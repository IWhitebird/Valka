use leptos::prelude::*;

#[cfg(feature = "hydrate")]
use crate::api::types::RawTaskEvent;
use crate::api::types::TaskEvent;
use crate::components::ui::*;
use crate::utils::{status_dot_color, truncate_id};

#[cfg(feature = "hydrate")]
const MAX_EVENTS: usize = 200;

#[cfg(feature = "hydrate")]
fn status_map(code: i32) -> &'static str {
    match code {
        0 | 1 => "PENDING",
        2 => "DISPATCHING",
        3 => "RUNNING",
        4 => "COMPLETED",
        5 => "FAILED",
        6 => "RETRY",
        7 => "DEAD_LETTER",
        8 => "CANCELLED",
        _ => "PENDING",
    }
}

#[cfg(feature = "hydrate")]
fn parse_raw_event(raw: RawTaskEvent) -> TaskEvent {
    let ts_secs = raw.timestamp_ms / 1000;
    let ts_nanos = ((raw.timestamp_ms % 1000) * 1_000_000) as u32;
    let timestamp = chrono::DateTime::from_timestamp(ts_secs, ts_nanos)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    TaskEvent {
        event_id: raw.event_id,
        task_id: raw.task_id,
        queue_name: raw.queue_name,
        status: status_map(raw.new_status).to_string(),
        timestamp,
    }
}

#[component]
fn ConnectionIndicator(connected: ReadSignal<bool>) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2">
            <span class="relative flex h-2 w-2">
                <Show when=move || connected.get()>
                    <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-75" />
                </Show>
                <span class=move || {
                    if connected.get() {
                        "relative inline-flex h-2 w-2 rounded-full bg-emerald-400"
                    } else {
                        "relative inline-flex h-2 w-2 rounded-full bg-zinc-500"
                    }
                } />
            </span>
            <span class=move || {
                if connected.get() {
                    "text-xs font-medium text-emerald-400"
                } else {
                    "text-xs font-medium text-muted-foreground"
                }
            }>
                {move || if connected.get() { "Live" } else { "Disconnected" }}
            </span>
        </div>
    }
}

#[component]
fn EventRow(event: TaskEvent) -> impl IntoView {
    let dot_color = status_dot_color(&event.status);
    let status_cls = crate::utils::status_color(&event.status);
    let time_str = event
        .timestamp
        .get(11..19)
        .unwrap_or(&event.timestamp)
        .to_string();
    let status = event.status.clone();
    let queue_name = event.queue_name.clone();
    let task_id = truncate_id(&event.task_id, 8);

    view! {
        <div class="flex items-center gap-3 px-4 py-2.5 text-sm transition-colors hover:bg-accent/50">
            <span class=format!("h-2 w-2 shrink-0 rounded-full {dot_color}") />
            <span class="w-20 shrink-0 font-mono text-xs text-muted-foreground">
                {time_str}
            </span>
            <span class=format!(
                "inline-flex shrink-0 items-center rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase {status_cls}"
            )>
                {status}
            </span>
            <span class="shrink-0 text-xs text-muted-foreground">
                {queue_name}
            </span>
            <span class="ml-auto shrink-0 font-mono text-xs text-muted-foreground">
                {task_id}
            </span>
        </div>
    }
}

#[component]
pub fn EventStream() -> impl IntoView {
    let (events, set_events) = signal(Vec::<TaskEvent>::new());
    let (connected, _set_connected) = signal(false);

    #[cfg(feature = "hydrate")]
    let set_connected = _set_connected;

    // Start SSE on client only
    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;
        use wasm_bindgen::JsCast;

        let api_url = Resource::new(
            || (),
            |_| async { crate::server_fns::config::get_api_url().await.unwrap_or_default() },
        );

        Effect::new(move || {
            let Some(base_url) = api_url.get() else {
                return;
            };
            let url = format!("{base_url}/api/v1/events");
            let Ok(es) = web_sys::EventSource::new(&url) else {
                return;
            };

            let on_message =
                Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                    if let Some(data) = e.data().as_string() {
                        if let Ok(raw) = serde_json::from_str::<RawTaskEvent>(&data) {
                            let event = parse_raw_event(raw);
                            set_connected.set(true);
                            set_events.update(|evts| {
                                evts.insert(0, event);
                                if evts.len() > MAX_EVENTS {
                                    evts.truncate(MAX_EVENTS);
                                }
                            });
                        }
                    }
                });

            es.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            on_message.forget();

            let on_error = Closure::<dyn Fn()>::new(move || {
                set_connected.set(false);
            });
            es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            on_error.forget();
        });
    }

    let clear = move |_: leptos::ev::MouseEvent| {
        set_events.set(Vec::new());
    };

    view! {
        <Card class="gap-0 py-0">
            <CardHeader>
                <div class="flex items-center justify-between w-full py-4">
                    <div class="flex items-center gap-3">
                        <h3 class="flex items-center gap-2 text-base font-medium text-muted-foreground">
                            <svg xmlns="http://www.w3.org/2000/svg" class="h-4 w-4 text-muted-foreground" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M12 12m-9 0a9 9 0 1 0 18 0a9 9 0 1 0 -18 0" />
                                <path d="M12 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
                                <path d="M12 3v4" />
                                <path d="M12 17v4" />
                                <path d="M3 12h4" />
                                <path d="M17 12h4" />
                            </svg>
                            "Event Stream"
                        </h3>
                        <ConnectionIndicator connected />
                        <span class="text-xs tabular-nums text-muted-foreground">
                            {move || {
                                let count = events.get().len();
                                if count == 1 { "1 event".to_string() }
                                else { format!("{count} events") }
                            }}
                        </span>
                    </div>
                    <Button variant=ButtonVariant::Ghost on_click=Callback::new(clear)>
                        "Clear"
                    </Button>
                </div>
            </CardHeader>
            <CardContent>
                <div class="max-h-[calc(100vh-280px)] overflow-auto">
                    {move || {
                        let evts = events.get();
                        if evts.is_empty() {
                            view! {
                                <div class="flex h-48 flex-col items-center justify-center gap-2 text-sm text-muted-foreground">
                                    <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5 opacity-40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M12 12m-9 0a9 9 0 1 0 18 0a9 9 0 1 0 -18 0" />
                                        <path d="M12 12m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
                                        <path d="M12 3v4" />
                                        <path d="M12 17v4" />
                                        <path d="M3 12h4" />
                                        <path d="M17 12h4" />
                                    </svg>
                                    "Waiting for events..."
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="divide-y divide-border">
                                    {evts.into_iter().map(|event| {
                                        view! {
                                            <EventRow event />
                                        }
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </CardContent>
        </Card>
    }
}
