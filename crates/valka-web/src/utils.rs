use chrono::NaiveDateTime;

pub fn format_date(iso: &str) -> String {
    if let Ok(dt) = NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S%.f") {
        dt.format("%b %-d, %Y %H:%M:%S").to_string()
    } else if let Ok(dt) = NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S") {
        dt.format("%b %-d, %Y %H:%M:%S").to_string()
    } else {
        iso.get(..19).unwrap_or(iso).replace('T', " ")
    }
}

pub fn format_relative(iso: &str) -> String {
    let Ok(dt) = NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S"))
    else {
        return iso.to_string();
    };
    let now = chrono::Utc::now().naive_utc();
    let diff = now.signed_duration_since(dt);
    let secs = diff.num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = diff.num_minutes();
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = diff.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = diff.num_days();
    format!("{days}d ago")
}

pub fn truncate_id(id: &str, len: usize) -> String {
    if id.len() <= len {
        id.to_string()
    } else {
        format!("{}...", &id[..len])
    }
}

pub fn status_color(status: &str) -> &'static str {
    match status {
        "PENDING" => "bg-zinc-500/10 text-zinc-400 border-zinc-500/20",
        "DISPATCHING" => "bg-blue-500/10 text-blue-400 border-blue-500/20",
        "RUNNING" => "bg-sky-500/10 text-sky-400 border-sky-500/20",
        "COMPLETED" => "bg-emerald-500/10 text-emerald-400 border-emerald-500/20",
        "FAILED" => "bg-red-500/10 text-red-400 border-red-500/20",
        "RETRY" => "bg-amber-500/10 text-amber-400 border-amber-500/20",
        "DEAD_LETTER" => "bg-rose-500/10 text-rose-400 border-rose-500/20",
        "CANCELLED" => "bg-neutral-500/10 text-neutral-400 border-neutral-500/20",
        _ => "bg-zinc-500/10 text-zinc-400 border-zinc-500/20",
    }
}

pub fn status_dot_color(status: &str) -> &'static str {
    match status {
        "PENDING" => "bg-zinc-400",
        "DISPATCHING" => "bg-blue-400",
        "RUNNING" => "bg-sky-400",
        "COMPLETED" => "bg-emerald-400",
        "FAILED" => "bg-red-400",
        "RETRY" => "bg-amber-400",
        "DEAD_LETTER" => "bg-rose-400",
        "CANCELLED" => "bg-neutral-400",
        _ => "bg-zinc-400",
    }
}
