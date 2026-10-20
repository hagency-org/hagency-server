use dioxus::dioxus_core::Task;
use dioxus::prelude::*;

use crate::api::users;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::Spinner;
use crate::components::ui::notifications::{
    NotificationSeverity, NotificationType, add_typed_notification,
};
use crate::components::ui::page_header::{BreadcrumbItem, Breadcrumbs};
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::router::Route;
use crate::types::{CreateUserRequest, Threepid};
use crate::utils::i18n::t;

/// Simple client-side email format validation.
fn is_valid_email(email: &str) -> bool {
    let re = regex_lite::Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap();
    re.is_match(email)
}

#[derive(Clone, PartialEq)]
enum UsernameStatus {
    Idle,
    Checking,
    Available,
    Taken,
    Error(String),
}

#[component]
pub fn UserCreate() -> Element {
    let nav = use_navigator();

    let mut username = use_signal(String::new);
    let mut display_name = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut is_admin = use_signal(|| false);
    // With Pasion the admin flag is owned by the Pasion account (palpo
    // refuses local changes), so it can't be set when creating here.
    let has_pasion = crate::utils::storage::get_item("pasion_url").is_some();
    let mut is_locked = use_signal(|| false);
    let mut email = use_signal(String::new);
    let mut phone = use_signal(String::new);
    let mut user_type = use_signal(|| String::from("regular"));
    let mut loading = use_signal(|| false);
    let mut username_status = use_signal(|| UsernameStatus::Idle);
    let mut debounce_task = use_signal(|| Option::<Task>::None);

    let handle_username_change = move |evt: FormEvent| {
        let value = evt.value();
        username.set(value.clone());

        // Cancel previous debounce
        if let Some(task) = debounce_task.write().take() {
            task.cancel();
        }

        if value.is_empty() {
            username_status.set(UsernameStatus::Idle);
            return;
        }

        username_status.set(UsernameStatus::Checking);

        let task = spawn(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            match users::check_username_available(&value).await {
                Ok(true) => username_status.set(UsernameStatus::Available),
                Ok(false) => username_status.set(UsernameStatus::Taken),
                Err(e) => username_status.set(UsernameStatus::Error(e.message)),
            }
        });
        debounce_task.set(Some(task));
    };

    let handle_submit = move |_: MouseEvent| {
        let user = username.read().clone();
        let name = display_name.read().clone();
        let pass = password.read().clone();
        let admin = !has_pasion && *is_admin.read();
        let locked = *is_locked.read();
        let utype = user_type.read().clone();
        let email_val = email.read().clone();
        let phone_val = phone.read().clone();

        if user.is_empty() || pass.is_empty() {
            show_toast("Username and password are required", ToastVariant::Error);
            return;
        }

        if !email_val.is_empty() && !is_valid_email(&email_val) {
            show_toast("Please enter a valid email address", ToastVariant::Error);
            return;
        }

        loading.set(true);

        spawn(async move {
            let user_type_value = match utype.as_str() {
                "bot" => Some("bot".to_string()),
                "support" => Some("support".to_string()),
                _ => None,
            };

            let mut threepids = Vec::new();
            if !email_val.is_empty() {
                threepids.push(Threepid {
                    medium: "email".to_string(),
                    address: email_val,
                    ..Default::default()
                });
            }
            if !phone_val.is_empty() {
                threepids.push(Threepid {
                    medium: "msisdn".to_string(),
                    address: phone_val,
                    ..Default::default()
                });
            }

            let request = CreateUserRequest {
                password: Some(pass),
                displayname: if name.is_empty() { None } else { Some(name) },
                admin,
                locked: if locked { Some(true) } else { None },
                user_type: user_type_value,
                threepids,
                ..Default::default()
            };

            match users::create_user(&user, request).await {
                Ok(_) => {
                    show_toast("User created successfully", ToastVariant::Success);
                    add_typed_notification(
                        &format!("User created: {user}"),
                        NotificationSeverity::Info,
                        Some(NotificationType::UserRegistered),
                    );
                    nav.push(Route::UserList {});
                }
                Err(e) => {
                    loading.set(false);
                    show_toast(
                        &format!("Failed to create user: {}", e.message),
                        ToastVariant::Error,
                    );
                }
            }
        });
    };

    let is_loading = *loading.read();
    let current_status = username_status.read().clone();

    rsx! {
        div { class: "space-y-6",
            Breadcrumbs {
                items: vec![
                    BreadcrumbItem { label: t("users.title"), href: Some("/admin/users".to_string()) },
                    BreadcrumbItem { label: t("users.create"), href: None },
                ],
            }

            div { class: "max-w-2xl",
                Card {
                    CardHeader {
                        CardTitle { {t("users.create_new")} }
                        CardDescription { {t("users.create_subtitle")} }
                    }
                    CardContent {
                        div { class: "responsive-form-grid",
                            div { class: "space-y-2 form-span-full",
                                Label { {t("users.username")} }
                                div { class: "relative",
                                    Input {
                                        placeholder: "username",
                                        value: username.read().clone(),
                                        disabled: is_loading,
                                        required: true,
                                        oninput: handle_username_change,
                                    }
                                    // Status icon inside the input area
                                    match &current_status {
                                        UsernameStatus::Checking => rsx! {
                                            div { class: "absolute right-3 top-2.5",
                                                Spinner { class: "h-4 w-4".to_string() }
                                            }
                                        },
                                        UsernameStatus::Available => rsx! {
                                            div { class: "absolute right-3 top-2.5 text-green-500",
                                                svg {
                                                    xmlns: "http://www.w3.org/2000/svg",
                                                    width: "20",
                                                    height: "20",
                                                    view_box: "0 0 24 24",
                                                    fill: "none",
                                                    stroke: "currentColor",
                                                    stroke_width: "2",
                                                    stroke_linecap: "round",
                                                    stroke_linejoin: "round",
                                                    path { d: "M20 6 9 17l-5-5" }
                                                }
                                            }
                                        },
                                        UsernameStatus::Taken => rsx! {
                                            div { class: "absolute right-3 top-2.5 text-red-500",
                                                svg {
                                                    xmlns: "http://www.w3.org/2000/svg",
                                                    width: "20",
                                                    height: "20",
                                                    view_box: "0 0 24 24",
                                                    fill: "none",
                                                    stroke: "currentColor",
                                                    stroke_width: "2",
                                                    stroke_linecap: "round",
                                                    stroke_linejoin: "round",
                                                    path { d: "M18 6 6 18" }
                                                    path { d: "m6 6 12 12" }
                                                }
                                            }
                                        },
                                        _ => rsx! {},
                                    }
                                }
                                match &current_status {
                                    UsernameStatus::Available => rsx! {
                                        p { class: "text-xs text-green-600", "Username available" }
                                    },
                                    UsernameStatus::Taken => rsx! {
                                        p { class: "text-xs text-red-600", "Username taken" }
                                    },
                                    UsernameStatus::Error(msg) => rsx! {
                                        p { class: "text-xs text-red-600", "Error checking username: {msg}" }
                                    },
                                    UsernameStatus::Checking => rsx! {
                                        p { class: "text-xs text-muted-foreground", "Checking availability..." }
                                    },
                                    UsernameStatus::Idle => rsx! {
                                        p { class: "text-xs text-muted-foreground",
                                            "The localpart of the Matrix ID (without @ and :server)"
                                        }
                                    },
                                }
                            }

                            div { class: "space-y-2",
                                Label { {t("users.display_name")} }
                                Input {
                                    placeholder: t("users.display_name"),
                                    value: display_name.read().clone(),
                                    disabled: is_loading,
                                    oninput: move |evt: FormEvent| display_name.set(evt.value()),
                                }
                            }

                            div { class: "space-y-2",
                                Label { {t("users.password")} }
                                Input {
                                    r#type: "password".to_string(),
                                    autocomplete: "new-password".to_string(),
                                    placeholder: t("users.password"),
                                    value: password.read().clone(),
                                    disabled: is_loading,
                                    required: true,
                                    oninput: move |evt: FormEvent| password.set(evt.value()),
                                }
                                PasswordStrengthIndicator { password: password.read().clone() }
                            }

                            div { class: "space-y-2",
                                Label { {t("users.email_optional")} }
                                Input {
                                    r#type: "email".to_string(),
                                    placeholder: "user@example.com",
                                    value: email.read().clone(),
                                    disabled: is_loading,
                                    oninput: move |evt: FormEvent| email.set(evt.value()),
                                }
                                {
                                    let email_val = email.read().clone();
                                    if !email_val.is_empty() && !is_valid_email(&email_val) {
                                        rsx! {
                                            p { class: "text-xs text-red-600", "Invalid email format" }
                                        }
                                    } else {
                                        rsx! {}
                                    }
                                }
                            }

                            div { class: "space-y-2",
                                Label { {t("users.phone_optional")} }
                                Input {
                                    placeholder: "+1234567890",
                                    value: phone.read().clone(),
                                    disabled: is_loading,
                                    oninput: move |evt: FormEvent| phone.set(evt.value()),
                                }
                                p { class: "text-xs text-muted-foreground",
                                    "International format recommended (e.g. +1234567890)"
                                }
                            }

                            div { class: "form-span-full responsive-option-grid",
                                if !has_pasion {
                                    div { class: "setting-toggle",
                                        input {
                                            r#type: "checkbox",
                                            id: "admin",
                                            class: "mt-1 h-4 w-4 rounded border-input",
                                            checked: *is_admin.read(),
                                            onchange: move |evt: FormEvent| {
                                                is_admin.set(evt.value() == "true");
                                            },
                                        }
                                        div { class: "flex-1",
                                            Label { r#for: "admin".to_string(), {t("users.admin_privileges")} }
                                        }
                                    }
                                }

                                div { class: "setting-toggle",
                                    input {
                                        r#type: "checkbox",
                                        id: "locked",
                                        class: "mt-1 h-4 w-4 rounded border-input",
                                        checked: *is_locked.read(),
                                        onchange: move |evt: FormEvent| {
                                            is_locked.set(evt.value() == "true");
                                        },
                                    }
                                    div { class: "flex-1",
                                        Label { r#for: "locked".to_string(), {t("users.locked")} }
                                    }
                                }
                            }

                            div { class: "space-y-2 form-span-full",
                                Label { {t("users.user_type")} }
                                select {
                                    id: "user_type",
                                    class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm touch-target",
                                    disabled: is_loading,
                                    value: user_type.read().clone(),
                                    onchange: move |evt: FormEvent| {
                                        user_type.set(evt.value());
                                    },
                                    option { value: "regular", {t("users.regular")} }
                                    option { value: "bot", {t("users.bot")} }
                                    option { value: "support", {t("users.support")} }
                                }
                                p { class: "text-xs text-muted-foreground",
                                    "Select the type of user account to create"
                                }
                            }
                        }
                    }
                    CardFooter { class: "responsive-action-row".to_string(),
                        Button {
                            variant: ButtonVariant::Outline,
                            onclick: move |_: MouseEvent| { nav.push(Route::UserList {}); },
                            {t("common.cancel")}
                        }
                        Button {
                            disabled: is_loading,
                            onclick: handle_submit,
                            if is_loading {
                                Spinner { class: "mr-2".to_string() }
                            }
                            {t("users.create")}
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PasswordStrength {
    Empty,
    Weak,
    Medium,
    Strong,
}

fn evaluate_password_strength(password: &str) -> PasswordStrength {
    if password.is_empty() {
        return PasswordStrength::Empty;
    }

    let len = password.len();
    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_alphanumeric());

    if len >= 10 && has_upper && has_lower && has_digit && has_special {
        PasswordStrength::Strong
    } else if len >= 8 && has_upper && has_lower {
        PasswordStrength::Medium
    } else {
        PasswordStrength::Weak
    }
}

#[component]
fn PasswordStrengthIndicator(password: String) -> Element {
    let strength = evaluate_password_strength(&password);

    if strength == PasswordStrength::Empty {
        return rsx! {};
    }

    let (label, color, width) = match strength {
        PasswordStrength::Weak => ("Weak", "bg-red-500", "w-1/3"),
        PasswordStrength::Medium => ("Medium", "bg-yellow-500", "w-2/3"),
        PasswordStrength::Strong => ("Strong", "bg-green-500", "w-full"),
        PasswordStrength::Empty => unreachable!(),
    };

    let text_color = match strength {
        PasswordStrength::Weak => "text-red-600",
        PasswordStrength::Medium => "text-yellow-600",
        PasswordStrength::Strong => "text-green-600",
        PasswordStrength::Empty => unreachable!(),
    };

    rsx! {
        div { class: "space-y-1",
            div { class: "h-1.5 w-full rounded-full bg-muted overflow-hidden",
                div { class: "h-full rounded-full transition-all duration-300 {color} {width}" }
            }
            p { class: "text-xs {text_color}", "{label}" }
        }
    }
}
