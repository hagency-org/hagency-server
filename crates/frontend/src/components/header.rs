use dioxus::prelude::*;

use crate::components::theme::{get_resolved_theme, set_theme};
use crate::components::ui::icons::Icon;
use crate::components::ui::notifications::{self, NOTIFICATIONS, NotificationSeverity};
use crate::utils::i18n::{Language, current_language, set_language, t};

/// Pre-computed header identity, derived once from localStorage.
#[derive(Clone, PartialEq)]
struct HeaderIdentity {
    /// Short label shown next to the avatar (user_id, falling back to name).
    label: String,
    /// Full "Name (id)" string used as the hover title / avatar alt.
    full: String,
    /// Single uppercase initial used when no avatar image is available.
    initial: String,
    /// Avatar image URL, if any.
    avatar_url: Option<String>,
}

impl HeaderIdentity {
    fn from_storage() -> Self {
        let display_name =
            crate::utils::storage::get_item("user_display_name").filter(|name| !name.is_empty());
        let user_id = crate::utils::storage::get_item("user_id").filter(|id| !id.is_empty());
        let avatar_url = crate::utils::storage::get_item("user_avatar_url");

        let label = user_id
            .clone()
            .or_else(|| display_name.clone())
            .unwrap_or_default();
        let full = match (display_name.as_deref(), user_id.as_deref()) {
            (Some(name), Some(id)) => format!("{name} ({id})"),
            (Some(name), None) => name.to_string(),
            (None, Some(id)) => id.to_string(),
            _ => String::new(),
        };
        let initial = display_name
            .as_deref()
            .or(user_id.as_deref())
            .unwrap_or("")
            .trim_start_matches('@')
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();

        HeaderIdentity {
            label,
            full,
            initial,
            avatar_url,
        }
    }
}

#[component]
pub fn AppHeader(collapsed: Signal<bool>, mobile_sidebar_open: Signal<bool>) -> Element {
    let mut is_collapsed = collapsed;
    let mut is_mobile_sidebar_open = mobile_sidebar_open;
    let mut dark_mode = use_signal(|| get_resolved_theme() == "dark");
    let mut show_notifications = use_signal(|| false);
    let nav = use_navigator();

    let unread = notifications::unread_count();

    // Identity comes from localStorage and only changes on login/logout, which
    // remounts the layout. Read it once into a memo instead of hitting
    // localStorage (4 reads + string work) on every header render.
    let identity = use_memo(HeaderIdentity::from_storage);

    rsx! {
        header { class: "flex h-14 items-center border-b px-4 lg:px-6",
            button {
                class: "inline-flex h-9 w-9 items-center justify-center rounded-lg text-sm font-medium hover:bg-accent hover:text-accent-foreground touch-target",
                title: "Toggle navigation",
                aria_label: "Toggle navigation",
                onclick: move |_| {
                    let is_mobile_view = web_sys::window()
                        .and_then(|window| window.inner_width().ok())
                        .and_then(|width| width.as_f64())
                        .map(|width| width < 768.0)
                        .unwrap_or(false);

                    if is_mobile_view {
                        let current = *is_mobile_sidebar_open.read();
                        is_mobile_sidebar_open.set(!current);
                    } else {
                        let current = *is_collapsed.read();
                        is_collapsed.set(!current);
                    }
                },
                svg {
                    class: "h-5 w-5",
                    xmlns: "http://www.w3.org/2000/svg",
                    width: "24", height: "24",
                    view_box: "0 0 24 24",
                    fill: "none", stroke: "currentColor",
                    stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                    line { x1: "4", x2: "20", y1: "12", y2: "12" }
                    line { x1: "4", x2: "20", y1: "6", y2: "6" }
                    line { x1: "4", x2: "20", y1: "18", y2: "18" }
                }
            }
            div { class: "flex-1" }
            // Language selector + Theme toggle + Notification bell + User info
            div { class: "app-header-controls",
                span {class:"hg-role", if crate::router::is_server_admin() {"Administrator"}else {"Personal account"}}
                a {href:"/auth/",class:"text-xs text-muted-foreground hover:underline","Account center"}
                // Language selector
                select {
                    class: "app-header-language h-9 rounded-lg border bg-background px-3 text-xs text-foreground touch-target",
                    value: "{current_language().code()}",
                    onchange: move |evt: FormEvent| {
                        if let Some(lang) = Language::from_code(&evt.value()) {
                            set_language(lang);
                        }
                    },
                    for lang in Language::all().iter() {
                        option {
                            value: "{lang.code()}",
                            selected: current_language() == *lang,
                            "{lang.label()}"
                        }
                    }
                }

                button {
                    class: "inline-flex h-9 w-9 items-center justify-center rounded-lg hover:bg-accent hover:text-accent-foreground touch-target",
                    title: if *dark_mode.read() { t("header.switch_light") } else { t("header.switch_dark") },
                    onclick: move |_| {
                        let new_dark = !*dark_mode.read();
                        dark_mode.set(new_dark);
                        set_theme(if new_dark { "dark" } else { "light" });
                    },
                    Icon {
                        name: if *dark_mode.read() { "sun".to_string() } else { "moon".to_string() },
                        class: "h-4 w-4".to_string(),
                    }
                }

                // Server notifications belong to administrators.
                if crate::router::is_server_admin() {
                div { class: "relative",
                    button {
                        class: "relative inline-flex h-9 w-9 items-center justify-center rounded-lg hover:bg-accent hover:text-accent-foreground touch-target",
                        title: t("header.notifications"),
                        onclick: move |_| {
                            let current = *show_notifications.read();
                            show_notifications.set(!current);
                        },
                        // Bell icon (SVG)
                        svg {
                            class: "h-4 w-4",
                            xmlns: "http://www.w3.org/2000/svg",
                            width: "24", height: "24",
                            view_box: "0 0 24 24",
                            fill: "none", stroke: "currentColor",
                            stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                            path { d: "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" }
                            path { d: "M10.3 21a1.94 1.94 0 0 0 3.4 0" }
                        }
                        // Unread badge
                        if unread > 0 {
                            span {
                                class: "absolute -top-1 -right-1 flex h-4 min-w-[1rem] items-center justify-center rounded-full bg-destructive px-1 text-[10px] font-bold text-destructive-foreground",
                                "{unread}"
                            }
                        }
                    }

                    // Dropdown panel
                    if *show_notifications.read() {
                        NotificationDropdown {
                            on_close: move |_: ()| show_notifications.set(false),
                        }
                    }
                }

                }
                {
                    let id = identity.read();
                    let full = id.full.clone();
                    let label = id.label.clone();
                    let initial = id.initial.clone();
                    let avatar_url = id.avatar_url.clone();
                    rsx! {
                        div { class: "app-header-session",
                            div { class: "app-header-user flex items-center gap-2", title: "{full}",
                                div { class: "flex h-8 w-8 items-center justify-center rounded-full bg-primary/10 text-xs font-semibold text-primary overflow-hidden",
                                    if let Some(url) = avatar_url.as_deref() {
                                        if !url.is_empty() {
                                            img { src: "{url}", alt: "{full}", class: "h-full w-full object-cover" }
                                        } else {
                                            span { "{initial}" }
                                        }
                                    } else {
                                        span { "{initial}" }
                                    }
                                }
                                div { class: "min-w-0 truncate text-xs font-mono text-muted-foreground", "{label}" }
                            }
                            button {
                                class: "app-header-logout inline-flex h-9 items-center justify-center rounded-lg border bg-background px-3 text-xs font-medium text-foreground hover:bg-accent hover:text-accent-foreground touch-target",
                                onclick: move |_| {
                                    crate::api::auth::logout_and_redirect(nav);
                                },
                                {t("nav.logout")}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NotificationDropdown(on_close: EventHandler<()>) -> Element {
    let notifs = NOTIFICATIONS.read();

    rsx! {
        div {
            class: "absolute right-0 top-10 z-50 w-80 rounded-lg border glass-panel shadow-lg",
            // Header
            div { class: "flex items-center justify-between border-b px-4 py-3",
                h4 { class: "text-sm font-semibold", {t("header.notifications")} }
                button {
                    class: "text-xs text-muted-foreground hover:text-foreground",
                    onclick: move |_| on_close.call(()),
                    {t("header.close")}
                }
            }
            // Notification list
            div { class: "max-h-80 overflow-y-auto",
                if notifs.is_empty() {
                    div { class: "px-4 py-8 text-center text-sm text-muted-foreground",
                        {t("header.no_notifications")}
                    }
                } else {
                    for notif in notifs.iter() {
                        {
                            let severity_color = match notif.severity {
                                NotificationSeverity::Info => "text-blue-500",
                                NotificationSeverity::Warning => "text-yellow-500",
                            };
                            let bg_class = if notif.read { "" } else { "bg-accent/50" };
                            let notif_id = notif.id;
                            rsx! {
                                div {
                                    key: "{notif_id}",
                                    class: "flex items-start gap-3 px-4 py-3 border-b last:border-0 {bg_class}",
                                    // Severity indicator dot
                                    span { class: "mt-0.5 h-2 w-2 shrink-0 rounded-full {severity_color}",
                                        style: "background: currentColor;"
                                    }
                                    div { class: "flex-1 min-w-0",
                                        p { class: "text-sm", "{notif.message}" }
                                        p { class: "text-xs text-muted-foreground mt-1", "{notif.timestamp}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Footer with actions
            if !notifs.is_empty() {
                div { class: "flex items-center justify-between border-t px-4 py-2",
                    button {
                        class: "text-xs text-primary hover:underline",
                        onclick: move |_| {
                            notifications::mark_all_read();
                        },
                        {t("header.mark_all_read")}
                    }
                    button {
                        class: "text-xs text-muted-foreground hover:text-foreground",
                        onclick: move |_| {
                            notifications::clear_notifications();
                        },
                        {t("header.clear_all")}
                    }
                }
            }
        }
    }
}
