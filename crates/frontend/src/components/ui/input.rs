use dioxus::prelude::*;

#[derive(Clone)]
struct FieldId(String);

thread_local! { static DOM_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
pub(super) fn use_dom_id(prefix: &'static str) -> String {
    use_hook(move || {
        DOM_ID.with(|counter| {
            let next = counter
                .get()
                .checked_add(1)
                .expect("DOM ID space exhausted");
            counter.set(next);
            format!("hg-{prefix}-{next}")
        })
    })
}

fn field_id() -> String {
    try_consume_context::<FieldId>()
        .map(|field| field.0)
        .unwrap_or_default()
}

/// Associates sibling Label and Input controls without requiring callers to invent IDs.
#[component]
pub fn Field(
    #[props(default)] class: String,
    #[props(default)] hidden: bool,
    children: Element,
) -> Element {
    let id = use_dom_id("field");
    use_context_provider(move || FieldId(id));
    rsx! { div { class, hidden, {children} } }
}

#[component]
pub fn Input(
    #[props(default = "text".to_string())] r#type: String,
    #[props(default)] class: String,
    #[props(default)] placeholder: String,
    #[props(default)] value: String,
    #[props(default)] disabled: bool,
    #[props(default)] required: bool,
    #[props(default)] id: String,
    #[props(default)] name: String,
    #[props(default)] autocomplete: String,
    #[props(default)] aria_label: String,
    #[props(default)] aria_describedby: String,
    #[props(default)] aria_invalid: bool,
    #[props(default)] oninput: EventHandler<FormEvent>,
) -> Element {
    let field = try_consume_context::<FieldId>();
    let id = if id.is_empty() {
        field.map(|field| field.0).unwrap_or_default()
    } else {
        id
    };
    let aria_label = if aria_label.is_empty() && id.is_empty() {
        placeholder.clone()
    } else {
        aria_label
    };
    let is_password = r#type == "password";
    let mut password_visible = use_signal(|| false);
    let visible = password_visible();
    let input_type = if is_password && visible {
        "text".to_string()
    } else {
        r#type
    };
    let accessible_label = if visible {
        "Hide password"
    } else {
        "Show password"
    };
    let password_padding = if is_password { "pr-11" } else { "" };

    rsx! {
        div { class: if is_password { "relative" } else { "contents" },
            input {
                r#type: input_type,
                id,
                name,
                autocomplete,
                aria_label,
                aria_describedby,
                aria_invalid,
                class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50 touch-target {class} {password_padding}",
                placeholder,
                value,
                disabled,
                required,
                oninput: move |evt| oninput.call(evt),
            }
            if is_password {
                button {
                    class: "absolute right-1 top-1 inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    r#type: "button",
                    aria_label: accessible_label,
                    aria_pressed: visible,
                    title: accessible_label,
                    onclick: move |_| password_visible.toggle(),
                    if visible {
                        svg { xmlns: "http://www.w3.org/2000/svg", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2",
                            path { d: "M3 3l18 18" }
                            path { d: "M10.6 10.6a2 2 0 002.8 2.8" }
                            path { d: "M9.9 4.2A10.5 10.5 0 0112 4c5 0 9 5 9 8a9.8 9.8 0 01-2 3.2" }
                            path { d: "M6.6 6.6C4.4 8 3 10.2 3 12c0 3 4 8 9 8a9.8 9.8 0 004.2-.9" }
                        }
                    } else {
                        svg { xmlns: "http://www.w3.org/2000/svg", width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2",
                            path { d: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" }
                            circle { cx: "12", cy: "12", r: "3" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn Label(
    #[props(default)] class: String,
    #[props(default)] r#for: String,
    children: Element,
) -> Element {
    let r#for = if r#for.is_empty() {
        try_consume_context::<FieldId>()
            .map(|field| field.0)
            .unwrap_or_default()
    } else {
        r#for
    };
    rsx! {
        label {
            r#for,
            class: "text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70 {class}",
            {children}
        }
    }
}

/// Native select with the same label association as Input.
#[component]
pub fn Select(
    #[props(default)] class: String,
    #[props(default)] value: Option<String>,
    #[props(default)] disabled: bool,
    #[props(default)] aria_label: String,
    #[props(default)] onchange: EventHandler<FormEvent>,
    children: Element,
) -> Element {
    let id = field_id();
    rsx! { select { id, class, value, disabled, aria_label, onchange: move |event| onchange.call(event), {children} } }
}

#[component]
pub fn Textarea(
    #[props(default)] class: String,
    #[props(default)] value: String,
    #[props(default)] placeholder: String,
    #[props(default)] disabled: bool,
    #[props(default)] aria_label: String,
    #[props(default)] oninput: EventHandler<FormEvent>,
) -> Element {
    let id = field_id();
    let aria_label = if aria_label.is_empty() && id.is_empty() {
        placeholder.clone()
    } else {
        aria_label
    };
    rsx! { textarea { id, class, value, placeholder, disabled, aria_label, oninput: move |event| oninput.call(event) } }
}

#[component]
pub fn SearchInput(
    #[props(default)] placeholder: String,
    value: String,
    oninput: EventHandler<FormEvent>,
) -> Element {
    rsx! {
        div { class: "relative",
            svg {
                class: "absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground",
                xmlns: "http://www.w3.org/2000/svg",
                width: "24",
                height: "24",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "2",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                circle { cx: "11", cy: "11", r: "8" }
                path { d: "m21 21-4.3-4.3" }
            }
            input {
                r#type: "search",
                aria_label: if placeholder.is_empty() { "Search".to_string() } else { placeholder.clone() },
                class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 pl-8 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 touch-target",
                placeholder: if placeholder.is_empty() { "Search...".to_string() } else { placeholder },
                value,
                oninput: move |evt| oninput.call(evt),
            }
        }
    }
}

#[cfg(test)]
mod association_tests {
    use super::*;
    fn form() -> Element {
        rsx! {
            Field { Label { "First" } Input { value: "first" } }
            Field { Label { "Second" } Select { option { "Second" } } }
            Field { Label { "Third" } Textarea { value: "third" } }
        }
    }
    #[test]
    fn sibling_labels_and_inputs_receive_matching_unique_ids() {
        use dioxus::dioxus_core::{AttributeValue, Mutation};
        let mut dom = VirtualDom::new(form);
        let mutations = dom.rebuild_to_vec();
        let mut ids = Vec::new();
        let mut labels = Vec::new();
        for edit in mutations.edits {
            if let Mutation::SetAttribute {
                name,
                value: AttributeValue::Text(value),
                ..
            } = edit
            {
                if name == "id" && !value.is_empty() {
                    ids.push(value);
                } else if name == "for" && !value.is_empty() {
                    labels.push(value);
                }
            }
        }
        assert_eq!(ids.len(), 3);
        assert_eq!(labels, ids);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
    }
}
