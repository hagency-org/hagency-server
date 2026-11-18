use std::rc::Rc;

use dioxus::prelude::*;
use wasm_bindgen::{JsCast, closure::Closure};

use crate::components::ui::dialog::Modal;
use crate::router::Route;

/// Whether the shortcut help modal is visible.
static SHOW_HELP: GlobalSignal<bool> = GlobalSignal::new(|| false);

/// Toggle the shortcut help modal.
pub fn toggle_help() {
    let mut show = SHOW_HELP.write();
    *show = !*show;
}

#[component]
pub fn KeyboardShortcuts(children: Element) -> Element {
    let nav = use_navigator();
    let mut chord_start = use_signal(|| None::<f64>);
    let handle_key = use_callback(move |event: web_sys::KeyboardEvent| {
        let target = event
            .target()
            .and_then(|v| v.dyn_into::<web_sys::Element>().ok());
        if event.default_prevented()
            || event.repeat()
            || event.is_composing()
            || event.ctrl_key()
            || event.alt_key()
            || event.meta_key()
            || target.as_ref().is_some_and(|el| {
                el.closest("input, textarea, select, dialog, [role=dialog]")
                    .ok()
                    .flatten()
                    .is_some()
                    || el
                        .dyn_ref::<web_sys::HtmlElement>()
                        .is_some_and(|el| el.is_content_editable())
            })
        {
            chord_start.set(None);
            return;
        }
        let now = js_sys::Date::now();
        let pending = chord_start.take();
        let key = event.key();
        if pending.is_some_and(|start| now - start <= 500.0) {
            let destination = match key.as_str() {
                "p" => Some(Route::HagencyProjects {}),
                "a" => Some(Route::HagencyAgents {}),
                "d" if crate::router::is_server_admin() => Some(Route::Dashboard {}),
                "u" if crate::router::is_server_admin() => Some(Route::UserList {}),
                "r" if crate::router::is_server_admin() => Some(Route::RoomList {}),
                _ => None,
            };
            if let Some(route) = destination {
                nav.push(route);
                return;
            }
        }
        match key.as_str() {
            "?" => toggle_help(),
            "g" => chord_start.set(Some(now)),
            _ => {}
        }
    });

    // A key pressed after clicking plain text targets the body, outside the layout.
    let listener = use_hook(move || {
        let doc = web_sys::window()?.document()?;
        let callback = Rc::new(Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event| {
                handle_key.call(event);
            },
        ));
        doc.add_event_listener_with_callback("keydown", callback.as_ref().as_ref().unchecked_ref())
            .ok()?;
        Some((doc, callback))
    });
    use_drop(move || {
        if let Some((doc, callback)) = listener {
            let _ = doc.remove_event_listener_with_callback(
                "keydown",
                callback.as_ref().as_ref().unchecked_ref(),
            );
        }
    });

    rsx! { {children} ShortcutHelpModal {} }
}

#[component]
fn ShortcutHelpModal() -> Element {
    rsx! {
        Modal {
            open: SHOW_HELP(),
            label: "Keyboard Shortcuts".to_string(),
            on_close: move |_| *SHOW_HELP.write() = false,
            div {
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Character("?".into()) {
                        event.stop_propagation();
                        *SHOW_HELP.write() = false;
                    }
                },
                // Header
                div { class: "flex items-center justify-between mb-4",
                    h2 { class: "text-lg font-semibold", "Keyboard Shortcuts" }
                    button {
                        aria_label: "Close keyboard shortcuts",
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
