use leptos::prelude::*;

use crate::components::ui::{Badge, BadgeVariant};

#[component]
pub fn TaskStatusBadge(status: String) -> impl IntoView {
    let variant = match status.as_str() {
        "COMPLETED" => BadgeVariant::Success,
        "RUNNING" | "DISPATCHING" => BadgeVariant::Info,
        "PENDING" => BadgeVariant::Default,
        "RETRY" => BadgeVariant::Warning,
        "FAILED" | "DEAD_LETTER" => BadgeVariant::Destructive,
        "CANCELLED" => BadgeVariant::Secondary,
        _ => BadgeVariant::Secondary,
    };
    view! { <Badge variant=variant>{status}</Badge> }
}
