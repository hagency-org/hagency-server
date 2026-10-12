mod api;
mod components;
mod pages;
mod router;
mod types;
mod utils;

use dioxus::prelude::*;

fn main() {
    dioxus_logger::init(dioxus_logger::tracing::Level::INFO).expect("failed to init logger");

    components::theme::apply_theme();

    // Both component APIs are mounted on this origin by the Rust host.
    if let Some(origin) = web_sys::window().and_then(|w| w.location().origin().ok()) {
        utils::storage::set_item("pasion_url", &format!("{origin}/auth"));
        utils::storage::set_item("pasion_public_url", &format!("{origin}/auth"));
        utils::storage::set_item("home_server", &origin);

        utils::storage::remove_item("palpo_admin_url");
    }

    // Override the same-origin default with an explicit `palpo_admin_url` from
    // runtime config when one is supplied (mirrors how login.rs applies
    // pasion_public_url / oauth_client_id from config.json).
    wasm_bindgen_futures::spawn_local(async {
        let cfg = utils::config::load_runtime_config().await;
        utils::storage::set_item("server_name", &cfg.server_name);
        utils::storage::set_item(
            "oauth_enabled",
            if cfg.oauth_enabled { "true" } else { "false" },
        );
        utils::storage::set_item(
            "pasion_enabled",
            if cfg.pasion_enabled { "true" } else { "false" },
        );
        if !cfg.palpo_admin_url.trim().is_empty() {
            utils::storage::set_item("palpo_admin_url", cfg.palpo_admin_url.trim());
        } else {
            utils::storage::remove_item("palpo_admin_url");
        }
    });

    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        style { {include_str!("./style.css")} }
        router::AppRouter {}
    }
}
