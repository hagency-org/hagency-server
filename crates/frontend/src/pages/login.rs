use crate::{
    api::auth,
    utils::{config, storage},
};
use dioxus::prelude::*;
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
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    rsx! {div{class:"hg-public hg-page",
        h1{class:"text-2xl font-bold","Hagency Server"}
        p{"Sign in with your personal Matrix account through Pasion."}
        if let Some(config)=cfg(){p{class:"hg-note","{config.server_name}"}}
        if !error().is_empty(){p{role:"alert","{error}"}}
        button{class:"hg-button",disabled:busy()||!cfg().is_some_and(|c|c.oauth_enabled),onclick:move|_|{
            busy.set(true);spawn(async move{if let Err(e)=auth::start_login().await{error.set(e.message);}busy.set(false);});
        },"Sign in with Pasion"}
        a{href:"/auth/",class:"hg-link","Account center"}
        a{href:"/auth/register",class:"hg-link","Create account"}
    }}
}
