use dioxus::prelude::*;

use crate::router::Route;

/// Global signal: if Some, we are awaiting the second key of a two-key chord.
/// The f64 is the timestamp (milliseconds) when the first key was pressed.
static PENDING_KEY: GlobalSignal<Option<(String, f64)>> = GlobalSignal::new(|| None);

/// Whether the shortcut help modal is visible.
static SHOW_HELP: GlobalSignal<bool> = GlobalSignal::new(|| false);

/// Toggle the shortcut help modal.
pub fn toggle_help() {
    let mut show = SHOW_HELP.write();
    *show = !*show;
}

/// Extract the character string from a keyboard_types::Key, if it is a Character variant.
fn key_to_string(key: &keyboard_types::Key) -> Option<String> {
    match key {
        keyboard_types::Key::Character(s) => Some(s.clone()),
        _ => None,
    }
}

#[component]
pub fn KeyboardShortcuts() -> Element {
    let nav = use_navigator();

    let handle_keydown = move |evt: KeyboardEvent| {
        // Ignore if the user is typing in an input, textarea or contenteditable.
        if let Some(window) = web_sys::window()
            && let Some(doc) = window.document()
            && let Some(active) = doc.active_element()
        {
            let tag = active.tag_name().to_uppercase();
            if tag == "INPUT" || tag == "TEXTAREA" || tag == "SELECT" {
                return;
            }
            if active
                .get_attribute("contenteditable")
                .map(|v| v == "true")
                .unwrap_or(false)
            {
                return;
            }
        }

        let key = evt.key();
        let key_str = match key_to_string(&key) {
            Some(s) => s,
            None => return,
        };

        // Current time for chord timeout.
        let now = js_sys::Date::now(); // milliseconds since epoch

        // Check if we are in a pending chord state.
        let pending = { PENDING_KEY.read().clone() };
        if let Some((first_key, ts)) = pending {
            // Clear pending state regardless.
            *PENDING_KEY.write() = None;

            // Only accept if within 500ms.
            if now - ts <= 500.0 && first_key == "g" {
                match key_str.as_str() {
                    "p" => {
                        nav.push(Route::HagencyProjects {});
                        return;
                    }
                    "a" => {
                        nav.push(Route::HagencyAgents {});
                        return;
                    }
                    "d" if crate::router::is_server_admin() => {
                        nav.push(Route::Dashboard {});
                        return;
                    }
                    "u" if crate::router::is_server_admin() => {
                        nav.push(Route::UserList {});
                        return;
                    }
                    "r" if crate::router::is_server_admin() => {
                        nav.push(Route::RoomList {});
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Single-key shortcuts.
        match key_str.as_str() {
            "?" => {
                toggle_help();
            }
            "g" => {
                *PENDING_KEY.write() = Some(("g".to_string(), now));
            }
            _ => {}
        }
    };

    rsx! {
        div {
            tabindex: 0,
            onkeydown: handle_keydown,
            style: "outline:none;",
        }
        ShortcutHelpModal {}
    }
}

#[component]
fn ShortcutHelpModal() -> Element {
    let show = *SHOW_HELP.read();

    if !show {
        return rsx! {};
    }

    rsx! {
        // Backdrop
        div {
            class: "fixed inset-0 z-50 flex items-center justify-center bg-black/50",
            onclick: move |_| {
                *SHOW_HELP.write() = false;
            },
            // Modal panel
            div {
                class: "relative w-full max-w-md rounded-lg border bg-card p-6 shadow-lg",
                onclick: move |evt: MouseEvent| {
                    evt.stop_propagation();
                },
                // Header
                div { class: "flex items-center justify-between mb-4",
                    h2 { class: "text-lg font-semibold", "Keyboard Shortcuts" }
                    button {
                        class: "text-muted-foreground hover:text-foreground",
                        onclick: move |_| {
                            *SHOW_HELP.write() = false;
                        },
                        "x"
                    }
                }
                // Table
                table { class: "w-full text-sm",
                    thead {
                        tr { class: "border-b",
                            th { class: "py-2 text-left font-medium", "Shortcut" }
                            th { class: "py-2 text-left font-medium", "Action" }
                        }
                    }
                    tbody {
                        tr { class: "border-b",
                            td { class: "py-2",
                                kbd { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "?" }
                            }
                            td { class: "py-2", "Toggle this help dialog" }
                        }
                        for (key,action) in [("p","Projects"),("a","My Agents")] {tr {class:"border-b",td {class:"py-2",kbd {"g"} " then " kbd {"{key}"}}td {class:"py-2","Go to {action}"}}}
                        if crate::router::is_server_admin() {
                        tr { class: "border-b",
                            td { class: "py-2",
                                kbd { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "g" }
                                span { class: "mx-1 text-muted-foreground", "then" }
                                kbd { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "d" }
                            }
                            td { class: "py-2", "Go to Dashboard" }
                        }
                        tr { class: "border-b",
                            td { class: "py-2",
                                kbd { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "g" }
                                span { class: "mx-1 text-muted-foreground", "then" }
                                kbd { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "u" }
                            }
                            td { class: "py-2", "Go to Users" }
                        }
                        tr {
                            td { class: "py-2",
                                kbd { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "g" }
                                span { class: "mx-1 text-muted-foreground", "then" }
                                kbd { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "r" }
                            }
                            td { class: "py-2", "Go to Rooms" }
                        }
                        }
                    }
                }
                // Footer note
                p { class: "mt-4 text-xs text-muted-foreground",
                    "Two-key shortcuts must be pressed within 500ms."
                }
            }
        }
    }
}
