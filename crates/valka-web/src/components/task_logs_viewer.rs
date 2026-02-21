use leptos::prelude::*;

use crate::components::ui::Skeleton;
use crate::server_fns::tasks::get_run_logs;

fn log_level_class(level: &str) -> &'static str {
    match level.to_uppercase().as_str() {
        "ERROR" => "text-red-400 border-red-500/30 bg-red-500/10",
        "WARN" | "WARNING" => "text-amber-400 border-amber-500/30 bg-amber-500/10",
        "INFO" => "text-blue-400 border-blue-500/30 bg-blue-500/10",
        "DEBUG" => "text-zinc-500 border-zinc-500/30 bg-zinc-500/10",
        _ => "text-zinc-400 border-zinc-500/30 bg-zinc-500/10",
    }
}

fn format_timestamp_ms(ms: i64) -> String {
    let secs = ms / 1000;
    let millis = ms % 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}.{millis:03}")
}

#[component]
pub fn TaskLogsViewer(task_id: String, run_id: String) -> impl IntoView {
    let tid = task_id.clone();
    let rid = run_id.clone();
    let logs = Resource::new(
        move || (tid.clone(), rid.clone()),
        move |(t, r)| async move { get_run_logs(t, r).await.unwrap_or_default() },
    );

    view! {
        <Suspense fallback=move || view! {
            <div class="flex h-48 flex-col gap-2 rounded-lg border border-border bg-zinc-950 p-4">
                <Skeleton class="h-4 w-3/4 bg-zinc-800" />
                <Skeleton class="h-4 w-1/2 bg-zinc-800" />
                <Skeleton class="h-4 w-2/3 bg-zinc-800" />
                <Skeleton class="h-4 w-1/3 bg-zinc-800" />
            </div>
        }>
            {move || {
                let log_list = logs.get().unwrap_or_default();
                if log_list.is_empty() {
                    view! {
                        <div class="flex h-48 flex-col items-center justify-center rounded-lg border border-border bg-zinc-950">
                            <svg xmlns="http://www.w3.org/2000/svg" class="h-8 w-8 text-muted-foreground/40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="4 17 10 11 4 5" />
                                <line x1="12" x2="20" y1="19" y2="19" />
                            </svg>
                            <p class="mt-2 text-sm text-muted-foreground">"No logs available"</p>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="h-96 overflow-auto rounded-lg border border-border bg-zinc-950">
                            <div class="p-4 font-mono text-xs">
                                {log_list.into_iter().map(|log| {
                                    let level_cls = log_level_class(&log.level);
                                    let ts = format_timestamp_ms(log.timestamp_ms);
                                    let level = log.level.clone();
                                    let msg = log.message.clone();
                                    view! {
                                        <div class="flex items-start gap-3 py-0.5 leading-5">
                                            <span class="shrink-0 tabular-nums text-zinc-600">
                                                {ts}
                                            </span>
                                            <span class=format!(
                                                "shrink-0 rounded border px-1.5 py-0 text-[10px] font-semibold uppercase {level_cls}"
                                            )>
                                                {level}
                                            </span>
                                            <span class="text-zinc-300">{msg}</span>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </Suspense>
    }
}
