use dioxus::prelude::*;

use crate::api::pasion;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{ConfirmDialog, Modal};
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::i18n::t;

#[component]
pub fn PersonalSessionsPage() -> Element {
    let mut sessions_data = use_resource(|| async { pasion::pasion_get_personal_sessions().await });

    // Create dialog state
    let mut show_create = use_signal(|| false);
    let mut create_name = use_signal(String::new);
    let mut create_scope = use_signal(String::new);
    let mut create_user_id = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let mut new_token = use_signal(|| Option::<String>::None);

    // Confirm revoke
    let mut confirm_revoke_open = use_signal(|| false);
    let mut session_to_revoke = use_signal(|| Option::<String>::None);

    let handle_create = move |_: MouseEvent| {
        let name = create_name.read().clone();
        let scope = create_scope.read().clone();
        let user_id = create_user_id.read().clone();

        let mut data = serde_json::json!({});
        if !name.is_empty() {
            data["name"] = serde_json::Value::String(name);
        }
        if !scope.is_empty() {
            data["scope"] = serde_json::Value::String(scope);
        }
        if !user_id.is_empty() {
            data["user_id"] = serde_json::Value::String(user_id);
        }

        creating.set(true);
        spawn(async move {
            match pasion::pasion_create_personal_session(data).await {
                Ok(session) => {
                    if let Some(token) = session.token.clone() {
                        new_token.set(Some(token));
                    }
                    show_toast(
                        &t("pasion.personal_sessions.created_success"),
                        ToastVariant::Success,
                    );
                    create_name.set(String::new());
                    create_scope.set(String::new());
                    create_user_id.set(String::new());
                    show_create.set(false);
                    sessions_data.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
            creating.set(false);
        });
    };

    let handle_regenerate = move |id: String| {
        spawn(async move {
            match pasion::pasion_regenerate_personal_session(&id).await {
                Ok(session) => {
                    if let Some(token) = session.token {
                        new_token.set(Some(token));
                    }
                    show_toast(
                        &t("pasion.personal_sessions.regenerated_success"),
                        ToastVariant::Success,
                    );
                    sessions_data.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
        });
    };

    let handle_revoke_confirm = move |_| {
        let id = session_to_revoke.read().clone();
        if let Some(id) = id {
            spawn(async move {
                match pasion::pasion_revoke_personal_session(&id).await {
                    Ok(_) => {
                        show_toast(
                            &t("pasion.personal_sessions.revoked_success"),
                            ToastVariant::Success,
                        );
                        confirm_revoke_open.set(false);
                        session_to_revoke.set(None);
                        sessions_data.restart();
                    }
                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                }
            });
        }
    };

    let is_creating = *creating.read();

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("pasion.personal_sessions.title"),
                description: t("pasion.personal_sessions.description"),
                Button {
                    onclick: move |_| show_create.set(true),
                    {t("pasion.personal_sessions.create")}
                }
            }

            // New token display banner
            if let Some(token) = new_token.read().clone() {
                div { class: "rounded-md bg-green-500/10 border border-green-500/20 p-4",
                    p { class: "text-sm font-medium text-green-700 dark:text-green-400 mb-2",
                        {t("pasion.personal_sessions.new_token_warning")}
                    }
                    div { class: "flex items-center gap-2",
                        code { class: "flex-1 text-xs font-mono bg-background p-2 rounded border break-all",
                            "{token}"
                        }
                        Button {
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            onclick: move |_| {
                                let tok = token.clone();
                                spawn(async move {
                                    let window = web_sys::window().unwrap();
                                    let navigator = window.navigator();
                                    let clipboard = navigator.clipboard();
                                    let _ = wasm_bindgen_futures::JsFuture::from(clipboard.write_text(&tok)).await;
                                    show_toast(&t("pasion.personal_sessions.copy_success"), ToastVariant::Success);
                                });
                            },
                            {t("common.copy")}
                        }
                        Button {
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            onclick: move |_| new_token.set(None),
                            {t("common.dismiss")}
                        }
                    }
                }
            }

            match &*sessions_data.read() {
                Some(Ok(sessions)) => rsx! {
                    div { class: "rounded-md border",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { {t("common.name")} }
                                    TableHead { {t("pasion.personal_sessions.col_scope")} }
                                    TableHead { {t("pasion.personal_sessions.col_owner")} }
                                    TableHead { {t("pasion.personal_sessions.col_created")} }
                                    TableHead { {t("pasion.personal_sessions.col_last_active")} }
                                    TableHead { {t("common.status")} }
                                    TableHead { class: "text-right".to_string(), {t("common.actions")} }
                                }
                            }
                            TableBody {
                                if sessions.is_empty() {
                                    EmptyRow { colspan: 7, message: t("pasion.personal_sessions.empty") }
                                } else {
                                    for session in sessions.iter() {
                                        {
                                            let sid = session.id.clone();
                                            let sid2 = sid.clone();
                                            let sid3 = sid.clone();
                                            let name = session.name.clone().unwrap_or_else(|| "-".to_string());
                                            let scope = session.scope.clone();
                                            let owner = session.user_id.clone().unwrap_or_else(|| "-".to_string());
                                            let created = session.created_at.clone();
                                            let last_active = session.last_active_at.clone().unwrap_or_else(|| "-".to_string());
                                            let is_revoked = session.revoked_at.is_some();

                                            rsx! {
                                                TableRow { key: "{sid}",
                                                    TableCell {
                                                        span { class: "font-medium text-sm", "{name}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs text-muted-foreground font-mono", "{scope}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs font-mono text-muted-foreground", "{owner}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs text-muted-foreground", "{created}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs text-muted-foreground", "{last_active}" }
                                                    }
                                                    TableCell {
                                                        if is_revoked {
                                                            Badge { variant: BadgeVariant::Destructive, {t("pasion.status_revoked")} }
                                                        } else {
                                                            Badge { variant: BadgeVariant::Success, {t("pasion.status_active")} }
                                                        }
                                                    }
                                                    TableCell { class: "text-right".to_string(),
                                                        div { class: "flex justify-end gap-2",
                                                            if !is_revoked {
                                                                Button {
                                                                    variant: ButtonVariant::Ghost,
                                                                    size: ButtonSize::Sm,
                                                                    onclick: move |_| handle_regenerate(sid2.clone()),
                                                                    {t("pasion.personal_sessions.regenerate")}
                                                                }
                                                                Button {
                                                                    variant: ButtonVariant::Ghost,
                                                                    size: ButtonSize::Sm,
                                                                    class: "text-destructive hover:text-destructive".to_string(),
                                                                    onclick: move |_| {
                                                                        session_to_revoke.set(Some(sid3.clone()));
                                                                        confirm_revoke_open.set(true);
                                                                    },
                                                                    {t("pasion.personal_sessions.revoke")}
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
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| sessions_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        // Create dialog
        Modal {
            open: *show_create.read(),
            on_close: move |_| {
                if !is_creating { show_create.set(false); }
            },
                    h2 { class: "text-lg font-semibold mb-4", {t("pasion.personal_sessions.create_title")} }
                    div { class: "space-y-4",
                        div { class: "space-y-2",
                            Label { {t("common.name")} }
                            Input {
                                placeholder: t("pasion.personal_sessions.name_placeholder"),
                                value: create_name.read().clone(),
                                oninput: move |evt: FormEvent| create_name.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { {t("pasion.personal_sessions.scope_label")} }
                            Input {
                                placeholder: t("pasion.personal_sessions.scope_placeholder"),
                                value: create_scope.read().clone(),
                                oninput: move |evt: FormEvent| create_scope.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { {t("pasion.personal_sessions.owner_label")} }
                            Input {
                                placeholder: t("pasion.personal_sessions.owner_placeholder"),
                                value: create_user_id.read().clone(),
                                oninput: move |evt: FormEvent| create_user_id.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                    }
                    div { class: "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2 mt-6",
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: is_creating,
                            onclick: move |_| show_create.set(false),
                            {t("common.cancel")}
                        }
                        Button {
                            disabled: is_creating,
                            onclick: handle_create,
                            if is_creating {
                                Spinner { class: "mr-2".to_string() }
                            }
                            {t("common.create")}
                        }
                    }
        }

        ConfirmDialog {
            open: *confirm_revoke_open.read(),
            title: t("pasion.personal_sessions.revoke_title"),
            description: t("pasion.personal_sessions.revoke_description"),
            confirm_text: t("pasion.personal_sessions.revoke"),
            destructive: true,
            on_confirm: handle_revoke_confirm,
            on_cancel: move |_| {
                confirm_revoke_open.set(false);
                session_to_revoke.set(None);
            },
        }
    }
}
