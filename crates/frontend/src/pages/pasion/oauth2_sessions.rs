use dioxus::prelude::*;

use crate::api::pasion;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::ConfirmDialog;
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::i18n::t;

#[component]
pub fn OAuth2SessionsPage() -> Element {
    let mut user_filter = use_signal(String::new);
    let mut sessions_data = use_resource(move || {
        let uid = user_filter.read().clone();
        async move {
            let filter = if uid.is_empty() {
                None
            } else {
                Some(uid.as_str())
            };
            pasion::pasion_get_oauth2_sessions(filter).await
        }
    });

    let mut confirm_open = use_signal(|| false);
    let mut session_to_finish = use_signal(|| Option::<String>::None);

    let handle_finish_confirm = move |_| {
        let id = session_to_finish.read().clone();
        if let Some(id) = id {
            spawn(async move {
                match pasion::pasion_finish_oauth2_session(&id).await {
                    Ok(_) => {
                        show_toast(
                            &t("pasion.oauth2_sessions.finished_success"),
                            ToastVariant::Success,
                        );
                        confirm_open.set(false);
                        session_to_finish.set(None);
                        sessions_data.restart();
                    }
                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                }
            });
        }
    };

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("pasion.oauth2_sessions.title"),
                description: t("pasion.oauth2_sessions.description"),
            }

            // Filter bar
            div { class: "flex items-center gap-4",
                input {
                    class: "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    placeholder: "Filter by user ID...",
                    value: user_filter.read().clone(),
                    oninput: move |evt: FormEvent| {
                        user_filter.set(evt.value());
                        sessions_data.restart();
                    },
                }
                Button {
                    variant: ButtonVariant::Outline,
                    onclick: move |_| sessions_data.restart(),
                    {t("common.refresh")}
                }
            }

            match &*sessions_data.read() {
                Some(Ok(sessions)) => rsx! {
                    div { class: "rounded-md border",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { "Client" }
                                    TableHead { "User" }
                                    TableHead { "Scope" }
                                    TableHead { "Created" }
                                    TableHead { "Last Active" }
                                    TableHead { "IP" }
                                    TableHead { "Status" }
                                    TableHead { class: "text-right".to_string(), "Actions" }
                                }
                            }
                            TableBody {
                                if sessions.is_empty() {
                                    EmptyRow { colspan: 8, message: t("pasion.oauth2_sessions.empty") }
                                } else {
                                    for session in sessions.iter() {
                                        {
                                            let sid = session.id.clone();
                                            let client = session.client_id.clone();
                                            let user = session.user_id.clone().unwrap_or_else(|| "-".to_string());
                                            let scope = session.scope.clone();
                                            let created = session.created_at.clone();
                                            let last_active = session.last_active_at.clone().unwrap_or_else(|| "-".to_string());
                                            let ip = session.last_active_ip.clone().unwrap_or_else(|| "-".to_string());
                                            let is_finished = session.finished_at.is_some();

                                            rsx! {
                                                TableRow { key: "{sid}",
                                                    TableCell {
                                                        span { class: "text-xs font-mono", "{client}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs font-mono text-muted-foreground", "{user}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs text-muted-foreground", "{scope}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs text-muted-foreground", "{created}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs text-muted-foreground", "{last_active}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs font-mono text-muted-foreground", "{ip}" }
                                                    }
                                                    TableCell {
                                                        if is_finished {
                                                            Badge { variant: BadgeVariant::Secondary, "Finished" }
                                                        } else {
                                                            Badge { variant: BadgeVariant::Success, "Active" }
                                                        }
                                                    }
                                                    TableCell { class: "text-right".to_string(),
                                                        if !is_finished {
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: ButtonSize::Sm,
                                                                class: "text-destructive hover:text-destructive".to_string(),
                                                                onclick: move |_| {
                                                                    session_to_finish.set(Some(sid.clone()));
                                                                    confirm_open.set(true);
                                                                },
                                                                "Terminate"
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

        ConfirmDialog {
            open: *confirm_open.read(),
            title: t("pasion.oauth2_sessions.finish_title"),
            description: t("pasion.oauth2_sessions.finish_description"),
            confirm_text: t("pasion.oauth2_sessions.finish"),
            destructive: true,
            on_confirm: handle_finish_confirm,
            on_cancel: move |_| {
                confirm_open.set(false);
                session_to_finish.set(None);
            },
        }
    }
}
