use dioxus::prelude::*;

use crate::api::pasion;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{ConfirmDialog, Modal};
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::i18n::t;

#[component]
pub fn UpstreamLinksPage() -> Element {
    let mut user_filter = use_signal(String::new);
    let mut provider_filter = use_signal(String::new);
    let mut create_open = use_signal(|| false);
    let mut create_user_id = use_signal(String::new);
    let mut create_provider_id = use_signal(String::new);
    let mut create_subject = use_signal(String::new);
    let mut create_account_name = use_signal(String::new);
    let mut creating = use_signal(|| false);

    let mut links_data = use_resource(move || {
        let uid = user_filter.read().clone();
        let pid = provider_filter.read().clone();
        async move {
            let user = if uid.is_empty() {
                None
            } else {
                Some(uid.as_str())
            };
            let provider = if pid.is_empty() {
                None
            } else {
                Some(pid.as_str())
            };
            pasion::pasion_get_upstream_links(user, provider).await
        }
    });

    let mut confirm_open = use_signal(|| false);
    let mut link_to_delete = use_signal(|| Option::<String>::None);

    let handle_create = move |_| {
        let user_id = create_user_id.read().trim().to_string();
        let provider_id = create_provider_id.read().trim().to_string();
        let subject = create_subject.read().trim().to_string();
        let account_name = create_account_name.read().trim().to_string();

        if user_id.is_empty() || provider_id.is_empty() || subject.is_empty() {
            show_toast(
                "user_id, provider_id, and subject are required",
                ToastVariant::Error,
            );
            return;
        }

        let mut body = serde_json::json!({
            "user_id": user_id,
            "provider_id": provider_id,
            "subject": subject,
        });
        if !account_name.is_empty() {
            body["human_account_name"] = account_name.into();
        }

        creating.set(true);
        spawn(async move {
            match pasion::pasion_create_upstream_link(body).await {
                Ok(link) => {
                    show_toast("Link created", ToastVariant::Success);
                    user_filter.set(link.user_id.unwrap_or_default());
                    provider_filter.set(link.provider_id.clone());
                    create_open.set(false);
                    create_user_id.set(String::new());
                    create_provider_id.set(String::new());
                    create_subject.set(String::new());
                    create_account_name.set(String::new());
                    links_data.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
            creating.set(false);
        });
    };

    let handle_delete_confirm = move |_| {
        let id = link_to_delete.read().clone();
        if let Some(id) = id {
            spawn(async move {
                match pasion::pasion_delete_upstream_link(&id).await {
                    Ok(_) => {
                        show_toast("Link deleted", ToastVariant::Success);
                        confirm_open.set(false);
                        link_to_delete.set(None);
                        links_data.restart();
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
                title: t("pasion.upstream_links.title"),
                description: t("pasion.upstream_links.description"),
                Button {
                    onclick: move |_| {
                        create_user_id.set(user_filter.read().clone());
                        create_provider_id.set(provider_filter.read().clone());
                        create_subject.set(String::new());
                        create_account_name.set(String::new());
                        create_open.set(true);
                    },
                    Icon { name: "plus".to_string(), class: "h-4 w-4 mr-2".to_string() }
                    "Create Link"
                }
            }

            // Filters
            div { class: "flex items-center gap-4 flex-wrap",
                input {
                    class: "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    placeholder: "Filter by user ID...",
                    value: user_filter.read().clone(),
                    oninput: move |evt: FormEvent| {
                        user_filter.set(evt.value());
                        links_data.restart();
                    },
                }
                input {
                    class: "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    placeholder: "Filter by provider ID...",
                    value: provider_filter.read().clone(),
                    oninput: move |evt: FormEvent| {
                        provider_filter.set(evt.value());
                        links_data.restart();
                    },
                }
                Button {
                    variant: ButtonVariant::Outline,
                    onclick: move |_| links_data.restart(),
                    {t("common.refresh")}
                }
            }

            match &*links_data.read() {
                Some(Ok(links)) => rsx! {
                    div { class: "rounded-md border",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { "User ID" }
                                    TableHead { "Provider" }
                                    TableHead { "Subject" }
                                    TableHead { "Created" }
                                    TableHead { class: "text-right".to_string(), "Actions" }
                                }
                            }
                            TableBody {
                                if links.is_empty() {
                                    EmptyRow { colspan: 5, message: t("pasion.upstream_links.empty") }
                                } else {
                                    for link in links.iter() {
                                        {
                                            let lid = link.id.clone();
                                            let lid2 = lid.clone();
                                            let user_id = link.user_id.clone().unwrap_or_else(|| "-".to_string());
                                            let provider_id = link.provider_id.clone();
                                            let account_name = link.human_account_name.clone();
                                            let subject = link.subject.clone();
                                            let created = link.created_at.clone();

                                            rsx! {
                                                TableRow { key: "{lid}",
                                                    TableCell {
                                                        span { class: "text-xs font-mono", "{user_id}" }
                                                    }
                                                    TableCell {
                                                        div { class: "space-y-1",
                                                            span { class: "text-xs font-mono", "{provider_id}" }
                                                            if let Some(account_name) = account_name.clone() {
                                                                if !account_name.is_empty() {
                                                                    div { class: "text-xs text-muted-foreground", "{account_name}" }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs font-mono text-muted-foreground", "{subject}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-xs text-muted-foreground", "{created}" }
                                                    }
                                                    TableCell { class: "text-right".to_string(),
                                                        Button {
                                                            variant: ButtonVariant::Ghost,
                                                            size: ButtonSize::Sm,
                                                            class: "text-destructive hover:text-destructive".to_string(),
                                                            onclick: move |_| {
                                                                link_to_delete.set(Some(lid2.clone()));
                                                                confirm_open.set(true);
                                                            },
                                                            "Delete"
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
                        on_retry: move |_| links_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        ConfirmDialog {
            open: *confirm_open.read(),
            title: "Delete Upstream Link".to_string(),
            description: "Are you sure you want to delete this upstream OAuth link? The user will no longer be able to sign in via this provider link.".to_string(),
            confirm_text: "Delete".to_string(),
            destructive: true,
            on_confirm: handle_delete_confirm,
            on_cancel: move |_| {
                confirm_open.set(false);
                link_to_delete.set(None);
            },
        }

        Modal {
            open: *create_open.read(),
            on_close: move |_| {
                if !is_creating {
                    create_open.set(false);
                }
            },
                    h2 { class: "text-lg font-semibold mb-2", "Create Upstream Link" }
                    p { class: "text-sm text-muted-foreground mb-4",
                        "Create a user-to-provider binding for an existing federated account."
                    }
                    div { class: "space-y-4",
                        div { class: "space-y-2",
                            Label { "Pasion User ID" }
                            Input {
                                placeholder: "01H...".to_string(),
                                value: create_user_id.read().clone(),
                                oninput: move |evt: FormEvent| create_user_id.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { "Provider ID" }
                            Input {
                                placeholder: "google".to_string(),
                                value: create_provider_id.read().clone(),
                                oninput: move |evt: FormEvent| create_provider_id.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { "Subject" }
                            Input {
                                placeholder: "OAuth subject".to_string(),
                                value: create_subject.read().clone(),
                                oninput: move |evt: FormEvent| create_subject.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { "Account name (optional)" }
                            Input {
                                placeholder: "name@example.com".to_string(),
                                value: create_account_name.read().clone(),
                                oninput: move |evt: FormEvent| create_account_name.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                    }
                    div { class: "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2 mt-6",
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: is_creating,
                            onclick: move |_| create_open.set(false),
                            "Cancel"
                        }
                        Button {
                            disabled: is_creating,
                            onclick: handle_create,
                            if is_creating {
                                Spinner { class: "mr-2".to_string() }
                            }
                            "Create"
                        }
                    }
        }
    }
}
