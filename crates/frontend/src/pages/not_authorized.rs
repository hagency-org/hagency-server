use dioxus::prelude::*;

use crate::api::auth;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::router::Route;
use crate::utils::i18n::t;
use crate::utils::storage;

/// Rendered by `AuthenticatedLayout` when the current user is
/// authenticated but does **not** have homeserver admin privileges.
///
/// Replaces the previous behaviour where every admin page would make its
/// own API call and surface a per-page `M_FORBIDDEN (403)` banner.
#[component]
pub fn NotAuthorizedPage() -> Element {
    let nav = use_navigator();
    let display_name = storage::get_item("user_display_name").unwrap_or_default();
    let user_id = storage::get_item("user_id").unwrap_or_default();

    let handle_logout = move |_evt: MouseEvent| {
        auth::logout_and_redirect(nav);
    };

    rsx! {
        div { class: "hg-access-denied",
            div { class: "w-full max-w-md space-y-6 text-center",
                div { class: "mx-auto h-16 w-16 rounded-full bg-destructive/10 flex items-center justify-center",
                    crate::components::ui::icons::Icon {
                        name: "shield-off".to_string(),
                        class: "h-8 w-8 text-destructive".to_string(),
                    }
                }
                div { class: "space-y-2",
                    h1 { class: "text-2xl font-bold tracking-tight", {t("auth.not_admin")} }
                    p { class: "text-muted-foreground text-sm",
                        "This page requires server administrator privileges. You can continue using your own Projects and Agents."
                    }
                }
                if !display_name.is_empty() || !user_id.is_empty() {
                    div { class: "rounded-lg border glass-panel p-4 text-left space-y-1",
                        p { class: "text-xs uppercase tracking-wide text-muted-foreground",
                            "Signed in as"
                        }
                        if !display_name.is_empty() {
                            p { class: "font-medium", "{display_name}" }
                        }
                        if !user_id.is_empty() {
                            p { class: "text-xs text-muted-foreground break-all", "{user_id}" }
                        }
                    }
                }
                Button {
                    variant: ButtonVariant::Outline,
                    class: "w-full".to_string(),
                    onclick: handle_logout,
                    {t("auth.sign_out")}
                }
                Link { to: Route::HagencyProjects {}, class: "text-primary hover:underline", "Go to your projects" }
            }
        }
    }
}
