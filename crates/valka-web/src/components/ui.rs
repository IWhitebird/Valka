use leptos::ev;
use leptos::prelude::*;

// ─── Card ────────────────────────────────────────────────────────────

#[component]
pub fn Card(
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let cls = format!("rounded-lg border border-border bg-card text-card-foreground p-6 {class}");
    view! { <div class=cls>{children()}</div> }
}

#[component]
pub fn CardHeader(children: Children) -> impl IntoView {
    view! { <div class="flex flex-col space-y-1.5 pb-4">{children()}</div> }
}

#[component]
pub fn CardTitle(children: Children) -> impl IntoView {
    view! { <h3 class="text-sm font-medium text-muted-foreground">{children()}</h3> }
}

#[component]
pub fn CardContent(children: Children) -> impl IntoView {
    view! { <div>{children()}</div> }
}

// ─── Button ──────────────────────────────────────────────────────────

#[derive(Clone, Default, PartialEq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Outline,
    Ghost,
    Destructive,
}

#[component]
pub fn Button(
    #[prop(optional)] variant: ButtonVariant,
    #[prop(optional, into)] class: String,
    #[prop(optional)] disabled: Option<Signal<bool>>,
    #[prop(optional)] on_click: Option<Callback<ev::MouseEvent>>,
    children: Children,
) -> impl IntoView {
    let base = "inline-flex items-center justify-center gap-2 rounded-md text-sm font-medium \
                transition-colors focus-visible:outline-none focus-visible:ring-2 \
                focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 \
                px-4 py-2";
    let variant_cls = match variant {
        ButtonVariant::Default => "bg-primary text-primary-foreground hover:bg-primary/90",
        ButtonVariant::Outline => {
            "border border-border bg-transparent hover:bg-accent hover:text-accent-foreground"
        }
        ButtonVariant::Ghost => "hover:bg-accent hover:text-accent-foreground",
        ButtonVariant::Destructive => {
            "bg-destructive text-destructive-foreground hover:bg-destructive/90"
        }
    };
    let cls = format!("{base} {variant_cls} {class}");
    let is_disabled = disabled.unwrap_or_else(|| Signal::derive(|| false));

    let click_handler = move |e: ev::MouseEvent| {
        if let Some(ref cb) = on_click {
            Callback::run(cb, e);
        }
    };

    view! {
        <button class=cls disabled=move || is_disabled.get() on:click=click_handler>
            {children()}
        </button>
    }
}

// ─── Badge ───────────────────────────────────────────────────────────

#[derive(Clone, Default, PartialEq)]
pub enum BadgeVariant {
    #[default]
    Default,
    Success,
    Warning,
    Destructive,
    Info,
    Secondary,
}

#[component]
pub fn Badge(
    #[prop(optional)] variant: BadgeVariant,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let base = "inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium";
    let variant_cls = match variant {
        BadgeVariant::Default => "bg-primary/10 text-primary",
        BadgeVariant::Success => "bg-success/10 text-success",
        BadgeVariant::Warning => "bg-warning/10 text-warning",
        BadgeVariant::Destructive => "bg-destructive/10 text-destructive",
        BadgeVariant::Info => "bg-info/10 text-info",
        BadgeVariant::Secondary => "bg-secondary text-secondary-foreground",
    };
    let cls = format!("{base} {variant_cls} {class}");
    view! { <span class=cls>{children()}</span> }
}

// ─── Table ───────────────────────────────────────────────────────────

#[component]
pub fn Table(children: Children) -> impl IntoView {
    view! {
        <div class="w-full overflow-auto">
            <table class="w-full caption-bottom text-sm">{children()}</table>
        </div>
    }
}

#[component]
pub fn Thead(children: Children) -> impl IntoView {
    view! { <thead class="border-b border-border">{children()}</thead> }
}

#[component]
pub fn Tbody(children: Children) -> impl IntoView {
    view! { <tbody class="divide-y divide-border">{children()}</tbody> }
}

#[component]
pub fn Tr(
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let cls = format!(
        "transition-colors hover:bg-muted/50 {class}"
    );
    view! { <tr class=cls>{children()}</tr> }
}

#[component]
pub fn Th(children: Children) -> impl IntoView {
    view! {
        <th class="h-10 px-4 text-left align-middle font-medium text-muted-foreground text-xs uppercase tracking-wider">
            {children()}
        </th>
    }
}

#[component]
pub fn Td(
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    let cls = format!("px-4 py-3 align-middle {class}");
    view! { <td class=cls>{children()}</td> }
}

// ─── Input ───────────────────────────────────────────────────────────

#[component]
pub fn Input(
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] class: String,
    #[prop(optional, into)] value: String,
    #[prop(optional)] on_input: Option<Callback<String>>,
) -> impl IntoView {
    let cls = format!(
        "flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm \
         text-foreground placeholder:text-muted-foreground focus-visible:outline-none \
         focus-visible:ring-2 focus-visible:ring-ring {class}"
    );
    view! {
        <input
            type="text"
            class=cls
            placeholder=placeholder
            value=value
            on:input=move |e| {
                if let Some(cb) = &on_input {
                    cb.run(event_target_value(&e));
                }
            }
        />
    }
}

// ─── Textarea ────────────────────────────────────────────────────────

#[component]
pub fn Textarea(
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] class: Signal<String>,
    #[prop(optional, into)] value: String,
    #[prop(optional)] on_input: Option<Callback<String>>,
) -> impl IntoView {
    let base = "flex w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm \
         text-foreground placeholder:text-muted-foreground focus-visible:outline-none \
         focus-visible:ring-2 focus-visible:ring-ring resize-none";
    view! {
        <textarea
            class=move || format!("{base} {}", class.get())
            placeholder=placeholder
            rows="2"
            on:input=move |e| {
                if let Some(cb) = &on_input {
                    cb.run(event_target_value(&e));
                }
            }
        >
            {value}
        </textarea>
    }
}

// ─── Select ──────────────────────────────────────────────────────────

#[component]
pub fn Select(
    #[prop(optional, into)] class: String,
    #[prop(optional, into)] value: String,
    #[prop(optional)] on_change: Option<Callback<String>>,
    children: Children,
) -> impl IntoView {
    let cls = format!(
        "flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm \
         text-foreground focus-visible:outline-none focus-visible:ring-2 \
         focus-visible:ring-ring {class}"
    );
    view! {
        <select
            class=cls
            prop:value=value
            on:change=move |e| {
                if let Some(cb) = &on_change {
                    cb.run(event_target_value(&e));
                }
            }
        >
            {children()}
        </select>
    }
}

// ─── Skeleton ────────────────────────────────────────────────────────

#[component]
pub fn Skeleton(
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    let cls = format!("animate-pulse rounded-md bg-muted {class}");
    view! { <div class=cls></div> }
}

// ─── Separator ───────────────────────────────────────────────────────

#[component]
pub fn Separator() -> impl IntoView {
    view! { <hr class="border-border" /> }
}
