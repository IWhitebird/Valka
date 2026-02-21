use leptos::prelude::*;

use crate::api::types::Worker;
use crate::components::ui::*;

#[component]
pub fn WorkerTable(workers: Vec<Worker>) -> impl IntoView {
    view! {
        <Table>
            <Thead>
                <Tr>
                    <Th>"ID"</Th>
                    <Th>"Name"</Th>
                    <Th>"Queues"</Th>
                    <Th>"Concurrency"</Th>
                    <Th>"Active Tasks"</Th>
                    <Th>"Status"</Th>
                    <Th>"Last Heartbeat"</Th>
                    <Th>"Connected At"</Th>
                </Tr>
            </Thead>
            <Tbody>
                {if workers.is_empty() {
                    view! {
                        <Tr>
                            <Td class="text-center text-muted-foreground py-8".to_string()>
                                "No workers connected"
                            </Td>
                        </Tr>
                    }
                        .into_any()
                } else {
                    workers
                        .into_iter()
                        .map(|worker| {
                            let short_id = if worker.id.len() > 8 {
                                format!("{}...", &worker.id[..8])
                            } else {
                                worker.id.clone()
                            };
                            let queues_str = worker.queues.join(", ");
                            let status_cls = if worker.status == "ACTIVE" {
                                "text-success"
                            } else {
                                "text-muted-foreground"
                            };
                            view! {
                                <Tr>
                                    <Td class="font-mono text-xs".to_string()>
                                        <span title=worker.id.clone()>{short_id}</span>
                                    </Td>
                                    <Td>{worker.name}</Td>
                                    <Td class="text-xs".to_string()>{queues_str}</Td>
                                    <Td>{worker.concurrency.to_string()}</Td>
                                    <Td>{worker.active_tasks.to_string()}</Td>
                                    <Td>
                                        <span class=format!("text-sm font-medium {status_cls}")>
                                            {worker.status.clone()}
                                        </span>
                                    </Td>
                                    <Td class="text-xs text-muted-foreground".to_string()>
                                        {format_timestamp(&worker.last_heartbeat)}
                                    </Td>
                                    <Td class="text-xs text-muted-foreground".to_string()>
                                        {format_timestamp(&worker.connected_at)}
                                    </Td>
                                </Tr>
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
    if let Some(t) = ts.get(..19) {
        t.replace('T', " ")
    } else {
        ts.to_string()
    }
}
