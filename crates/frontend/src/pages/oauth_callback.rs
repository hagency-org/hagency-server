use dioxus::prelude::*;

use crate::api::auth;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::loading::Spinner;
use crate::router::Route;
use crate::utils::i18n::t;

/// OAuth2 callback page: exchanges the authorization code for tokens,
/// verifies admin status, and redirects to the dashboard.
#[component]
pub fn OAuthCallback(
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
) -> Element {
    let nav = use_navigator();
    let mut error_msg = use_signal(|| Option::<String>::None);
    let mut processing = use_signal(|| true);

    // Process the OAuth callback parameters
    use_effect(move || {
        let code = code.clone();
        let state = state.clone();
        let error = error.clone();
        let error_description = error_description.clone();

        spawn(async move {
            if let Some(err) = error {
                let desc = error_description.unwrap_or_else(|| err.clone());
                processing.set(false);
                error_msg.set(Some(desc));
                return;
            }

            let Some(code) = code else {
                processing.set(false);
                error_msg.set(Some("No authorization code received".into()));
                return;
            };

            // Step 1: Exchange code for tokens (validates `state` against the
            // value stashed in sessionStorage by start_oauth_login).
            match auth::handle_oauth_callback(&code, state.as_deref()).await {
                Ok(true) => return, // Same Pasion login, now authorizing its administrator role.
                Ok(false) => {}
                Err(e) => {
                    processing.set(false);
                    error_msg.set(Some(e.message));
                    return;
                }
            }

            // Step 2: Forget any verdict from a previous login in this tab;
            // `AuthenticatedLayout` probes the servers for the new user and
            // shows the "not authorized" page unless they are an admin.
            crate::router::reset_admin_cache();
            match auth::verify_admin().await {
                Ok(admin) => {
                    crate::router::set_admin_verdict(admin);
                    processing.set(false);
                    let return_route =
                        crate::utils::storage::get_session_item("oauth_return_route")
                            .and_then(|path| crate::router::restorable_route(&path, admin));
                    crate::utils::storage::remove_session_item("oauth_return_route");
                    nav.replace(return_route.unwrap_or(Route::Home {}));
                }
                Err(e) => {
                    processing.set(false);
                    error_msg.set(Some(e.message));
                }
            }
        });
    });

    let is_processing = *processing.read();

    rsx! {
        div { class: "flex min-h-screen items-center justify-center bg-background p-4",
            div { class: "w-full max-w-md space-y-6 text-center",
                div { class: "mx-auto h-16 w-16 rounded-full bg-primary/10 flex items-center justify-center",
                    crate::components::ui::icons::Icon {
                        name: "shield".to_string(),
                        class: "h-8 w-8 text-primary".to_string(),
                    }
                }

                if is_processing {
                    div { class: "space-y-3",
                        Spinner { class: "mx-auto".to_string() }
                        p { class: "text-muted-foreground", {t("auth.processing")} }
                    }
                }

                if let Some(ref err) = *error_msg.read() {
                    div { class: "rounded-lg border glass-panel p-6 shadow-sm space-y-4",
                        div { class: "rounded-md bg-destructive/10 p-3 text-sm text-destructive",
                            "{err}"
                        }
                        Button {
                            variant: ButtonVariant::Outline,
                            class: "w-full".to_string(),
                            onclick: move |_evt: MouseEvent| { nav.push(Route::LoginPage {}); },
                            {t("auth.try_again")}
                        }
                    }
                }
            }
        }
    }
}
