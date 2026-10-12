//! Pasion local account detail: profile, admin role, lock / deactivation,
//! password reset, emails and sessions.

use dioxus::prelude::*;

use crate::api::pasion;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::dialog::ConfirmDialog;
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::{BreadcrumbItem, Breadcrumbs, PageHeader};
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::pages::pasion::accounts::{AccountStatusBadges, short_time};
use crate::utils::i18n::t;
use crate::utils::storage;

/// Localpart of the signed-in admin (`@alice:example.org` → `alice`), used
/// to stop admins from locking themselves out.
fn own_localpart() -> Option<String> {
    let mxid = storage::get_item("user_id")?;
    let rest = mxid.strip_prefix('@')?;
    Some(rest.split(':').next()?.to_string())
}

#[derive(Clone, PartialEq)]
enum PendingAction {
    Promote,
    Demote,
    Lock,
    Unlock,
    Deactivate { erase: bool },
    Reactivate,
    EndBrowserSessions,
    FinishBrowserSession(String),
    FinishOAuth2Session(String),
    DeleteEmail { id: String, email: String },
}

impl PendingAction {
    fn title(&self) -> &'static str {
        match self {
            Self::Promote => "Grant administrator privileges",
            Self::Demote => "Revoke administrator privileges",
            Self::Lock => "Lock account",
            Self::Unlock => "Unlock account",
            Self::Deactivate { .. } => "Deactivate account",
            Self::Reactivate => "Reactivate account",
            Self::EndBrowserSessions => "Sign out of all browser sessions",
            Self::FinishBrowserSession(_) => "End browser session",
            Self::FinishOAuth2Session(_) => "End client session",
            Self::DeleteEmail { .. } => "Remove email address",
        }
    }

    fn description(&self, username: &str) -> String {
        match self {
            Self::Promote => format!(
                "{username} will be able to request the admin scope and manage this deployment through Hagency Server."
            ),
            Self::Demote => format!(
                "{username} will lose administrator access immediately: existing admin sessions stop working and the homeserver admin flag is revoked too."
            ),
            Self::Lock => format!(
                "{username} will not be able to sign in or use existing sessions until the account is unlocked."
            ),
            Self::Unlock => format!("{username} will be able to sign in again."),
            Self::Deactivate { erase: true } => format!(
                "{username} will be deactivated and their data erased on the homeserver. The erasure cannot be undone."
            ),
            Self::Deactivate { erase: false } => format!(
                "{username} will be deactivated on Pasion and the homeserver. The account can be reactivated later."
            ),
            Self::Reactivate => {
                format!("{username} will be reactivated on Pasion and the homeserver.")
            }
            Self::EndBrowserSessions => format!(
                "All active browser sessions of {username} will be ended. Matrix client sessions are not affected."
            ),
            Self::FinishBrowserSession(_) => "This browser session will be signed out.".to_string(),
            Self::FinishOAuth2Session(_) => {
                "This client session will be signed out and its tokens revoked.".to_string()
            }
            Self::DeleteEmail { email, .. } => format!("{email} will be removed from {username}."),
        }
    }

    fn destructive(&self) -> bool {
        !matches!(self, Self::Promote | Self::Unlock | Self::Reactivate)
    }
}

#[component]
pub fn PasionAccountShowPage(user_id: String) -> Element {
    let id = use_signal(|| user_id.clone());
    // Re-key the signal when the route param changes.
    use_effect(use_reactive!(|user_id| {
        let mut id = id;
        id.set(user_id);
    }));

    let mut user_res = use_resource(move || async move { pasion::pasion_get_user(&id()).await });
    let mut emails_res =
        use_resource(move || async move { pasion::pasion_get_user_emails(Some(&id())).await });
    let mut browser_res = use_resource(move || async move {
        pasion::pasion_get_browser_sessions(Some(&id()), true).await
    });
    let mut oauth_res = use_resource(move || async move {
        pasion::pasion_get_active_oauth2_sessions_for_user(&id()).await
    });

    let mut pending = use_signal(|| Option::<PendingAction>::None);
    let mut busy = use_signal(|| false);

    let mut display_name = use_signal(String::new);
    let mut locale = use_signal(String::new);
    let mut profile_loaded_for = use_signal(String::new);

    let mut new_password = use_signal(String::new);
    let mut skip_check = use_signal(|| false);
    let mut end_sessions_after_reset = use_signal(|| true);

    let mut erase_on_deactivate = use_signal(|| false);
    let mut new_email = use_signal(String::new);

    // Seed the profile form once per loaded account.
    use_effect(move || {
        if let Some(Ok(user)) = &*user_res.read()
            && *profile_loaded_for.peek() != user.id
        {
            display_name.set(user.display_name.clone().unwrap_or_default());
            locale.set(user.preferred_locale.clone().unwrap_or_default());
            profile_loaded_for.set(user.id.clone());
        }
    });

    let mut run_pending = move |_: ()| {
        let Some(action) = pending.read().clone() else {
            return;
        };
        let uid = id();
        busy.set(true);
        spawn(async move {
            let result: Result<String, String> = match &action {
                PendingAction::Promote => {
                    patch_user(
                        &uid,
                        serde_json::json!({ "admin": true }),
                        "Administrator privileges granted",
                    )
                    .await
                }
                PendingAction::Demote => {
                    patch_user(
                        &uid,
                        serde_json::json!({ "admin": false }),
                        "Administrator privileges revoked",
                    )
                    .await
                }
                PendingAction::Lock => {
                    patch_user(
                        &uid,
                        serde_json::json!({ "locked": true }),
                        "Account locked",
                    )
                    .await
                }
                PendingAction::Unlock => {
                    patch_user(
                        &uid,
                        serde_json::json!({ "locked": false }),
                        "Account unlocked",
                    )
                    .await
                }
                PendingAction::Deactivate { erase } => {
                    patch_user(
                        &uid,
                        serde_json::json!({ "deactivated": true, "hs_erase": erase }),
                        "Account deactivated",
                    )
                    .await
                }
                PendingAction::Reactivate => {
                    patch_user(
                        &uid,
                        serde_json::json!({ "deactivated": false }),
                        "Account reactivated",
                    )
                    .await
                }
                PendingAction::EndBrowserSessions => {
                    pasion::pasion_risk_action(&uid, "terminate_sessions", None)
                        .await
                        .map(|r| {
                            format!(
                                "Ended {} browser session(s)",
                                r.sessions_terminated.unwrap_or(0)
                            )
                        })
                        .map_err(|e| e.message)
                }
                PendingAction::FinishBrowserSession(sid) => {
                    pasion::pasion_finish_browser_session(sid)
                        .await
                        .map(|_| "Browser session ended".to_string())
                        .map_err(|e| e.message)
                }
                PendingAction::FinishOAuth2Session(sid) => {
                    pasion::pasion_finish_oauth2_session(sid)
                        .await
                        .map(|_| "Client session ended".to_string())
                        .map_err(|e| e.message)
                }
                PendingAction::DeleteEmail { id: email_id, .. } => {
                    pasion::pasion_delete_user_email(email_id)
                        .await
                        .map(|_| "Email removed".to_string())
                        .map_err(|e| e.message)
                }
            };
            match result {
                Ok(message) => show_toast(&message, ToastVariant::Success),
                Err(message) => show_toast(&format!("Failed: {message}"), ToastVariant::Error),
            }
            if matches!(action, PendingAction::Deactivate { .. }) {
                erase_on_deactivate.set(false);
            }
            busy.set(false);
            pending.set(None);
            user_res.restart();
            emails_res.restart();
            browser_res.restart();
            oauth_res.restart();
        });
    };

    let save_profile = move |_| {
        let uid = id();
        let name = display_name.read().trim().to_string();
        let loc = locale.read().trim().to_string();
        let body = serde_json::json!({
            "display_name": if name.is_empty() { serde_json::Value::Null } else { name.into() },
            "preferred_locale": if loc.is_empty() { serde_json::Value::Null } else { loc.into() },
        });
        busy.set(true);
        spawn(async move {
            match pasion::pasion_update_user(&uid, body).await {
                Ok(_) => {
                    show_toast("Profile updated", ToastVariant::Success);
                    user_res.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
            busy.set(false);
        });
    };

    let reset_password = move |_| {
        let uid = id();
        let password = new_password.read().clone();
        let skip = *skip_check.read();
        let end_sessions = *end_sessions_after_reset.read();
        if password.is_empty() {
            show_toast("Enter or generate a password first", ToastVariant::Error);
            return;
        }
        busy.set(true);
        spawn(async move {
            match pasion::pasion_set_password(&uid, &password, skip).await {
                Ok(()) => {
                    let mut message = "Password updated".to_string();
                    if end_sessions {
                        match pasion::pasion_risk_action(
                            &uid,
                            "terminate_sessions",
                            Some("password reset by admin"),
                        )
                        .await
                        {
                            Ok(r) => {
                                message = format!(
                                    "Password updated; ended {} browser session(s)",
                                    r.sessions_terminated.unwrap_or(0)
                                );
                            }
                            Err(e) => {
                                message = format!(
                                    "Password updated, but ending sessions failed: {}",
                                    e.message
                                );
                            }
                        }
                    }
                    show_toast(&message, ToastVariant::Success);
                    new_password.set(String::new());
                    browser_res.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
            busy.set(false);
        });
    };

    let add_email = move |_| {
        let uid = id();
        let email = new_email.read().trim().to_string();
        if email.is_empty() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match pasion::pasion_add_user_email(&uid, &email).await {
                Ok(_) => {
                    show_toast("Email added", ToastVariant::Success);
                    new_email.set(String::new());
                    emails_res.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
            busy.set(false);
        });
    };

    let is_busy = *busy.read();

    let user_read = user_res.read();
    let user = match &*user_read {
        Some(Ok(user)) => user.clone(),
        Some(Err(e)) => {
            return rsx! {
                div { class: "space-y-6",
                    Breadcrumbs {
                        items: vec![
                            BreadcrumbItem { label: t("pasion.accounts.title"), href: Some("/admin/pasion/accounts".to_string()) },
                            BreadcrumbItem { label: id(), href: None },
                        ],
                    }
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| user_res.restart(),
                    }
                }
            };
        }
        None => return rsx! { PageSkeleton {} },
    };
    drop(user_read);

    let username = user.username.clone();
    let is_self = own_localpart().as_deref() == Some(username.as_str());
    let is_locked = user.locked_at.is_some();
    let is_deactivated = user.deactivated_at.is_some();
    let title = user
        .display_name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| username.clone());
    let pending_action = pending.read().clone();

    rsx! {
        div { class: "space-y-6",
            Breadcrumbs {
                items: vec![
                    BreadcrumbItem { label: t("pasion.accounts.title"), href: Some("/admin/pasion/accounts".to_string()) },
                    BreadcrumbItem { label: username.clone(), href: None },
                ],
            }

            PageHeader {
                title: title,
                description: format!("{username} · {}", user.id),
                AccountStatusBadges { user: user.clone() }
            }

            if is_self {
                div { class: "rounded-md border border-yellow-500/20 bg-yellow-500/10 p-3 text-sm",
                    "This is your own account. Revoking admin, locking and deactivating are disabled here so you cannot lock yourself out."
                }
            }

            div { class: "grid gap-6 md:grid-cols-2",
                // ── Overview ─────────────────────────────────────────────
                Card {
                    CardHeader {
                        CardTitle { "Overview" }
                    }
                    CardContent { class: "space-y-2 text-sm".to_string(),
                        InfoRow { label: "Username", value: username.clone() }
                        InfoRow { label: "Pasion ID", value: user.id.clone() }
                        InfoRow { label: "Created", value: short_time(&user.created_at) }
                        InfoRow {
                            label: "Locked at",
                            value: user.locked_at.as_deref().map(short_time).unwrap_or_else(|| "-".to_string()),
                        }
                        InfoRow {
                            label: "Deactivated at",
                            value: user.deactivated_at.as_deref().map(short_time).unwrap_or_else(|| "-".to_string()),
                        }
                    }
                }

                // ── Profile ──────────────────────────────────────────────
                Card {
                    CardHeader {
                        CardTitle { "Profile" }
                        CardDescription { "Display name is synced to the homeserver." }
                    }
                    CardContent { class: "space-y-4".to_string(),
                        div { class: "space-y-2",
                            Label { r#for: "pasion-display-name", "Display name" }
                            Input {
                                id: "pasion-display-name".to_string(),
                                value: display_name.read().clone(),
                                disabled: is_busy || is_deactivated,
                                oninput: move |evt: FormEvent| display_name.set(evt.value()),
                            }
                        }
                        div { class: "space-y-2",
                            Label { r#for: "pasion-locale", "Preferred locale" }
                            Input {
                                id: "pasion-locale".to_string(),
                                placeholder: "en".to_string(),
                                value: locale.read().clone(),
                                disabled: is_busy || is_deactivated,
                                oninput: move |evt: FormEvent| locale.set(evt.value()),
                            }
                        }
                        Button {
                            disabled: is_busy || is_deactivated,
                            onclick: save_profile,
                            "Save profile"
                        }
                    }
                }

                // ── Role ─────────────────────────────────────────────────
                Card {
                    CardHeader {
                        CardTitle { "Administrator" }
                        CardDescription {
                            "Grants server administration, not ownership of other users’ Agents or Space/Room permissions. Regular users can still sign in to manage their own Projects and Agents."
                        }
                    }
                    CardContent { class: "flex items-center justify-between gap-4".to_string(),
                        if user.admin {
                            Badge { variant: BadgeVariant::Default,
                                Icon { name: "shield".to_string(), class: "h-3 w-3 mr-1".to_string() }
                                {t("pasion.accounts.admin")}
                            }
                            Button {
                                variant: ButtonVariant::Outline,
                                disabled: is_busy || is_self,
                                onclick: move |_| pending.set(Some(PendingAction::Demote)),
                                "Revoke admin"
                            }
                        } else {
                            Badge { variant: BadgeVariant::Secondary, "Regular user" }
                            Button {
                                disabled: is_busy || is_deactivated,
                                onclick: move |_| pending.set(Some(PendingAction::Promote)),
                                Icon { name: "shield".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                "Make admin"
                            }
                        }
                    }
                }

                // ── Account status ───────────────────────────────────────
                Card {
                    CardHeader {
                        CardTitle { "Account status" }
                        CardDescription {
                            "Locking blocks sign-in and existing sessions but keeps the account intact. Deactivation also deactivates the Matrix user."
                        }
                    }
                    CardContent { class: "space-y-4".to_string(),
                        div { class: "flex flex-wrap gap-2",
                            if is_locked && !is_deactivated {
                                Button {
                                    variant: ButtonVariant::Outline,
                                    disabled: is_busy,
                                    onclick: move |_| pending.set(Some(PendingAction::Unlock)),
                                    "Unlock"
                                }
                            } else if !is_locked {
                                Button {
                                    variant: ButtonVariant::Outline,
                                    disabled: is_busy || is_self,
                                    onclick: move |_| pending.set(Some(PendingAction::Lock)),
                                    Icon { name: "lock".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                    "Lock"
                                }
                            }
                            if is_deactivated {
                                Button {
                                    variant: ButtonVariant::Outline,
                                    disabled: is_busy,
                                    onclick: move |_| pending.set(Some(PendingAction::Reactivate)),
                                    "Reactivate"
                                }
                            } else {
                                Button {
                                    variant: ButtonVariant::Destructive,
                                    disabled: is_busy || is_self,
                                    onclick: move |_| {
                                        let erase = *erase_on_deactivate.read();
                                        pending.set(Some(PendingAction::Deactivate { erase }));
                                    },
                                    "Deactivate"
                                }
                            }
                        }
                        if !is_deactivated {
                            label { class: "flex items-center gap-2 text-sm",
                                input {
                                    r#type: "checkbox",
                                    checked: *erase_on_deactivate.read(),
                                    disabled: is_busy || is_self,
                                    onchange: move |evt: FormEvent| erase_on_deactivate.set(evt.checked()),
                                }
                                "Also erase the user's data on the homeserver (irreversible)"
                            }
                        }
                    }
                }

                // ── Password ─────────────────────────────────────────────
                Card {
                    CardHeader {
                        CardTitle { "Reset password" }
                        CardDescription {
                            "Sets a new password directly. Share it with the user through a secure channel."
                        }
                    }
                    CardContent { class: "space-y-4".to_string(),
                        div { class: "flex gap-2",
                            div { class: "flex-1",
                                Input {
                                    r#type: "password".to_string(),
                                    autocomplete: "new-password".to_string(),
                                    aria_label: "New password".to_string(),
                                    placeholder: "New password".to_string(),
                                    value: new_password.read().clone(),
                                    disabled: is_busy || is_deactivated,
                                    oninput: move |evt: FormEvent| new_password.set(evt.value()),
                                }
                            }
                            Button {
                                variant: ButtonVariant::Outline,
                                disabled: is_busy || is_deactivated,
                                onclick: move |_| {
                                    new_password.set(crate::utils::password::generate_random_password());
                                },
                                "Generate"
                            }
                        }
                        label { class: "flex items-center gap-2 text-sm",
                            input {
                                r#type: "checkbox",
                                checked: *skip_check.read(),
                                disabled: is_busy,
                                onchange: move |evt: FormEvent| skip_check.set(evt.checked()),
                            }
                            "Skip password complexity check"
                        }
                        label { class: "flex items-center gap-2 text-sm",
                            input {
                                r#type: "checkbox",
                                checked: *end_sessions_after_reset.read(),
                                disabled: is_busy,
                                onchange: move |evt: FormEvent| end_sessions_after_reset.set(evt.checked()),
                            }
                            "Sign out of all browser sessions afterwards"
                        }
                        if is_locked && !is_deactivated {
                            p { class: "text-xs text-muted-foreground",
                                "The account is locked; unlock it so the user can sign in with the new password."
                            }
                        }
                        Button {
                            disabled: is_busy || is_deactivated,
                            onclick: reset_password,
                            if is_busy {
                                Spinner { class: "mr-2".to_string() }
                            }
                            Icon { name: "key".to_string(), class: "h-4 w-4 mr-1".to_string() }
                            "Set password"
                        }
                    }
                }

                // ── Emails ───────────────────────────────────────────────
                Card {
                    CardHeader {
                        CardTitle { "Email addresses" }
                        CardDescription { "Used for sign-in, notifications and self-service account recovery." }
                    }
                    CardContent { class: "space-y-4".to_string(),
                        match &*emails_res.read() {
                            Some(Ok(emails)) if emails.is_empty() => rsx! {
                                p { class: "text-sm text-muted-foreground", "No email addresses." }
                            },
                            Some(Ok(emails)) => rsx! {
                                ul { class: "space-y-2",
                                    for email in emails.iter() {
                                        {
                                            let eid = email.id.clone();
                                            let addr = email.email.clone();
                                            let addr_for_delete = addr.clone();
                                            rsx! {
                                                li { key: "{eid}", class: "flex items-center justify-between gap-2 rounded-md border px-3 py-2 text-sm",
                                                    div { class: "flex flex-wrap items-center gap-2",
                                                        span { class: "font-mono", "{addr}" }
                                                        if email.is_primary {
                                                            Badge { variant: BadgeVariant::Default, "Primary" }
                                                        }
                                                        if email.confirmed_at.is_some() {
                                                            Badge { variant: BadgeVariant::Success, "Verified" }
                                                        } else {
                                                            Badge { variant: BadgeVariant::Outline, "Unverified" }
                                                        }
                                                    }
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: ButtonSize::Sm,
                                                        class: "text-destructive hover:text-destructive".to_string(),
                                                        disabled: is_busy,
                                                        onclick: move |_| {
                                                            pending.set(Some(PendingAction::DeleteEmail {
                                                                id: eid.clone(),
                                                                email: addr_for_delete.clone(),
                                                            }));
                                                        },
                                                        "Remove"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            Some(Err(e)) => rsx! {
                                ErrorBanner { message: e.message.clone(), on_retry: move |_| emails_res.restart() }
                            },
                            None => rsx! { Spinner {} },
                        }
                        div { class: "flex gap-2",
                            div { class: "flex-1",
                                Input {
                                    r#type: "email".to_string(),
                                    aria_label: "New email address".to_string(),
                                    placeholder: "user@example.com".to_string(),
                                    value: new_email.read().clone(),
                                    disabled: is_busy || is_deactivated,
                                    oninput: move |evt: FormEvent| new_email.set(evt.value()),
                                }
                            }
                            Button {
                                variant: ButtonVariant::Outline,
                                disabled: is_busy || is_deactivated,
                                onclick: add_email,
                                "Add"
                            }
                        }
                    }
                }
            }

            // ── Sessions ─────────────────────────────────────────────────
            Card {
                CardHeader {
                    div { class: "flex items-start justify-between gap-4",
                        div { class: "space-y-1",
                            CardTitle { "Active sessions" }
                            CardDescription { "Browser sessions on Pasion and OAuth2 client sessions (e.g. Matrix clients)." }
                        }
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: is_busy,
                            onclick: move |_| pending.set(Some(PendingAction::EndBrowserSessions)),
                            Icon { name: "log-out".to_string(), class: "h-4 w-4 mr-1".to_string() }
                            "End all browser sessions"
                        }
                    }
                }
                CardContent { class: "space-y-6".to_string(),
                    div { class: "space-y-2",
                        h3 { class: "text-sm font-medium", "Browser sessions" }
                        match &*browser_res.read() {
                            Some(Ok(sessions)) => rsx! {
                                div { class: "rounded-md border",
                                    Table {
                                        TableHeader {
                                            TableRow {
                                                TableHead { "Started" }
                                                TableHead { "Last active" }
                                                TableHead { "IP" }
                                                TableHead { "User agent" }
                                                TableHead { class: "text-right".to_string(), "" }
                                            }
                                        }
                                        TableBody {
                                            if sessions.is_empty() {
                                                EmptyRow { colspan: 5, message: "No active browser sessions".to_string() }
                                            }
                                            for session in sessions.iter() {
                                                {
                                                    let sid = session.id.clone();
                                                    let started = short_time(&session.created_at);
                                                    let last_active = session.last_active_at.as_deref().map(short_time).unwrap_or_else(|| "-".to_string());
                                                    let ip = session.last_active_ip.clone().unwrap_or_else(|| "-".to_string());
                                                    let ua = session.user_agent.clone().unwrap_or_else(|| "-".to_string());
                                                    rsx! {
                                                        TableRow { key: "{sid}",
                                                            TableCell { span { class: "text-xs", "{started}" } }
                                                            TableCell { span { class: "text-xs", "{last_active}" } }
                                                            TableCell { span { class: "text-xs font-mono", "{ip}" } }
                                                            TableCell { span { class: "block truncate text-xs text-muted-foreground", title: "{ua}", "{ua}" } }
                                                            TableCell { class: "text-right".to_string(),
                                                                Button {
                                                                    variant: ButtonVariant::Ghost,
                                                                    size: ButtonSize::Sm,
                                                                    disabled: is_busy,
                                                                    onclick: move |_| pending.set(Some(PendingAction::FinishBrowserSession(sid.clone()))),
                                                                    "End"
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
                                ErrorBanner { message: e.message.clone(), on_retry: move |_| browser_res.restart() }
                            },
                            None => rsx! { Spinner {} },
                        }
                    }
                    div { class: "space-y-2",
                        h3 { class: "text-sm font-medium", "Client sessions" }
                        match &*oauth_res.read() {
                            Some(Ok(sessions)) => rsx! {
                                div { class: "rounded-md border",
                                    Table {
                                        TableHeader {
                                            TableRow {
                                                TableHead { "Client" }
                                                TableHead { "Started" }
                                                TableHead { "Last active" }
                                                TableHead { "IP" }
                                                TableHead { class: "text-right".to_string(), "" }
                                            }
                                        }
                                        TableBody {
                                            if sessions.is_empty() {
                                                EmptyRow { colspan: 5, message: "No active client sessions".to_string() }
                                            }
                                            for session in sessions.iter() {
                                                {
                                                    let sid = session.id.clone();
                                                    let client = session.client_id.clone();
                                                    let started = short_time(&session.created_at);
                                                    let last_active = session.last_active_at.as_deref().map(short_time).unwrap_or_else(|| "-".to_string());
                                                    let ip = session.last_active_ip.clone().unwrap_or_else(|| "-".to_string());
                                                    rsx! {
                                                        TableRow { key: "{sid}",
                                                            TableCell { span { class: "text-xs font-mono", "{client}" } }
                                                            TableCell { span { class: "text-xs", "{started}" } }
                                                            TableCell { span { class: "text-xs", "{last_active}" } }
                                                            TableCell { span { class: "text-xs font-mono", "{ip}" } }
                                                            TableCell { class: "text-right".to_string(),
                                                                Button {
                                                                    variant: ButtonVariant::Ghost,
                                                                    size: ButtonSize::Sm,
                                                                    disabled: is_busy,
                                                                    onclick: move |_| pending.set(Some(PendingAction::FinishOAuth2Session(sid.clone()))),
                                                                    "End"
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
                                ErrorBanner { message: e.message.clone(), on_retry: move |_| oauth_res.restart() }
                            },
                            None => rsx! { Spinner {} },
                        }
                    }
                }
            }
        }

        if let Some(action) = pending_action {
            ConfirmDialog {
                open: true,
                title: action.title().to_string(),
                description: action.description(&username),
                confirm_text: if is_busy { "Working...".to_string() } else { "Confirm".to_string() },
                destructive: action.destructive(),
                on_confirm: move |_| {
                    if !*busy.peek() {
                        run_pending(());
                    }
                },
                on_cancel: move |_| {
                    if !*busy.peek() {
                        pending.set(None);
                    }
                },
            }
        }
    }
}

async fn patch_user(uid: &str, body: serde_json::Value, ok: &str) -> Result<String, String> {
    pasion::pasion_update_user(uid, body)
        .await
        .map(|_| ok.to_string())
        .map_err(|e| e.message)
}

#[component]
fn InfoRow(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "flex justify-between gap-4",
            span { class: "text-muted-foreground", "{label}" }
            span { class: "font-mono text-xs break-all text-right", "{value}" }
        }
    }
}
