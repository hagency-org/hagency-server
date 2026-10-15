use crate::{
    api::{auth, browser_auth},
    components::{
        theme::{get_resolved_theme, set_theme},
        ui::{
            button::{Button, ButtonSize},
            icons::Icon,
        },
    },
    router::Route,
    utils::{
        config,
        i18n::{Language, current_language, set_language, t},
        storage,
    },
};
use dioxus::prelude::*;

/// Public entry and login share the console's theme, components and OAuth flow.
#[component]
pub fn LoginPage() -> Element {
    let cfg = use_resource(|| async {
        let cfg = config::load_runtime_config().await;
        for (key, value) in [
            ("server_name", cfg.server_name.as_str()),
            ("pasion_public_url", cfg.pasion_public_url.as_str()),
            ("oauth_client_id", cfg.oauth_client_id.as_str()),
        ] {
            storage::set_item(key, value);
        }
        storage::set_item(
            "oauth_enabled",
            if cfg.oauth_enabled { "true" } else { "false" },
        );
        storage::set_item(
            "pasion_enabled",
            if cfg.pasion_enabled { "true" } else { "false" },
        );
        cfg
    });
    // One check on page load, including a retained HttpOnly session after reload.
    let session = use_resource(|| async { browser_auth::ensure_session().await.ok() });
    let mut dark = use_signal(|| get_resolved_theme() == "dark");
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    let nav = use_navigator();
    let signed_in = session().is_some_and(|s| s.is_some());
    let enabled = cfg().is_some_and(|c| c.oauth_enabled);
    let account_enabled = cfg().is_some_and(|c| c.pasion_enabled);

    rsx! {
        div { class: "hg-welcome",
            header { class: "hg-welcome-header",
                a { href: "/", class: "hg-welcome-brand", aria_label: "Hagency",
                    img { src: "/assets/hagency-mark.svg", alt: "", width: "28", height: "28" }
                    "Hagency"
                }
                div { class: "hg-welcome-controls",
                    select {
                        class: "hg-input", aria_label: t("language.select"),
                        value: current_language().code(),
                        onchange: move |evt: FormEvent| {
                            if let Some(language) = Language::from_code(&evt.value()) { set_language(language); }
                        },
                        for language in Language::all().iter() {
                            option { value: language.code(), selected: current_language() == *language, "{language.label()}" }
                        }
                    }
                    button {
                        class: "hg-welcome-theme", r#type: "button",
                        aria_label: t("welcome.theme"), title: t("welcome.theme"),
                        onclick: move |_| { let next = !dark(); set_theme(if next { "dark" } else { "light" }); dark.set(next); },
                        span { aria_hidden: "true", Icon { name: if dark() { "sun" } else { "moon" } } }
                    }
                }
            }
            main { class: "hg-welcome-main",
                section { class: "hg-welcome-card hg-card", aria_labelledby: "welcome-title",
                    div { class: "hg-welcome-mark", aria_hidden: "true",
                        img { src: "/assets/hagency-mark.svg", alt: "", width: "40", height: "40" }
                    }
                    h1 { id: "welcome-title", {t("welcome.title")} }
                    p { class: "hg-welcome-description", {t("welcome.description")} }
                    if let Some(config) = cfg() {
                        if !config.server_name.is_empty() {
                            p { class: "hg-welcome-server", Icon { name: "globe" } "{config.server_name}" }
                        }
                    }
                    div { class: "hg-welcome-signin",
                        if !error().is_empty() { p { class: "hg-notice hg-error", role: "alert", "{error}" } }
                        Button {
                            size: ButtonSize::Lg, class: "hg-welcome-primary",
                            disabled: busy() || (!signed_in && !enabled),
                            onclick: move |_| {
                                if signed_in { nav.push(Route::Home {}); }
                                else {
                                    busy.set(true);
                                    error.set(String::new());
                                    spawn(async move {
                                        if let Err(e) = auth::start_login().await { error.set(e.message); }
                                        busy.set(false);
                                    });
                                }
                            },
                            if busy() { {t("welcome.signing_in")} }
                            else if signed_in { {t("welcome.continue")} }
                            else { {t("welcome.sign_in")} }
                            span { aria_hidden: "true", Icon { name: "log-in" } }
                        }
                        p { class: "hg-note", role: "status",
                            if signed_in { {t("welcome.signed_in")} }
                            else if cfg().is_none() { {t("welcome.loading")} }
                            else if !enabled { {t("welcome.unavailable")} }
                            else { {t("welcome.sign_in_hint")} }
                        }
                    }
                    if account_enabled {
                        nav { class: "hg-welcome-links", aria_label: t("welcome.account_links"),
                            a { href: "/auth/register", class: "hg-link", {t("welcome.create_account")} }
                            a { href: "/auth/", class: "hg-link", {t("home.account")} }
                        }
                    }
                }
                p { class: "hg-welcome-caption", {t("welcome.caption")} }
            }
        }
    }
}
