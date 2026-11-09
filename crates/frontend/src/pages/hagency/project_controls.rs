use super::common::*;
use dioxus::prelude::*;
use serde_json::{Value, json};

/// A keyed editor owns one scope; switching selection discards its draft and pending responses.
#[component]
pub fn ProjectControls(project_id: Signal<String>, room_id: String) -> Element {
    let room_scope = !room_id.is_empty();
    let path = if room_scope {
        format!(
            "/projects/{}/rooms/{}",
            urlencoding::encode(&project_id()),
            urlencoding::encode(&room_id)
        )
    } else {
        format!("/projects/{}", urlencoding::encode(&project_id()))
    };
    let read_path = path.clone();
    let mut state = use_resource(move || {
        let path = read_path.clone();
        async move {
            browser_auth::call(&format!("{path}/service-state"), "GET", None)
                .await
                .map_err(|e| e.message)
        }
    });
    let mut mode = use_signal(|| "inherit_project".to_string());
    let mut default_allow = use_signal(|| true);
    let mut allow = use_signal(String::new);
    let mut deny = use_signal(String::new);
    let mut loaded_revision = use_signal(|| None::<i64>);
    let mut message = use_signal(String::new);
    let mut busy = use_signal(|| false);
    use_effect(move || {
        loaded_revision.set(None);
        if let Some(Ok(value)) = state() {
            match policy(&value) {
                Ok(policy) => {
                    mode.set(text(&policy, "mode"));
                    default_allow.set(policy["defaultAllow"] == true);
                    allow.set(member_lines(&policy, "allow"));
                    deny.set(member_lines(&policy, "deny"));
                    loaded_revision.set(value["revision"].as_i64());
                }
                Err(e) => message.set(e),
            }
        }
    });
    let data = state().and_then(Result::ok).unwrap_or(Value::Null);
    let can_manage = data["canManagePolicy"] == true;
    let own_paused = data[if room_scope {
        "roomPaused"
    } else {
        "projectPaused"
    }] == true;
    let policy_path = path.clone();
    let pause_path = path.clone();
    rsx! {section {class:"hg-card hg-stack",
        h2 {if room_scope {{t("hagency.access.room_title")}} else {{t("hagency.access.project_title")}}}
        if room_scope {p {class:"hg-note", "{room_id}"}}
        p {class:"hg-note", {t("hagency.access.description")}}
        Notice {message}
        if state().is_none() {p {role:"status", {t("hagency.access.loading")}}}
        if let Some(Err(error))=state() {p {role:"alert", "{error}"}}
        if let Some(Ok(_))=state() {
            p {if data["servicePaused"]==true {{t("hagency.access.paused")}} else {{t("hagency.access.no_pause")}}}
            if !can_manage {p {class:"hg-note", {t("hagency.access.read_only")}}}
        }
        div {class:"hg-actions", button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| state.restart(),{t("hagency.access.refresh")}}}
        if loaded_revision().is_some() {
            form {class:"hg-stack", onsubmit:move |e| {
                e.prevent_default();if busy()||!can_manage{return;}
                let Some(expected_revision)=loaded_revision()else{return;};
                let policy=if room_scope {match mode().as_str() {
                    "disabled"=>json!({"mode":"disabled"}),
                    "allow_list"=>json!({"mode":"allow_list","allow":members(&allow()),"deny":members(&deny())}),
                    _=>json!({"mode":"inherit_project","deny":members(&deny())}),
                }}else {json!({"defaultAllow":default_allow(),"allow":members(&allow()),"deny":members(&deny())})};
                let path=policy_path.clone();busy.set(true);
                spawn(async move {match browser_auth::call(&format!("{path}/creation-policy"),"PUT",Some(json!({"expectedRevision":expected_revision,"policy":policy}))).await {
                    Ok(_)=>{message.set({t("hagency.access.saved")}.into());state.restart();},
                    Err(e)=>{message.set(format!("{} Refresh access before retrying.",e.message));loaded_revision.set(None);}
                }busy.set(false);});
            },
                if room_scope {
                    label {class:"hg-field", {t("hagency.access.who")} select {class:"hg-input",value:mode(),disabled:busy()||!can_manage,onchange:move |e| mode.set(e.value()),
                        option {value:"inherit_project", {t("hagency.access.inherit")}}
                        option {value:"allow_list", {t("hagency.access.listed")}}
                        option {value:"disabled", {t("hagency.access.disabled")}}
                    }}
                } else {
                    label {input {r#type:"checkbox",checked:default_allow(),disabled:busy()||!can_manage,onchange:move |e| default_allow.set(e.checked())} " Members may add Agents by default"}
                }
                if (room_scope&&mode()=="allow_list")||(!room_scope&&!default_allow()) {
                    label {class:"hg-field", {t("hagency.access.allowed")} textarea {class:"hg-input",value:allow(),disabled:busy()||!can_manage,placeholder:"@user:server",oninput:move |e| allow.set(e.value())}}
                }
                if !room_scope||mode()!="disabled" {
                    label {class:"hg-field", {t("hagency.access.denied")} textarea {class:"hg-input",value:deny(),disabled:busy()||!can_manage,placeholder:{t("hagency.access.lines")},oninput:move |e| deny.set(e.value())}}
                }
                if can_manage {div {class:"hg-actions",button {class:"hg-button",r#type:"submit",disabled:busy()||loaded_revision().is_none(),{t("hagency.access.save")}}}}
            }
        }
        if can_manage {
            details {summary {{t("hagency.access.service_pause")}}
                p {class:"hg-note", {t("hagency.access.pause_note")}}
                if room_scope&&data["projectPaused"]==true {p {class:"hg-note", {t("hagency.access.project_paused")}}}
                div {class:"hg-actions", button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| {
                    let path=pause_path.clone();let action=if own_paused {"clear-service-pause"}else{"pause-service"};busy.set(true);
                    spawn(async move {match browser_auth::call(&format!("{path}/{action}"),"POST",None).await {
                        Ok(_)=>{message.set(if own_paused {{t("hagency.access.cleared")}} else {{t("hagency.access.service_paused")}}.into());state.restart();},Err(e)=>message.set(e.message)
                    }busy.set(false);});
                },if own_paused {{t("hagency.access.clear")}}else {{t("hagency.access.pause")}}}}
            }
        }
    }}
}
