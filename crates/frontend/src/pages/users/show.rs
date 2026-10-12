use dioxus::prelude::*;

use crate::api::{pasion, users};
use crate::components::deactivate_user_dialog::{DeactivateUserDialog, show_account_change_toast};
use crate::components::experimental_features::ExperimentalFeatures;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::dialog::Modal;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::{LoadingSkeleton, PageSkeleton};
use crate::components::ui::page_header::{BreadcrumbItem, Breadcrumbs, PageHeader};
use crate::components::ui::relative_time::RelativeTime;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::components::user_account_data::UserAccountData;
use crate::components::user_rate_limits::UserRateLimits;
use crate::router::Route;
use crate::utils::date::format_timestamp;
use crate::utils::i18n::t;

#[derive(Debug, Clone, PartialEq)]
enum Tab {
    Overview,
    Rooms,
    Devices,
    Sessions,
    AccountData,
    RateLimits,
    Features,
}

#[component]
pub fn UserShow(user_id: String) -> Element {
    let decoded_user_id = urlencoding::decode(&user_id)
        .map(|s| s.into_owned())
        .unwrap_or(user_id.clone());

    let decoded_for_resource = decoded_user_id.clone();
    let mut user_data = use_resource(move || {
        let id = decoded_for_resource.clone();
        async move { users::get_user(&id).await }
    });

    let mut active_tab = use_signal(|| Tab::Overview);
    let mut show_deactivate_dialog = use_signal(|| false);
    let mut show_reset_password_dialog = use_signal(|| false);
    let mut editing = use_signal(|| false);
    let mut edit_display_name = use_signal(|| String::new());
    let mut edit_admin = use_signal(|| false);
    let mut reset_password_value = use_signal(|| String::new());
    let nav = use_navigator();
    let service_identity = decoded_user_id.starts_with("@_hagency_");
    let has_pasion = crate::utils::storage::get_item("oauth_enabled").as_deref() == Some("true");

    rsx! {
        div { class: "space-y-6",
            Breadcrumbs {
                items: vec![
                    BreadcrumbItem { label: t("users.title"), href: Some("/admin/users".to_string()) },
                    BreadcrumbItem { label: decoded_user_id.clone(), href: None },
                ],
            }

            match &*user_data.read() {
                Some(Ok(user)) => {
                    let display_name = user.user.displayname.clone().unwrap_or_else(|| "-".to_string());
                    let is_admin = user.user.admin;
                    let is_deactivated = user.user.deactivated;
                    let is_guest = user.user.is_guest;
                    let is_shadow_banned = user.user.shadow_banned;
                    let is_suspended = user.user.suspended;
                    let creation_ts_ms = user.creation_ts_ms;
                    let user_id_str = user.id.clone();
                    let uid_for_actions = user_id_str.clone();
                    let uid_for_deactivate = user_id_str.clone();
                    let uid_for_reactivate = user_id_str.clone();
                    let uid_for_tabs = user_id_str.clone();
                    let uid_for_edit = user_id_str.clone();
                    let uid_for_reset_pw = user_id_str.clone();
                    let uid_for_shadow_ban = user_id_str.clone();
                    let uid_for_suspend = user_id_str.clone();
                    let current_tab = active_tab.read().clone();
                    let is_editing = *editing.read();

                    rsx! {
                        // Header with action buttons
                        div { class: "flex items-start justify-between",
                            PageHeader {
                                title: display_name.clone(),
                                description: user_id_str.clone(),
                            }
                            div { class: "flex gap-2",
                                if has_pasion && !service_identity {
                                    Button {
                                        variant: ButtonVariant::Outline,
                                        onclick: {
                                            let mxid = user_id_str.clone();
                                            move |_| {
                                                let localpart = mxid
                                                    .trim_start_matches('@')
                                                    .split(':')
                                                    .next()
                                                    .unwrap_or_default()
                                                    .to_string();
                                                spawn(async move {
                                                    match pasion::pasion_get_user_by_username(&localpart).await {
                                                        Ok(account) => {
                                                            nav.push(Route::PasionAccountShow { user_id: account.id });
                                                        }
                                                        Err(e) => show_toast(
                                                            &format!("No Pasion account for {localpart}: {}", e.message),
                                                            ToastVariant::Error,
                                                        ),
                                                    }
                                                });
                                            }
                                        },
                                        Icon { name: "user-check".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                        {t("users.pasion_account")}
                                    }
                                }
                                if !is_deactivated {
                                    Button {
                                        variant: ButtonVariant::Outline,
                                        onclick: {
                                            let dn = display_name.clone();
                                            move |_| {
                                                if !is_editing {
                                                    edit_display_name.set(dn.clone());
                                                    edit_admin.set(is_admin);
                                                }
                                                editing.set(!is_editing);
                                            }
                                        },
                                        if is_editing {
                                            Icon { name: "x".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                            {t("users.cancel_edit")}
                                        } else {
                                            Icon { name: "edit".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                            {t("users.edit")}
                                        }
                                    }
                                    if !has_pasion { Button {
                                        variant: ButtonVariant::Outline,
                                        onclick: move |_| {
                                            reset_password_value.set(String::new());
                                            show_reset_password_dialog.set(true);
                                        },
                                        Icon { name: "key".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                        {t("users.reset_password")}
                                    } }
                                    Button {
                                        variant: ButtonVariant::Outline,
                                        onclick: move |_| show_deactivate_dialog.set(true),
                                        {t("users.deactivate")}
                                    }
                                } else {
                                    Button {
                                        variant: ButtonVariant::Outline,
                                        onclick: {
                                            let uid = uid_for_reactivate.clone();
                                            move |_| {
                                                let uid = uid.clone();
                                                spawn(async move {
                                                    match users::reactivate_account(&uid).await {
                                                        Ok(owner) => {
                                                            show_account_change_toast(&uid, false, owner);
                                                            user_data.restart();
                                                        }
                                                        Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                    }
                                                });
                                            }
                                        },
                                        {t("users.reactivate")}
                                    }
                                }
                            }
                        }

                        // Tab navigation
                        div { class: "flex border-b",
                            {
                                let tabs = vec![
                                    (Tab::Overview, t("users.overview")),
                                    (Tab::Rooms, t("users.rooms")),
                                    (Tab::Devices, t("users.devices")),
                                    (Tab::Sessions, t("users.sessions")),
                                    (Tab::AccountData, t("users.account_data")),
                                    (Tab::RateLimits, t("users.rate_limits")),
                                    (Tab::Features, t("users.features")),
                                ];
                                rsx! {
                                    for (tab, label) in tabs.iter() {
                                        {
                                            let is_active = current_tab == *tab;
                                            let tab_val = tab.clone();
                                            let label = label.clone();
                                            rsx! {
                                                button {
                                                    key: "{label}",
                                                    class: if is_active {
                                                        "px-4 py-2 text-sm font-medium border-b-2 border-primary text-primary"
                                                    } else {
                                                        "px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground"
                                                    },
                                                    onclick: move |_| active_tab.set(tab_val.clone()),
                                                    "{label}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Tab content
                        match current_tab {
                            Tab::Overview => rsx! {
                                // Edit form (shown when editing)
                                if is_editing {
                                    Card {
                                        CardHeader {
                                            CardTitle { {t("users.edit")} }
                                        }
                                        CardContent {
                                            div { class: "space-y-4",
                                                div { class: "space-y-2",
                                                    Label { {t("users.display_name")} }
                                                    Input {
                                                        placeholder: t("users.display_name"),
                                                        value: edit_display_name.read().clone(),
                                                        oninput: move |evt: FormEvent| edit_display_name.set(evt.value()),
                                                    }
                                                }
                                                if service_identity {
                                                    p {class:"text-sm text-muted-foreground","Appservice identities have no personal Pasion account or administrator role."}
                                                } else if has_pasion {
                                                    // The homeserver admin flag mirrors the Pasion
                                                    // account's admin role; palpo refuses local changes.
                                                    p { class: "text-sm text-muted-foreground",
                                                        "Administrator privileges are managed on the Pasion account (use the Pasion account button above)."
                                                    }
                                                } else {
                                                    div { class: "flex items-center space-x-2",
                                                        input {
                                                            r#type: "checkbox",
                                                            id: "edit-admin",
                                                            class: "h-4 w-4 rounded border-input",
                                                            checked: *edit_admin.read(),
                                                            onchange: move |evt: FormEvent| {
                                                                edit_admin.set(evt.value() == "true");
                                                            },
                                                        }
                                                        Label { r#for: "edit-admin".to_string(), {t("users.admin_privileges")} }
                                                    }
                                                }
                                            }
                                        }
                                        CardFooter { class: "flex justify-end gap-2".to_string(),
                                            Button {
                                                variant: ButtonVariant::Outline,
                                                onclick: move |_| editing.set(false),
                                                {t("common.cancel")}
                                            }
                                            Button {
                                                onclick: {
                                                    let uid = uid_for_edit.clone();
                                                    move |_| {
                                                        let uid = uid.clone();
                                                        let dn = edit_display_name.read().clone();
                                                        let admin = *edit_admin.read();
                                                        spawn(async move {
                                                            let mut data = serde_json::json!({
                                                                "displayname": dn,
                                                            });
                                                            if !has_pasion {
                                                                data["admin"] = serde_json::Value::Bool(admin);
                                                            }
                                                            match users::update_user(&uid, data).await {
                                                                Ok(_) => {
                                                                    show_toast("User updated", ToastVariant::Success);
                                                                    editing.set(false);
                                                                    user_data.restart();
                                                                }
                                                                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                            }
                                                        });
                                                    }
                                                },
                                                {t("users.save_changes")}
                                            }
                                        }
                                    }
                                }

                                div { class: "grid gap-6 md:grid-cols-2",
                                    // User Information
                                    Card {
                                        CardHeader {
                                            CardTitle { {t("users.user_info")} }
                                        }
                                        CardContent {
                                            div { class: "space-y-4",
                                                InfoRow { label: t("users.user_id"), value: user_id_str.clone() }
                                                InfoRow { label: t("users.display_name"), value: display_name.clone() }
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("users.admin")} }
                                                    if is_admin {
                                                        Badge {
                                                            variant: BadgeVariant::Default,
                                                            Icon { name: "shield".to_string(), class: "h-3 w-3 mr-1".to_string() }
                                                            "Yes"
                                                        }
                                                    } else {
                                                        span { class: "text-sm", "No" }
                                                    }
                                                }
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("users.status")} }
                                                    if is_deactivated {
                                                        Badge { variant: BadgeVariant::Destructive, {t("users.deactivated")} }
                                                    } else {
                                                        Badge { variant: BadgeVariant::Success, {t("users.active")} }
                                                    }
                                                }
                                                // Shadow Ban toggle
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("users.shadow_banned")} }
                                                    div { class: "flex items-center gap-2",
                                                        if is_shadow_banned {
                                                            Badge { variant: BadgeVariant::Destructive, "Yes" }
                                                        } else {
                                                            span { class: "text-sm", "No" }
                                                        }
                                                        if !is_deactivated {
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: crate::components::ui::button::ButtonSize::Sm,
                                                                onclick: {
                                                                    let uid = uid_for_shadow_ban.clone();
                                                                    move |_| {
                                                                        let uid = uid.clone();
                                                                        let new_val = !is_shadow_banned;
                                                                        spawn(async move {
                                                                            let data = serde_json::json!({
                                                                                "shadow_banned": new_val,
                                                                            });
                                                                            match users::update_user(&uid, data).await {
                                                                                Ok(_) => {
                                                                                    let msg = if new_val { "User shadow banned" } else { "Shadow ban removed" };
                                                                                    show_toast(msg, ToastVariant::Success);
                                                                                    user_data.restart();
                                                                                }
                                                                                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                            }
                                                                        });
                                                                    }
                                                                },
                                                                if is_shadow_banned { {t("common.remove")} } else { {t("common.enable")} }
                                                            }
                                                        }
                                                    }
                                                }
                                                // Suspend toggle
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("users.suspended")} }
                                                    div { class: "flex items-center gap-2",
                                                        if is_suspended {
                                                            Badge { variant: BadgeVariant::Destructive, "Yes" }
                                                        } else {
                                                            span { class: "text-sm", "No" }
                                                        }
                                                        if !is_deactivated {
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: crate::components::ui::button::ButtonSize::Sm,
                                                                onclick: {
                                                                    let uid = uid_for_suspend.clone();
                                                                    move |_| {
                                                                        let uid = uid.clone();
                                                                        let new_val = !is_suspended;
                                                                        spawn(async move {
                                                                            let data = serde_json::json!({
                                                                                "suspended": new_val,
                                                                            });
                                                                            match users::update_user(&uid, data).await {
                                                                                Ok(_) => {
                                                                                    let msg = if new_val { "User suspended" } else { "Suspension removed" };
                                                                                    show_toast(msg, ToastVariant::Success);
                                                                                    user_data.restart();
                                                                                }
                                                                                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                            }
                                                                        });
                                                                    }
                                                                },
                                                                if is_suspended { {t("common.remove")} } else { {t("common.enable")} }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Account Details
                                    Card {
                                        CardHeader {
                                            CardTitle { {t("users.account_details")} }
                                        }
                                        CardContent {
                                            div { class: "space-y-4",
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("users.created")} }
                                                    span { class: "text-sm",
                                                        RelativeTime { ts_ms: creation_ts_ms }
                                                    }
                                                }
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("users.guest")} }
                                                    span { class: "text-sm",
                                                        if is_guest { "Yes" } else { "No" }
                                                    }
                                                }
                                                if user.user.locked {
                                                    div { class: "flex items-center justify-between py-2",
                                                        span { class: "text-sm font-medium text-muted-foreground", {t("users.locked")} }
                                                        Badge { variant: BadgeVariant::Destructive, "Yes" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Third-party IDs
                                Card {
                                    CardHeader {
                                        CardTitle { {t("users.third_party_ids")} }
                                    }
                                    CardContent {
                                        if user.user.threepids.is_empty() {
                                            p { class: "text-sm text-muted-foreground", "No third-party IDs associated." }
                                        } else {
                                            div { class: "space-y-2",
                                                for tp in user.user.threepids.iter() {
                                                    {
                                                        let medium = tp.medium.clone();
                                                        let address = tp.address.clone();
                                                        let is_verified = tp.validated_at.is_some();
                                                        let added = tp.added_at.map(format_timestamp).unwrap_or_else(|| "-".to_string());
                                                        let uid_for_3pid = user_id_str.clone();
                                                        let medium_for_delete = medium.clone();
                                                        let address_for_delete = address.clone();
                                                        let icon_name = if medium == "email" { "mail" } else { "phone" };

                                                        rsx! {
                                                            div {
                                                                key: "{medium}-{address}",
                                                                class: "flex items-center justify-between py-2 border-b last:border-0",
                                                                div { class: "flex items-center gap-3 min-w-0 flex-1",
                                                                    Icon { name: icon_name.to_string(), class: "h-4 w-4 text-muted-foreground shrink-0".to_string() }
                                                                    div { class: "min-w-0",
                                                                        p { class: "text-sm font-medium truncate", "{address}" }
                                                                        p { class: "text-xs text-muted-foreground", "Added: {added}" }
                                                                    }
                                                                }
                                                                div { class: "flex items-center gap-2 shrink-0",
                                                                    Badge { variant: BadgeVariant::Secondary, "{medium}" }
                                                                    if is_verified {
                                                                        Badge { variant: BadgeVariant::Success, {t("users.verified")} }
                                                                    } else {
                                                                        Badge { variant: BadgeVariant::Outline, {t("users.unverified")} }
                                                                    }
                                                                    if !is_deactivated {
                                                                        Button {
                                                                            variant: ButtonVariant::Ghost,
                                                                            size: crate::components::ui::button::ButtonSize::Sm,
                                                                            onclick: move |_| {
                                                                                let uid = uid_for_3pid.clone();
                                                                                let med = medium_for_delete.clone();
                                                                                let addr = address_for_delete.clone();
                                                                                spawn(async move {
                                                                                    match users::delete_user_threepid(&uid, &med, &addr).await {
                                                                                        Ok(_) => {
                                                                                            show_toast("3PID removed", ToastVariant::Success);
                                                                                            user_data.restart();
                                                                                        }
                                                                                        Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                                    }
                                                                                });
                                                                            },
                                                                            Icon { name: "trash".to_string(), class: "h-4 w-4 text-destructive".to_string() }
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
                            Tab::Rooms => rsx! {
                                UserJoinedRooms { user_id: uid_for_tabs.clone() }
                            },
                            Tab::Devices => rsx! {
                                UserDevices { user_id: uid_for_tabs.clone() }
                            },
                            Tab::Sessions => rsx! {
                                UserSessions { user_id: uid_for_tabs.clone() }
                            },
                            Tab::AccountData => rsx! {
                                UserAccountData { user_id: uid_for_tabs.clone() }
                            },
                            Tab::RateLimits => rsx! {
                                UserRateLimits { user_id: uid_for_tabs.clone() }
                            },
                            Tab::Features => rsx! {
                                ExperimentalFeatures { user_id: uid_for_tabs.clone() }
                            },
                        }

                        if service_identity {p {class:"text-sm text-muted-foreground", "This is an Appservice identity, not a personal Pasion account. Agent ownership and execution devices are managed by its owner in Hagency Desktop."}}
                        // Deactivate dialog
                        DeactivateUserDialog {
                            open: *show_deactivate_dialog.read(),
                            user_ids: vec![uid_for_deactivate.clone()],
                            on_confirm: move |erase: bool| {
                                let uid = uid_for_actions.clone();
                                show_deactivate_dialog.set(false);
                                spawn(async move {
                                    match users::deactivate_account(&uid, erase).await {
                                        Ok(owner) => {
                                            show_account_change_toast(&uid, true, owner);
                                            user_data.restart();
                                        }
                                        Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                    }
                                });
                            },
                            on_cancel: move |_| show_deactivate_dialog.set(false),
                        }

                        // Reset Password dialog
                        ResetPasswordDialog {
                            open: *show_reset_password_dialog.read(),
                            password_value: reset_password_value.read().clone(),
                            on_password_input: move |evt: FormEvent| {
                                reset_password_value.set(evt.value());
                            },
                            on_confirm: {
                                let uid = uid_for_reset_pw.clone();
                                move |_| {
                                    let uid = uid.clone();
                                    let pw = reset_password_value.read().clone();
                                    show_reset_password_dialog.set(false);
                                    if pw.is_empty() {
                                        show_toast("Password cannot be empty", ToastVariant::Error);
                                        return;
                                    }
                                    spawn(async move {
                                        match users::reset_password(&uid, &pw).await {
                                            Ok(_) => {
                                                show_toast("Password reset successfully", ToastVariant::Success);
                                            }
                                            Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                        }
                                    });
                                }
                            },
                            on_cancel: move |_| show_reset_password_dialog.set(false),
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    div { class: "rounded-md bg-destructive/10 p-4 text-sm text-destructive",
                        "Error loading user: {e.message}"
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}

/// Reset Password dialog with input field
#[component]
fn ResetPasswordDialog(
    open: bool,
    password_value: String,
    on_password_input: EventHandler<FormEvent>,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        Modal { open, on_close: move |_| on_cancel.call(()),
                div { class: "flex flex-col space-y-2 text-center sm:text-left",
                    h2 { class: "text-lg font-semibold", {t("users.reset_password")} }
                    p { class: "text-sm text-muted-foreground",
                        "Enter a new password for this user. The user will be logged out of all sessions."
                    }
                }
                div { class: "mt-4 space-y-2",
                    Label { {t("users.new_password")} }
                    Input {
                        r#type: "password".to_string(),
                        autocomplete: "new-password".to_string(),
                        placeholder: "Enter new password",
                        value: password_value,
                        oninput: move |evt: FormEvent| on_password_input.call(evt),
                    }
                }
                div { class: "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2 mt-4",
                    Button {
                        variant: ButtonVariant::Outline,
                        onclick: move |_| on_cancel.call(()),
                        {t("common.cancel")}
                    }
                    Button {
                        onclick: move |_| on_confirm.call(()),
                        {t("users.reset_password")}
                    }
                }
        }
    }
}

/// User's joined rooms list
#[component]
fn UserJoinedRooms(user_id: String) -> Element {
    let uid = user_id.clone();
    let mut page = use_signal(|| 1u64);

    let rooms_data = use_resource(move || {
        let id = uid.clone();
        let p = *page.read();
        async move { users::get_user_joined_rooms(&id, p, 25).await }
    });

    rsx! {
        Card {
            CardHeader {
                CardTitle { {t("users.joined_rooms")} }
            }
            CardContent {
                match &*rooms_data.read() {
                    Some(Ok(resp)) => rsx! {
                        if resp.data.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("users.no_rooms")} }
                        } else {
                            div { class: "space-y-1",
                                p { class: "text-sm text-muted-foreground mb-3", "{resp.total} rooms total" }
                                for room_id in resp.data.iter() {
                                    {
                                        let rid = room_id.clone();
                                        rsx! {
                                            div {
                                                key: "{rid}",
                                                class: "flex items-center py-2 border-b last:border-0",
                                                Icon { name: "message-square".to_string(), class: "h-4 w-4 text-muted-foreground mr-2".to_string() }
                                                Link {
                                                    to: crate::router::Route::RoomShow { room_id: urlencoding::encode(&rid).to_string() },
                                                    class: "text-sm font-mono text-primary hover:underline",
                                                    "{rid}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if resp.total > 25 {
                                {
                                    let current_page = *page.read();
                                    let total = resp.total;
                                    rsx! {
                                        div { class: "flex justify-center gap-2 mt-4",
                                            if current_page > 1 {
                                                Button {
                                                    variant: ButtonVariant::Outline,
                                                    onclick: move |_| page.set(current_page - 1),
                                                    {t("common.previous")}
                                                }
                                            }
                                            if current_page * 25 < total {
                                                Button {
                                                    variant: ButtonVariant::Outline,
                                                    onclick: move |_| page.set(current_page + 1),
                                                    {t("common.next")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        p { class: "text-sm text-destructive", "Error: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

/// User's devices list with admin actions
#[component]
fn UserDevices(user_id: String) -> Element {
    let uid = user_id.clone();
    let uid_for_revoke_all = user_id.clone();
    let mut revoking_all = use_signal(|| false);

    let mut devices_data = use_resource(move || {
        let id = uid.clone();
        async move { users::get_user_devices(&id, 1, 50).await }
    });

    rsx! {
        Card {
            CardHeader { class: "flex flex-row items-center justify-between space-y-0".to_string(),
                CardTitle { {t("users.devices")} }
                Button {
                    variant: ButtonVariant::Destructive,
                    size: crate::components::ui::button::ButtonSize::Sm,
                    disabled: *revoking_all.read(),
                    onclick: {
                        let uid = uid_for_revoke_all.clone();
                        move |_| {
                            let uid = uid.clone();
                            revoking_all.set(true);
                            spawn(async move {
                                match users::delete_all_user_devices(&uid).await {
                                    Ok(count) => {
                                        show_toast(&format!("Revoked {count} devices"), ToastVariant::Success);
                                        devices_data.restart();
                                    }
                                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                }
                                revoking_all.set(false);
                            });
                        }
                    },
                    {t("users.revoke_all_sessions")}
                }
            }
            CardContent {
                match &*devices_data.read() {
                    Some(Ok(resp)) => rsx! {
                        if resp.data.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("users.no_devices")} }
                        } else {
                            p { class: "text-sm text-muted-foreground mb-3", "{resp.total} devices" }
                            div { class: "space-y-3",
                                for device in resp.data.iter() {
                                    {
                                        let device_id = device.id.clone();
                                        let device_name = device.device.display_name.clone().unwrap_or_else(|| device_id.clone());
                                        let last_ip = device.device.last_seen_ip.clone();
                                        let last_ua = device.device.last_seen_user_agent.clone();
                                        let last_ts = device.device.last_seen_ts;
                                        let uid_for_delete = user_id.clone();
                                        let did_for_delete = device_id.clone();

                                        rsx! {
                                            div {
                                                key: "{device_id}",
                                                class: "flex items-start justify-between py-3 border-b last:border-0",
                                                div { class: "space-y-1 min-w-0 flex-1",
                                                    p { class: "text-sm font-medium", "{device_name}" }
                                                    p { class: "text-xs text-muted-foreground font-mono", "{device_id}" }
                                                    if let Some(ref ip) = last_ip {
                                                        div { class: "flex items-center gap-1",
                                                            Icon { name: "globe".to_string(), class: "h-3 w-3 text-muted-foreground".to_string() }
                                                            span { class: "text-xs text-muted-foreground", "{ip}" }
                                                        }
                                                    }
                                                    if let Some(ref ua) = last_ua {
                                                        p { class: "text-xs text-muted-foreground truncate max-w-md", "{ua}" }
                                                    }
                                                    if let Some(ts) = last_ts {
                                                        p { class: "text-xs text-muted-foreground",
                                                            "Last seen: {format_timestamp(ts)}"
                                                        }
                                                    }
                                                }
                                                Button {
                                                    variant: ButtonVariant::Ghost,
                                                    size: crate::components::ui::button::ButtonSize::Sm,
                                                    onclick: move |_| {
                                                        let uid = uid_for_delete.clone();
                                                        let did = did_for_delete.clone();
                                                        spawn(async move {
                                                            match users::delete_user_device(&uid, &did).await {
                                                                Ok(_) => {
                                                                    show_toast("Device revoked", ToastVariant::Success);
                                                                    devices_data.restart();
                                                                }
                                                                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                            }
                                                        });
                                                    },
                                                    Icon { name: "trash".to_string(), class: "h-4 w-4 text-destructive".to_string() }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        p { class: "text-sm text-destructive", "Error: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

/// User's sessions (whois) view
#[component]
fn UserSessions(user_id: String) -> Element {
    let uid = user_id.clone();

    let whois_data = use_resource(move || {
        let id = uid.clone();
        async move { users::whois_user(&id).await }
    });

    rsx! {
        Card {
            CardHeader {
                CardTitle { {t("users.sessions")} }
            }
            CardContent {
                match &*whois_data.read() {
                    Some(Ok(whois)) => rsx! {
                        if whois.devices.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("users.no_sessions")} }
                        } else {
                            div { class: "space-y-6",
                                for (device_id, device) in whois.devices.iter() {
                                    div { class: "space-y-2",
                                        div { class: "flex items-center gap-2",
                                            Icon { name: "monitor".to_string(), class: "h-4 w-4 text-muted-foreground".to_string() }
                                            h3 { class: "text-sm font-semibold", "Device: {device_id}" }
                                        }
                                        for session in device.sessions.iter() {
                                            for connection in session.connections.iter() {
                                                div { class: "ml-6 rounded-md border p-3 space-y-1",
                                                    div { class: "flex items-center justify-between",
                                                        span { class: "text-xs font-medium text-muted-foreground", "IP Address" }
                                                        span { class: "text-sm font-mono",
                                                            {connection.ip.clone().unwrap_or_else(|| "-".to_string())}
                                                        }
                                                    }
                                                    div { class: "flex items-center justify-between",
                                                        span { class: "text-xs font-medium text-muted-foreground", "User Agent" }
                                                        span { class: "text-sm truncate max-w-md",
                                                            {connection.user_agent.clone().unwrap_or_else(|| "-".to_string())}
                                                        }
                                                    }
                                                    div { class: "flex items-center justify-between",
                                                        span { class: "text-xs font-medium text-muted-foreground", "Last Seen" }
                                                        span { class: "text-sm",
                                                            {connection.last_seen.map(format_timestamp).unwrap_or_else(|| "-".to_string())}
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
                        p { class: "text-sm text-destructive", "Error: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

#[component]
fn InfoRow(label: String, value: String) -> Element {
    rsx! {
        div { class: "flex items-center justify-between py-2",
            span { class: "text-sm font-medium text-muted-foreground", "{label}" }
            span { class: "text-sm", "{value}" }
        }
    }
}
