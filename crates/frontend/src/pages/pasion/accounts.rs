//! Pasion local accounts: list, search/filter and create.
//!
//! These are the identity-provider accounts that Pasion owns (username,
//! password, lock / deactivation state, admin flag). They are distinct
//! from the Matrix user records under `/users`, which come from palpo.

use dioxus::prelude::*;

use crate::api::pasion::{self, PasionUserFilter};
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label, SearchInput};
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::router::Route;
use crate::types::PasionUser;
use crate::utils::i18n::t;

const PAGE_SIZE: u64 = 25;

const SELECT_CLASS: &str = "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

/// Render an RFC 3339 timestamp as `YYYY-MM-DD HH:MM` (UTC).
pub(crate) fn short_time(ts: &str) -> String {
    let trimmed: String = ts.chars().take(16).collect();
    trimmed.replace('T', " ")
}

/// Status badges shared by the list and detail pages.
#[component]
pub(crate) fn AccountStatusBadges(user: PasionUser) -> Element {
    rsx! {
        div { class: "flex flex-wrap items-center gap-1",
            if user.deactivated_at.is_some() {
                Badge { variant: BadgeVariant::Destructive, {t("pasion.accounts.status_deactivated")} }
            } else if user.locked_at.is_some() {
                Badge { variant: BadgeVariant::Destructive, {t("pasion.accounts.status_locked")} }
            } else {
                Badge { variant: BadgeVariant::Success, {t("pasion.accounts.status_active")} }
            }
            if user.admin {
                Badge { variant: BadgeVariant::Default,
                    Icon { name: "shield".to_string(), class: "h-3 w-3 mr-1".to_string() }
                    {t("pasion.accounts.admin")}
                }
            }
            if user.legacy_guest {
                Badge { variant: BadgeVariant::Outline, "Guest" }
            }
        }
    }
}

#[component]
pub fn PasionAccountsPage() -> Element {
    let nav = use_navigator();

    let mut search = use_signal(String::new);
    let mut status = use_signal(String::new);
    let mut admin = use_signal(String::new);
    // Cursor stack: `cursors[i]` is the `page[after]` for page i+1.
    let mut cursors = use_signal(Vec::<Option<String>>::new);

    let mut create_open = use_signal(|| false);
    let mut create_username = use_signal(String::new);
    let mut create_password = use_signal(String::new);
    let mut create_skip_check = use_signal(|| false);
    let mut create_admin = use_signal(|| false);
    let mut creating = use_signal(|| false);

    let mut users_data = use_resource(move || {
        let filter = PasionUserFilter {
            search: Some(search.read().trim().to_string()),
            status: Some(status.read().clone()),
            admin: match admin.read().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
        };
        let after = cursors.read().last().cloned().flatten();
        async move { pasion::pasion_get_users(&filter, after.as_deref(), PAGE_SIZE).await }
    });

    let handle_create = move |_| {
        let username = create_username.read().trim().to_string();
        let password = create_password.read().clone();
        let skip_check = *create_skip_check.read();
        let make_admin = *create_admin.read();

        if username.is_empty() {
            show_toast("Username is required", ToastVariant::Error);
            return;
        }

        creating.set(true);
        spawn(async move {
            let user = match pasion::pasion_create_user(&username).await {
                Ok(user) => user,
                Err(e) => {
                    show_toast(&format!("Failed: {}", e.message), ToastVariant::Error);
                    creating.set(false);
                    return;
                }
            };

            // The account exists from here on; follow-up failures are
            // reported but still take the admin to the new account so
            // they can retry from the detail page.
            let mut follow_up_errors = Vec::new();
            if !password.is_empty()
                && let Err(e) = pasion::pasion_set_password(&user.id, &password, skip_check).await
            {
                follow_up_errors.push(format!("password: {}", e.message));
            }
            if make_admin
                && let Err(e) =
                    pasion::pasion_update_user(&user.id, serde_json::json!({ "admin": true })).await
            {
                follow_up_errors.push(format!("admin: {}", e.message));
            }

            if follow_up_errors.is_empty() {
                show_toast("Account created", ToastVariant::Success);
            } else {
                show_toast(
                    &format!(
                        "Account created, but some settings failed ({})",
                        follow_up_errors.join("; ")
                    ),
                    ToastVariant::Error,
                );
            }
            creating.set(false);
            create_open.set(false);
            nav.push(Route::PasionAccountShow { user_id: user.id });
        });
    };

    let page_number = cursors.read().len() + 1;
    let is_creating = *creating.read();

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("pasion.accounts.title"),
                description: t("pasion.accounts.description"),
                Button {
                    onclick: move |_| {
                        create_username.set(String::new());
                        create_password.set(String::new());
                        create_skip_check.set(false);
                        create_admin.set(false);
                        create_open.set(true);
                    },
                    Icon { name: "plus".to_string(), class: "h-4 w-4 mr-2".to_string() }
                    "Create Account"
                }
            }

            // Filters
            div { class: "flex items-center gap-4 flex-wrap",
                div { class: "w-64",
                    SearchInput {
                        placeholder: "Search username...".to_string(),
                        value: search.read().clone(),
                        oninput: move |evt: FormEvent| {
                            search.set(evt.value());
                            cursors.write().clear();
                        },
                    }
                }
                crate::components::ui::input::Select {
                    class: SELECT_CLASS,
                    aria_label: "Status",
                    value: status.read().clone(),
                    onchange: move |evt: FormEvent| {
                        status.set(evt.value());
                        cursors.write().clear();
                    },
                    option { value: "", "All statuses" }
                    option { value: "active", {t("pasion.accounts.status_active")} }
                    option { value: "locked", {t("pasion.accounts.status_locked")} }
                    option { value: "deactivated", {t("pasion.accounts.status_deactivated")} }
                }
                crate::components::ui::input::Select {
                    class: SELECT_CLASS,
                    aria_label: "Role",
                    value: admin.read().clone(),
                    onchange: move |evt: FormEvent| {
                        admin.set(evt.value());
                        cursors.write().clear();
                    },
                    option { value: "", "All roles" }
                    option { value: "true", "Admins" }
                    option { value: "false", "Non-admins" }
                }
                Button {
                    variant: ButtonVariant::Outline,
                    onclick: move |_| users_data.restart(),
                    {t("common.refresh")}
                }
            }

            match &*users_data.read() {
                Some(Ok(page)) => {
                    let next_cursor = page.next_cursor.clone();
                    let count_label = page
                        .count
                        .map(|c| format!("{c} accounts"))
                        .unwrap_or_default();
                    rsx! {
                        div { class: "rounded-md border",
                            Table {
                                TableHeader {
                                    TableRow {
                                        TableHead { "Username" }
                                        TableHead { "Display name" }
                                        TableHead { "Status" }
                                        TableHead { "Created" }
                                    }
                                }
                                TableBody {
                                    if page.data.is_empty() {
                                        EmptyRow { colspan: 4, message: t("pasion.accounts.empty") }
                                    } else {
                                        for user in page.data.iter() {
                                            {
                                                let uid = user.id.clone();
                                                let username = user.username.clone();
                                                let display_name = user.display_name.clone().unwrap_or_else(|| "-".to_string());
                                                let created = short_time(&user.created_at);
                                                let user = user.clone();
                                                rsx! {
                                                    TableRow { key: "{uid}",
                                                        TableCell {
                                                            Link {
                                                                to: Route::PasionAccountShow { user_id: uid.clone() },
                                                                class: "font-medium text-primary hover:underline",
                                                                "{username}"
                                                            }
                                                        }
                                                        TableCell { "{display_name}" }
                                                        TableCell { AccountStatusBadges { user } }
                                                        TableCell {
                                                            span { class: "text-xs text-muted-foreground", "{created}" }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex items-center justify-between px-2",
                            div { class: "text-sm text-muted-foreground", "{count_label}" }
                            div { class: "flex items-center space-x-2",
                                Button {
                                    variant: ButtonVariant::Outline,
                                    size: ButtonSize::Sm,
                                    disabled: page_number <= 1,
                                    onclick: move |_| {
                                        cursors.write().pop();
                                    },
                                    "Previous"
                                }
                                span { class: "text-sm text-muted-foreground", "Page {page_number}" }
                                Button {
                                    variant: ButtonVariant::Outline,
                                    size: ButtonSize::Sm,
                                    disabled: next_cursor.is_none(),
                                    onclick: move |_| {
                                        if let Some(cursor) = next_cursor.clone() {
                                            cursors.write().push(Some(cursor));
                                        }
                                    },
                                    "Next"
                                }
                            }
                        }
                    }
                }
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| users_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        Modal {
            open: *create_open.read(),
            on_close: move |_| {
                if !is_creating {
                    create_open.set(false);
                }
            },
            h2 { class: "text-lg font-semibold mb-2", "Create Account" }
            p { class: "text-sm text-muted-foreground mb-4",
                "Create a local Pasion account. The matching Matrix user is provisioned on the homeserver automatically."
            }
            div { class: "space-y-4",
                crate::components::ui::input::Field { class: "space-y-2",
                    Label { r#for: "pasion-create-username", "Username" }
                    Input {
                        id: "pasion-create-username".to_string(),
                        placeholder: "alice".to_string(),
                        value: create_username.read().clone(),
                        oninput: move |evt: FormEvent| create_username.set(evt.value()),
                        disabled: is_creating,
                    }
                }
                crate::components::ui::input::Field { class: "space-y-2",
                    Label { r#for: "pasion-create-password", "Initial password (optional)" }
                    div { class: "flex gap-2",
                        div { class: "flex-1",
                            Input {
                                id: "pasion-create-password".to_string(),
                                r#type: "password".to_string(),
                                autocomplete: "new-password".to_string(),
                                value: create_password.read().clone(),
                                oninput: move |evt: FormEvent| create_password.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: is_creating,
                            onclick: move |_| {
                                create_password.set(crate::utils::password::generate_random_password());
                            },
                            "Generate"
                        }
                    }
                    p { class: "text-xs text-muted-foreground",
                        "Leave empty to create the account without a password (e.g. for upstream-only sign-in)."
                    }
                }
                label { class: "flex items-center gap-2 text-sm",
                    input {
                        r#type: "checkbox",
                        checked: *create_skip_check.read(),
                        disabled: is_creating,
                        onchange: move |evt: FormEvent| create_skip_check.set(evt.checked()),
                    }
                    "Skip password complexity check"
                }
                label { class: "flex items-center gap-2 text-sm",
                    input {
                        r#type: "checkbox",
                        checked: *create_admin.read(),
                        disabled: is_creating,
                        onchange: move |evt: FormEvent| create_admin.set(evt.checked()),
                    }
                    "Grant administrator privileges"
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
