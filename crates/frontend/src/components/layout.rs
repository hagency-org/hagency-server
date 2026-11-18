use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api::auth;
use crate::components::header::AppHeader;
use crate::components::keyboard_shortcuts::KeyboardShortcuts;
use crate::components::sidebar::AppSidebar;
use crate::components::ui::toast::Toaster;
use crate::router::Route;

/// Holds a `storage`-event listener registered on `window`; unregisters it on
/// `Drop` so the closure does not outlive the component.
struct StorageListener {
    closure: Closure<dyn FnMut()>,
}

impl Drop for StorageListener {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback(
                "storage",
                self.closure.as_ref().unchecked_ref(),
            );
        }
    }
}

#[component]
pub fn AppLayout(children: Element) -> Element {
    let collapsed = use_signal(|| false);
    let mut mobile_sidebar_open = use_signal(|| false);
    let nav = use_navigator();

    // Multi-tab session sync: when another tab logs out, it mutates
    // localStorage, which fires a `storage` event in *this* tab. React to that
    // event instead of polling every 2s. `use_hook` keeps the listener alive
    // for the component's lifetime; `StorageListener`'s Drop removes it on
    // unmount.
    use_hook(move || {
        let closure = Closure::wrap(Box::new(move || {
            if !auth::is_authenticated() {
                nav.replace(Route::LoginPage {});
            }
        }) as Box<dyn FnMut()>);

        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("storage", closure.as_ref().unchecked_ref());
        }

        std::rc::Rc::new(StorageListener { closure })
    });

    let mobile_sidebar_state = *mobile_sidebar_open.read();

    rsx! { KeyboardShortcuts {
        div { class: "flex h-screen overflow-hidden",
            div {
                class: if mobile_sidebar_state {
                    "sidebar-backdrop sidebar-backdrop-open"
                } else {
                    "sidebar-backdrop"
                },
                onclick: move |_| mobile_sidebar_open.set(false),
            }
            AppSidebar {
                collapsed,
                mobile_open: mobile_sidebar_open,
            }
            div { class: "flex flex-1 flex-col overflow-hidden",
                AppHeader {
                    collapsed,
                    mobile_sidebar_open,
                }
                main { class: "flex-1 overflow-auto p-4 md:p-6",
                    {children}
                }
            }
        }
        Toaster {}
    }}
}
