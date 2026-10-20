use dioxus::prelude::*;

use crate::api::registration_tokens;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{ConfirmDialog, Modal};
use crate::components::ui::empty_state::EmptyState;
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label, SearchInput};
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::date::format_timestamp;
use crate::utils::i18n::t;

#[derive(Debug, Clone, PartialEq)]
enum TokenFilter {
    All,
    Active,
    Expired,
}

impl TokenFilter {
    fn label(&self) -> String {
        match self {
            TokenFilter::All => t("registration_tokens.filter_all"),
            TokenFilter::Active => t("registration_tokens.filter_active"),
            TokenFilter::Expired => t("registration_tokens.filter_expired"),
        }
    }
}

#[component]
pub fn RegistrationTokenList() -> Element {
    let mut tokens_data =
        use_resource(|| async { registration_tokens::get_registration_tokens_cached().await });
    let mut delete_dialog_open = use_signal(|| false);
    let mut token_to_delete = use_signal(|| Option::<String>::None);
    let mut search = use_signal(String::new);
    let mut filter = use_signal(|| TokenFilter::All);

    // Create dialog state
    let mut show_create_dialog = use_signal(|| false);
    let mut create_token_value = use_signal(String::new);
    let mut create_uses_allowed = use_signal(String::new);
    let mut create_expiry_date = use_signal(String::new);
    let mut creating = use_signal(|| false);

    let handle_create = move |_: MouseEvent| {
        let token_val = create_token_value.read().clone();
        let uses_str = create_uses_allowed.read().clone();
        let expiry_str = create_expiry_date.read().clone();

        let token = if token_val.is_empty() {
            None
        } else {
            Some(token_val)
        };
        let uses = uses_str.parse::<u64>().ok();
        let expiry = if expiry_str.is_empty() {
            None
        } else {
            chrono::NaiveDate::parse_from_str(&expiry_str, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(23, 59, 59))
                .map(|dt| dt.and_utc().timestamp_millis() as u64)
        };
        let length = if token.is_some() { None } else { Some(16) };

        creating.set(true);
        spawn(async move {
            match registration_tokens::create_registration_token(
                token.as_deref(),
                uses,
                expiry,
                length,
            )
            .await
            {
                Ok(t) => {
                    show_toast(
                        &format!("Token created: {}", t.token.token),
                        ToastVariant::Success,
                    );
                    show_create_dialog.set(false);
                    create_token_value.set(String::new());
                    create_uses_allowed.set(String::new());
                    create_expiry_date.set(String::new());
                    tokens_data.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
            creating.set(false);
        });
    };

    let handle_delete_confirm = move |_| {
        let token = token_to_delete.read().clone();
        if let Some(token) = token {
            spawn(async move {
                match registration_tokens::delete_registration_token(&token).await {
                    Ok(_) => {
                        show_toast("Token deleted", ToastVariant::Success);
                        delete_dialog_open.set(false);
                        token_to_delete.set(None);
                        tokens_data.restart();
                    }
                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                }
            });
        }
    };

    let is_creating = *creating.read();
    let current_filter = filter.read().clone();
    let search_val = search.read().clone();

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("registration_tokens.title"),
                description: t("registration_tokens.subtitle"),
                Button {
                    onclick: move |_| show_create_dialog.set(true),
                    Icon { name: "plus".to_string(), class: "h-4 w-4 mr-2".to_string() }
                    {t("registration_tokens.create")}
                }
            }

            // Search and filters
            div { class: "flex items-center gap-4",
                div { class: "flex-1",
                    SearchInput {
                        placeholder: t("registration_tokens.search_placeholder"),
                        value: search.read().clone(),
                        oninput: move |evt: FormEvent| search.set(evt.value()),
                    }
                }
            }

            // Filter chips
            div { class: "flex items-center gap-2",
                {
                    let filters = [TokenFilter::All, TokenFilter::Active, TokenFilter::Expired];
                    rsx! {
                        for f in filters.iter() {
                            {
                                let is_active = current_filter == *f;
                                let filter_val = f.clone();
                                let label = f.label();
                                rsx! {
                                    button {
                                        key: "{label}",
                                        class: if is_active {
                                            "inline-flex items-center rounded-full px-3 py-1 text-xs font-medium bg-primary text-primary-foreground"
                                        } else {
                                            "inline-flex items-center rounded-full px-3 py-1 text-xs font-medium border border-input bg-background text-foreground hover:bg-accent hover:text-accent-foreground"
                                        },
                                        onclick: move |_| filter.set(filter_val.clone()),
                                        "{label}"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            match &*tokens_data.read() {
                Some(Ok(tokens)) => {
                    let now = js_sys::Date::now() as u64;
                    let filtered: Vec<_> = tokens.iter().filter(|t| {
                        // Search filter
                        let matches_search = search_val.is_empty() || t.token.token.contains(&search_val);
                        // Status filter
                        let is_expired = t.token.expiry_time.map(|ts| ts > 0 && ts < now).unwrap_or(false);
                        let matches_filter = match current_filter {
                            TokenFilter::All => true,
                            TokenFilter::Active => !is_expired,
                            TokenFilter::Expired => is_expired,
                        };
                        matches_search && matches_filter
                    }).collect();

                    rsx! {
                    div { class: "rounded-md border",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { {t("registration_tokens.token")} }
                                    TableHead { {t("registration_tokens.uses_allowed")} }
                                    TableHead { {t("registration_tokens.pending")} }
                                    TableHead { {t("registration_tokens.completed")} }
                                    TableHead { {t("registration_tokens.expiry")} }
                                    TableHead { class: "text-right".to_string(), {t("registration_tokens.actions")} }
                                }
                            }
                            TableBody {
                                if filtered.is_empty() {
                                    TableRow {
                                        TableCell { class: "p-0".to_string(), colspan: 99,
                                            EmptyState {
                                                icon: "key".to_string(),
                                                title: t("registration_tokens.no_tokens"),
                                                description: t("registration_tokens.no_tokens_description"),
                                            }
                                        }
                                    }
                                } else {
                                    for token in filtered.iter() {
                                        {
                                            let token_str = token.token.token.clone();
                                            let uses_allowed = token.token.uses_allowed
                                                .map(|u| u.to_string())
                                                .unwrap_or_else(|| t("registration_tokens.unlimited"));
                                            let pending = token.token.pending;
                                            let completed = token.token.completed;
                                            let expiry = token.token.expiry_time
                                                .map(format_timestamp)
                                                .unwrap_or_else(|| t("registration_tokens.never"));
                                            let is_expired = token.token.expiry_time
                                                .map(|ts| ts > 0 && ts < now)
                                                .unwrap_or(false);
                                            let token_for_copy = token_str.clone();
                                            let token_for_delete = token_str.clone();

                                            rsx! {
                                                TableRow {
                                                    TableCell {
                                                        div { class: "flex items-center gap-2",
                                                            Icon { name: "key".to_string(), class: "h-4 w-4 text-muted-foreground".to_string() }
                                                            code { class: "text-sm font-mono bg-muted px-1.5 py-0.5 rounded",
                                                                "{token_str}"
                                                            }
                                                            button {
                                                                class: "h-6 w-6 inline-flex items-center justify-center rounded hover:bg-muted",
                                                                title: "Copy to clipboard",
                                                                onclick: move |_| {
                                                                    let t = token_for_copy.clone();
                                                                    spawn(async move {
                                                                        let window = web_sys::window().unwrap();
                                                                        let navigator = window.navigator();
                                                                        let clipboard = navigator.clipboard();
                                                                        let _ = wasm_bindgen_futures::JsFuture::from(clipboard.write_text(&t)).await;
                                                                        show_toast("Token copied to clipboard", ToastVariant::Success);
                                                                    });
                                                                },
                                                                svg {
                                                                    class: "h-3 w-3",
                                                                    xmlns: "http://www.w3.org/2000/svg",
                                                                    width: "24", height: "24",
                                                                    view_box: "0 0 24 24",
                                                                    fill: "none", stroke: "currentColor",
                                                                    stroke_width: "2",
                                                                    rect { width: "14", height: "14", x: "8", y: "8", rx: "2", ry: "2" }
                                                                    path { d: "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" }
                                                                }
                                                            }
                                                            if is_expired {
                                                                Badge { variant: BadgeVariant::Destructive, {t("registration_tokens.filter_expired")} }
                                                            }
                                                        }
                                                    }
                                                    TableCell { "{uses_allowed}" }
                                                    TableCell {
                                                        if pending > 0 {
                                                            Badge { variant: BadgeVariant::Secondary, "{pending}" }
                                                        } else {
                                                            "0"
                                                        }
                                                    }
                                                    TableCell {
                                                        Badge { variant: BadgeVariant::Success, "{completed}" }
                                                    }
                                                    TableCell {
                                                        span {
                                                            class: if is_expired { "text-destructive" } else { "text-muted-foreground" },
                                                            "{expiry}"
                                                        }
                                                    }
                                                    TableCell { class: "text-right".to_string(),
                                                        Button {
                                                            variant: ButtonVariant::Ghost,
                                                            size: ButtonSize::Sm,
                                                            class: "text-destructive hover:text-destructive".to_string(),
                                                            onclick: move |_| {
                                                                token_to_delete.set(Some(token_for_delete.clone()));
                                                                delete_dialog_open.set(true);
                                                            },
                                                            {t("registration_tokens.delete")}
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }},
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| tokens_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        // Create Token Dialog
        Modal {
            open: *show_create_dialog.read(),
            on_close: move |_| {
                if !is_creating { show_create_dialog.set(false); }
            },
                    div { class: "flex flex-col space-y-2 text-center sm:text-left",
                        h2 { class: "text-lg font-semibold", {t("registration_tokens.create")} }
                        p { class: "text-sm text-muted-foreground", {t("registration_tokens.create_description")} }
                    }
                    div { class: "space-y-4 mt-4",
                        div { class: "space-y-2",
                            Label { {t("registration_tokens.custom_token")} }
                            Input {
                                placeholder: t("registration_tokens.custom_token_placeholder"),
                                value: create_token_value.read().clone(),
                                oninput: move |evt: FormEvent| create_token_value.set(evt.value()),
                                disabled: is_creating,
                            }
                            p { class: "text-xs text-muted-foreground", {t("registration_tokens.custom_token_hint")} }
                        }
                        div { class: "space-y-2",
                            Label { {t("registration_tokens.uses_allowed")} }
                            Input {
                                r#type: "number".to_string(),
                                placeholder: t("registration_tokens.unlimited"),
                                value: create_uses_allowed.read().clone(),
                                oninput: move |evt: FormEvent| create_uses_allowed.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { {t("registration_tokens.expiry")} }
                            input {
                                r#type: "date",
                                class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2",
                                value: create_expiry_date.read().clone(),
                                oninput: move |evt: FormEvent| create_expiry_date.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                    }
                    div { class: "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2 mt-6",
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: is_creating,
                            onclick: move |_| show_create_dialog.set(false),
                            {t("common.cancel")}
                        }
                        Button {
                            disabled: is_creating,
                            onclick: handle_create,
                            if is_creating {
                                Spinner { class: "mr-2".to_string() }
                            }
                            {t("registration_tokens.create")}
                        }
                    }
        }

        ConfirmDialog {
            open: *delete_dialog_open.read(),
            title: t("registration_tokens.delete_token"),
            description: {
                let t = token_to_delete.read().clone().unwrap_or_default();
                format!("Are you sure you want to delete token {t}?")
            },
            confirm_text: t("common.delete"),
            destructive: true,
            on_confirm: handle_delete_confirm,
            on_cancel: move |_| {
                delete_dialog_open.set(false);
                token_to_delete.set(None);
            },
        }
    }
}
