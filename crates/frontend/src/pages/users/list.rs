use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use futures::stream::{self, StreamExt};
use std::collections::HashSet;
use wasm_bindgen::JsCast;

use crate::api::users;
use crate::components::deactivate_user_dialog::{DeactivateUserDialog, show_account_change_toast};
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{Modal, ModalSize};
use crate::components::ui::empty_state::EmptyState;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::SearchInput;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::pagination::Pagination;
use crate::components::ui::relative_time::RelativeTime;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::components::user_import::UserImport;
use crate::pages::users::tabs::{UsersTab, UsersTabs};
use crate::router::Route;
use crate::utils::i18n::t;

const PAGE_SIZE_OPTIONS: &[u64] = &[10, 25, 50, 100];
const DEFAULT_PAGE_SIZE: u64 = 25;

/// Max in-flight requests for bulk user actions. Bounds concurrency so a large
/// selection doesn't open hundreds of simultaneous connections.
const BULK_CONCURRENCY: usize = 8;

fn download_csv(filename: &str, content: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(document) = window.document() {
            let blob_parts = js_sys::Array::new();
            blob_parts.push(&wasm_bindgen::JsValue::from_str(content));
            let options = web_sys::BlobPropertyBag::new();
            options.set_type("text/csv;charset=utf-8;");
            if let Ok(blob) =
                web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &options)
            {
                if let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) {
                    if let Ok(a) = document.create_element("a") {
                        let _ = a.set_attribute("href", &url);
                        let _ = a.set_attribute("download", filename);
                        if let Some(body) = document.body() {
                            let _ = body.append_child(&a);
                            if let Some(html_a) = a.dyn_ref::<web_sys::HtmlElement>() {
                                html_a.click();
                            }
                            let _ = body.remove_child(&a);
                        }
                        let _ = web_sys::Url::revoke_object_url(&url);
                    }
                }
            }
        }
    }
}

fn escape_csv_field(field: &str) -> String {
    // A leading = + - @ (or tab / CR) makes spreadsheets evaluate the cell as
    // a formula; display names are user-controlled.
    let field = if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{field}")
    } else {
        field.to_string()
    };
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field
    }
}

fn session_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.session_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
}

fn session_set(key: &str, value: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
        let _ = storage.set_item(key, value);
    }
}

/// Grant admin to a Matrix user. With Pasion the flag is owned by the Pasion
/// account (and mirrored onto palpo, which refuses local changes), so it is
/// set there.
async fn grant_admin(user_id: &str, has_pasion: bool) -> bool {
    if !has_pasion {
        return users::update_user(user_id, serde_json::json!({"admin": true}))
            .await
            .is_ok();
    }
    let localpart = user_id
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or_default();
    match crate::api::pasion::pasion_get_user_by_username(localpart).await {
        Ok(account) => {
            crate::api::pasion::pasion_update_user(&account.id, serde_json::json!({"admin": true}))
                .await
                .is_ok()
        }
        Err(_) => false,
    }
}

#[component]
pub fn UserList() -> Element {
    let nav = use_navigator();

    let initial_page = session_get("users_page")
        .and_then(|p| p.parse::<u64>().ok())
        .unwrap_or(1);
    let initial_search = session_get("users_search").unwrap_or_default();

    let initial_search2 = initial_search.clone();
    let mut search_input = use_signal(move || initial_search.clone());
    let mut search = use_signal(move || initial_search2.clone());
    let mut page = use_signal(move || initial_page);
    // Accounts waiting for confirmation in the deactivate dialog; empty = closed.
    let mut deactivate_targets = use_signal(Vec::<String>::new);
    let mut debounce_task = use_signal(|| Option::<Task>::None);
    let mut per_page = use_signal(|| DEFAULT_PAGE_SIZE);
    let mut exporting = use_signal(|| false);
    let mut show_import_dialog = use_signal(|| false);
    let mut selected_users = use_signal(|| HashSet::<String>::new());
    let mut bulk_running = use_signal(|| false);
    let mut sort_by = use_signal(|| "name".to_string());
    let mut sort_dir = use_signal(|| "asc".to_string());

    // Column visibility: (admin, status, created)
    let mut col_admin_visible = use_signal(|| true);
    let mut col_status_visible = use_signal(|| true);
    let mut col_created_visible = use_signal(|| true);
    let mut col_dropdown_open = use_signal(|| false);

    // Persist page and search to sessionStorage
    {
        let page_sig = page;
        use_effect(move || {
            let p = *page_sig.read();
            session_set("users_page", &p.to_string());
        });
    }
    {
        let search_sig = search;
        use_effect(move || {
            let s = search_sig.read().clone();
            session_set("users_search", &s);
        });
    }

    let search_val = search.read().clone();
    let page_val = *page.read();
    let per_page_val = *per_page.read();
    let sort_by_val = sort_by.read().clone();
    let sort_dir_val = sort_dir.read().clone();

    let mut users_data = use_resource(move || {
        let search = search_val.clone();
        let order_by = sort_by_val.clone();
        let order = sort_dir_val.clone();
        async move { users::get_users_cached(page_val, per_page_val, &order_by, &order, &search).await }
    });

    let handle_export_csv = move |_: MouseEvent| {
        if *exporting.read() {
            return;
        }
        exporting.set(true);
        spawn(async move {
            match users::get_all_users_for_export("name", "asc").await {
                Ok(users_list) => {
                    let mut csv =
                        String::from("user_id,displayname,admin,deactivated,creation_ts\n");
                    for user in &users_list {
                        let displayname = user.user.displayname.clone().unwrap_or_default();
                        let line = format!(
                            "{},{},{},{},{}\n",
                            escape_csv_field(&user.id),
                            escape_csv_field(&displayname),
                            user.user.admin,
                            user.user.deactivated,
                            user.creation_ts_ms / 1000,
                        );
                        csv.push_str(&line);
                    }
                    download_csv("users_export.csv", &csv);
                    show_toast(
                        &format!("Exported {} users", users_list.len()),
                        ToastVariant::Success,
                    );
                }
                Err(e) => {
                    show_toast(
                        &format!("Failed to export users: {}", e.message),
                        ToastVariant::Error,
                    );
                }
            }
            exporting.set(false);
        });
    };

    let handle_deactivate_confirm = move |erase: bool| {
        let ids = deactivate_targets.read().clone();
        deactivate_targets.set(Vec::new());
        if ids.is_empty() || *bulk_running.read() {
            return;
        }
        bulk_running.set(true);
        spawn(async move {
            let total = ids.len();
            let results = stream::iter(ids.iter())
                .map(|uid| async move { (uid, users::deactivate_account(uid, erase).await) })
                .buffer_unordered(BULK_CONCURRENCY)
                .collect::<Vec<_>>()
                .await;
            let failures: Vec<String> = results
                .iter()
                .filter_map(|(uid, r)| r.as_ref().err().map(|e| format!("{uid}: {}", e.message)))
                .collect();
            match (total, results.first()) {
                (1, Some((uid, Ok(owner)))) => show_account_change_toast(uid, true, *owner),
                _ if failures.is_empty() => {
                    show_toast(&format!("Deactivated {total} users"), ToastVariant::Success)
                }
                _ => show_toast(
                    &format!(
                        "Deactivated {} of {total} users. Failed: {}",
                        total - failures.len(),
                        failures.join("; ")
                    ),
                    ToastVariant::Error,
                ),
            }
            for (uid, result) in &results {
                if result.is_ok() {
                    selected_users.write().remove(*uid);
                }
            }
            bulk_running.set(false);
            users_data.restart();
        });
    };

    let handle_bulk_deactivate = move |_: MouseEvent| {
        let ids: Vec<String> = selected_users.read().iter().cloned().collect();
        if !ids.is_empty() && !*bulk_running.read() {
            deactivate_targets.set(ids);
        }
    };

    let has_pasion = crate::utils::storage::get_item("pasion_url").is_some();
    let handle_bulk_set_admin = move |_: MouseEvent| {
        if *bulk_running.read() {
            return;
        }
        let ids: Vec<String> = selected_users.read().iter().cloned().collect();
        if ids.is_empty() {
            return;
        }
        bulk_running.set(true);
        spawn(async move {
            let total = ids.len();
            let results = stream::iter(ids.iter())
                .map(|uid| grant_admin(uid, has_pasion))
                .buffer_unordered(BULK_CONCURRENCY)
                .collect::<Vec<bool>>()
                .await;
            let success_count = results.iter().filter(|ok| **ok).count();
            let fail_count = total - success_count;
            if fail_count == 0 {
                show_toast(
                    &format!("Set admin on {} users", success_count),
                    ToastVariant::Success,
                );
            } else {
                show_toast(
                    &format!("Set admin on {success_count} of {total} users ({fail_count} failed)"),
                    ToastVariant::Error,
                );
            }
            selected_users.set(HashSet::new());
            bulk_running.set(false);
            users_data.restart();
        });
    };

    let is_exporting = *exporting.read();
    let is_bulk_running = *bulk_running.read();
    let selected_count = selected_users.read().len();

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("users.title"),
                description: "Matrix identities include people and Appservice puppets. Create and manage people’s accounts in Local Accounts.".to_string(),
                if crate::utils::storage::get_item("oauth_enabled").as_deref() != Some("true") { Button {
                    variant: ButtonVariant::Outline,
                    onclick: move |_| show_import_dialog.set(true),
                    Icon { name: "upload".to_string(), class: "h-4 w-4 mr-2".to_string() }
                    {t("users.import_csv")}
                } }
                Button {
                    variant: ButtonVariant::Outline,
                    disabled: is_exporting,
                    onclick: handle_export_csv,
                    Icon { name: "download".to_string(), class: "h-4 w-4 mr-2".to_string() }
                    if is_exporting { {t("users.exporting")} } else { {t("users.export_csv")} }
                }
                Link {
                    to: if crate::utils::storage::get_item("oauth_enabled").as_deref() == Some("true") { Route::PasionAccounts {} } else { Route::UserCreate {} },
                    class: "inline-flex items-center justify-center whitespace-nowrap rounded-lg text-sm font-medium h-10 px-4 py-2 btn-gradient",
                    Icon { name: "plus".to_string(), class: "h-4 w-4 mr-2".to_string() }
                    {t("users.create")}
                }
            }
            UsersTabs { active: UsersTab::Accounts }

            div { class: "flex items-center gap-4",
                div { class: "flex-1",
                    SearchInput {
                        placeholder: t("users.search"),
                        value: search_input.read().clone(),
                        oninput: move |evt: FormEvent| {
                            let value = evt.value();
                            search_input.set(value.clone());
                            // Cancel previous debounce timer
                            if let Some(task) = debounce_task.write().take() {
                                task.cancel();
                            }
                            let task = spawn(async move {
                                gloo_timers::future::TimeoutFuture::new(300).await;
                                search.set(value);
                                page.set(1);
                            });
                            debounce_task.set(Some(task));
                        },
                    }
                }
                div { class: "flex items-center gap-2",
                    label { class: "text-sm text-muted-foreground whitespace-nowrap", {t("common.rows")} }
                    select {
                        class: "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm",
                        value: "{per_page_val}",
                        onchange: move |evt: FormEvent| {
                            if let Ok(val) = evt.value().parse::<u64>() {
                                per_page.set(val);
                                page.set(1);
                            }
                        },
                        for size in PAGE_SIZE_OPTIONS.iter() {
                            option { value: "{size}", selected: *size == per_page_val, "{size}" }
                        }
                    }
                }
                // Column visibility toggle
                div { class: "relative",
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        onclick: move |_| {
                            let current = *col_dropdown_open.read();
                            col_dropdown_open.set(!current);
                        },
                        Icon { name: "columns".to_string(), class: "h-4 w-4 mr-2".to_string() }
                        {t("common.columns")}
                    }
                    if *col_dropdown_open.read() {
                        div { class: "absolute right-0 top-full mt-1 z-50 min-w-[160px] rounded-md border bg-background p-2 shadow-md",
                            div { class: "space-y-1",
                                label { class: "flex items-center gap-2 px-2 py-1.5 text-sm text-muted-foreground cursor-not-allowed",
                                    input {
                                        r#type: "checkbox",
                                        class: "h-4 w-4 rounded border-gray-300",
                                        checked: true,
                                        disabled: true,
                                    }
                                    {t("users.display_name")}
                                }
                                label { class: "flex items-center gap-2 px-2 py-1.5 text-sm cursor-pointer hover:bg-accent rounded",
                                    input {
                                        r#type: "checkbox",
                                        class: "h-4 w-4 rounded border-gray-300",
                                        checked: *col_admin_visible.read(),
                                        onchange: move |_| {
                                            let v = *col_admin_visible.read();
                                            col_admin_visible.set(!v);
                                        },
                                    }
                                    {t("users.admin")}
                                }
                                label { class: "flex items-center gap-2 px-2 py-1.5 text-sm cursor-pointer hover:bg-accent rounded",
                                    input {
                                        r#type: "checkbox",
                                        class: "h-4 w-4 rounded border-gray-300",
                                        checked: *col_status_visible.read(),
                                        onchange: move |_| {
                                            let v = *col_status_visible.read();
                                            col_status_visible.set(!v);
                                        },
                                    }
                                    {t("users.status")}
                                }
                                label { class: "flex items-center gap-2 px-2 py-1.5 text-sm cursor-pointer hover:bg-accent rounded",
                                    input {
                                        r#type: "checkbox",
                                        class: "h-4 w-4 rounded border-gray-300",
                                        checked: *col_created_visible.read(),
                                        onchange: move |_| {
                                            let v = *col_created_visible.read();
                                            col_created_visible.set(!v);
                                        },
                                    }
                                    {t("users.created")}
                                }
                            }
                        }
                    }
                }
            }

            // Bulk action bar
            if selected_count > 0 {
                div { class: "flex items-center gap-3 rounded-md border bg-muted/50 px-4 py-2",
                    span { class: "text-sm font-medium", "{selected_count} {t(\"common.selected\")}" }
                    Button {
                        variant: ButtonVariant::Destructive,
                        size: ButtonSize::Sm,
                        disabled: is_bulk_running,
                        onclick: handle_bulk_deactivate,
                        {t("users.deactivate_selected")}
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        size: ButtonSize::Sm,
                        disabled: is_bulk_running,
                        onclick: handle_bulk_set_admin,
                        {t("users.set_admin")}
                    }
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: is_bulk_running,
                        onclick: move |_| {
                            selected_users.set(HashSet::new());
                        },
                        {t("common.clear_selection")}
                    }
                }
            }

            match &*users_data.read() {
                Some(Ok(data)) => {
                    let all_ids: Vec<String> = data.data.iter().map(|u| u.id.clone()).collect();
                    let current_selected = selected_users.read().clone();
                    let all_selected = !all_ids.is_empty() && all_ids.iter().all(|id| current_selected.contains(id));

                    let current_sort_by = sort_by.read().clone();
                    let current_sort_dir = sort_dir.read().clone();

                    rsx! {
                    div { class: "rounded-md border",
                      div { class: "overflow-x-auto max-h-[600px] overflow-y-auto -mx-4 sm:mx-0",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { class: "w-10".to_string(),
                                        input {
                                            r#type: "checkbox",
                                            class: "h-4 w-4 rounded border-gray-300",
                                            checked: all_selected,
                                            onchange: {
                                                let all_ids = all_ids.clone();
                                                move |_| {
                                                    let mut set = selected_users.write();
                                                    let currently_all = all_ids.iter().all(|id| set.contains(id));
                                                    if currently_all {
                                                        for id in &all_ids {
                                                            set.remove(id);
                                                        }
                                                    } else {
                                                        for id in &all_ids {
                                                            set.insert(id.clone());
                                                        }
                                                    }
                                                }
                                            },
                                        }
                                    }
                                    TableHead { {t("users.user_id")} }
                                    TableHead {
                                        button {
                                            class: "flex items-center gap-1 hover:text-foreground",
                                            onclick: move |_| {
                                                if *sort_by.read() == "name" {
                                                    sort_dir.set(if *sort_dir.read() == "asc" { "desc".to_string() } else { "asc".to_string() });
                                                } else {
                                                    sort_by.set("name".to_string());
                                                    sort_dir.set("asc".to_string());
                                                }
                                                page.set(1);
                                            },
                                            {t("users.display_name")}
                                            {sort_indicator(&current_sort_by, &current_sort_dir, "name")}
                                        }
                                    }
                                    if *col_admin_visible.read() {
                                        TableHead {
                                            button {
                                                class: "flex items-center gap-1 hover:text-foreground",
                                                onclick: move |_| {
                                                    if *sort_by.read() == "admin" {
                                                        sort_dir.set(if *sort_dir.read() == "asc" { "desc".to_string() } else { "asc".to_string() });
                                                    } else {
                                                        sort_by.set("admin".to_string());
                                                        sort_dir.set("asc".to_string());
                                                    }
                                                    page.set(1);
                                                },
                                                {t("users.admin")}
                                                {sort_indicator(&current_sort_by, &current_sort_dir, "admin")}
                                            }
                                        }
                                    }
                                    if *col_status_visible.read() {
                                        TableHead { {t("users.status")} }
                                    }
                                    if *col_created_visible.read() {
                                        TableHead {
                                            button {
                                                class: "flex items-center gap-1 hover:text-foreground",
                                                onclick: move |_| {
                                                    if *sort_by.read() == "creation_ts" {
                                                        sort_dir.set(if *sort_dir.read() == "asc" { "desc".to_string() } else { "asc".to_string() });
                                                    } else {
                                                        sort_by.set("creation_ts".to_string());
                                                        sort_dir.set("asc".to_string());
                                                    }
                                                    page.set(1);
                                                },
                                                {t("users.created")}
                                                {sort_indicator(&current_sort_by, &current_sort_dir, "creation_ts")}
                                            }
                                        }
                                    }
                                    TableHead { class: "text-right".to_string(), {t("users.actions")} }
                                }
                            }
                            TableBody {
                                if data.data.is_empty() {
                                    TableRow {
                                        TableCell { class: "p-0".to_string(), colspan: 99,
                                            EmptyState {
                                                icon: "users".to_string(),
                                                title: t("users.no_users"),
                                                description: "Try adjusting your search query or create a new user.".to_string(),
                                                action_label: t("users.create"),
                                                action_href: "/admin/users/create".to_string(),
                                            }
                                        }
                                    }
                                } else {
                                    for user in data.data.iter() {
                                        {
                                            let user_id = user.id.clone();
                                            let display_name = user.user.displayname.clone().unwrap_or_else(|| "-".to_string());
                                            let is_admin = user.user.admin;
                                            let is_deactivated = user.user.deactivated;
                                            let creation_ts_ms = user.creation_ts_ms;
                                            let user_id_for_deactivate = user.id.clone();
                                            let user_id_for_checkbox = user.id.clone();
                                            let is_checked = selected_users.read().contains(&user_id);

                                            rsx! {
                                                TableRow {
                                                    key: "{user_id}",
                                                    TableCell { class: "w-10".to_string(),
                                                        input {
                                                            r#type: "checkbox",
                                                            class: "h-4 w-4 rounded border-gray-300",
                                                            checked: is_checked,
                                                            onchange: move |_| {
                                                                let mut set = selected_users.write();
                                                                if set.contains(&user_id_for_checkbox) {
                                                                    set.remove(&user_id_for_checkbox);
                                                                } else {
                                                                    set.insert(user_id_for_checkbox.clone());
                                                                }
                                                            },
                                                        }
                                                    }
                                                    TableCell {
                                                        Link {
                                                            to: Route::UserShow { user_id: urlencoding::encode(&user_id).to_string() },
                                                            class: "font-medium text-primary hover:underline",
                                                            "{user_id}"
                                                        }
                                                    }
                                                    TableCell { "{display_name}" }
                                                    if *col_admin_visible.read() {
                                                        TableCell {
                                                            if is_admin {
                                                                Badge {
                                                                    variant: BadgeVariant::Default,
                                                                    Icon { name: "shield".to_string(), class: "h-3 w-3 mr-1".to_string() }
                                                                    {t("users.admin")}
                                                                }
                                                            } else {
                                                                Badge { variant: BadgeVariant::Secondary, if user_id.starts_with("@_hagency_agt_") {"Agent"} else if user_id.starts_with("@_hagency_") {"Hagency service"} else {{t("common.user")}} }
                                                            }
                                                        }
                                                    }
                                                    if *col_status_visible.read() {
                                                        TableCell {
                                                            if is_deactivated {
                                                                Badge { variant: BadgeVariant::Destructive, {t("users.deactivated")} }
                                                            } else {
                                                                Badge { variant: BadgeVariant::Success, {t("users.active")} }
                                                            }
                                                        }
                                                    }
                                                    if *col_created_visible.read() {
                                                        TableCell { class: "text-muted-foreground".to_string(),
                                                            RelativeTime { ts_ms: creation_ts_ms }
                                                        }
                                                    }
                                                    TableCell { class: "text-right".to_string(),
                                                        div { class: "flex items-center justify-end gap-2",
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: ButtonSize::Sm,
                                                                disabled: is_bulk_running,
                                                                onclick: {
                                                                    let uid = user_id_for_deactivate.clone();
                                                                    move |_| {
                                                                        let uid = uid.clone();
                                                                        if !is_deactivated {
                                                                            deactivate_targets.set(vec![uid]);
                                                                            return;
                                                                        }
                                                                        spawn(async move {
                                                                            match users::reactivate_account(&uid).await {
                                                                                Ok(owner) => {
                                                                                    show_account_change_toast(&uid, false, owner);
                                                                                    users_data.restart();
                                                                                }
                                                                                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                            }
                                                                        });
                                                                    }
                                                                },
                                                                if is_deactivated { {t("users.reactivate")} } else { {t("users.deactivate")} }
                                                            }
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: ButtonSize::Sm,
                                                                onclick: move |_| {
                                                                    nav.push(Route::UserShow {
                                                                        user_id: urlencoding::encode(&user_id).to_string(),
                                                                    });
                                                                },
                                                                {t("common.view")}
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

                    Pagination {
                        page: page_val,
                        total: data.total,
                        per_page: per_page_val,
                        on_page_change: move |p| page.set(p),
                    }
                }},
                Some(Err(e)) => rsx! {
                    div { class: "rounded-md bg-destructive/10 p-4",
                        div { class: "flex items-center justify-between",
                            p { class: "text-sm text-destructive", "Error: {e.message}" }
                            button {
                                class: "text-sm font-medium text-primary hover:underline",
                                onclick: move |_| users_data.restart(),
                                {t("common.retry")}
                            }
                        }
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        // Import CSV dialog
        Modal {
            open: *show_import_dialog.read(),
            on_close: move |_| show_import_dialog.set(false),
            size: ModalSize::Xl3,
            panel_class: "max-h-[80vh] overflow-y-auto".to_string(),
            div { class: "flex items-center justify-between mb-4",
                h2 { class: "text-lg font-semibold", {t("users.import_users")} }
                Button {
                    variant: ButtonVariant::Ghost,
                    onclick: move |_| show_import_dialog.set(false),
                    "X"
                }
            }
            UserImport {
                on_import_complete: move |_| users_data.restart(),
            }
        }

        DeactivateUserDialog {
            open: !deactivate_targets.read().is_empty(),
            user_ids: deactivate_targets.read().clone(),
            on_confirm: handle_deactivate_confirm,
            on_cancel: move |_| deactivate_targets.set(Vec::new()),
        }
    }
}

/// Returns a sort direction indicator arrow for a column header.
fn sort_indicator(current_sort_by: &str, current_sort_dir: &str, column: &str) -> &'static str {
    if current_sort_by == column {
        if current_sort_dir == "asc" {
            " \u{2191}" // up arrow
        } else {
            " \u{2193}" // down arrow
        }
    } else {
        " \u{2195}" // up-down arrow (neutral)
    }
}
