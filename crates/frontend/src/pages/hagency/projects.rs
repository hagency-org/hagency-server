use super::common::*;
use dioxus::prelude::*;
use serde_json::{Value, json};

#[component]
pub fn Projects() -> Element {
    let mut projects = use_resource(|| async {
        browser_auth::call("/projects", "GET", None)
            .await
            .map_err(|e| e.message)
    });
    let mut message = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut space = use_signal(String::new);
    let mut project_id = use_signal(String::new);
    let mut room = use_signal(String::new);
    let mut new_room = use_signal(String::new);
    let data = projects().and_then(Result::ok).unwrap_or(Value::Null);
    let selected = rows(&data, "projects")
        .into_iter()
        .find(|p| text(p, "id") == project_id());
    rsx! {div { class:"hg-page hg-stack",
        div {class:"hg-heading", h1 {{t("hagency.projects.title")}}
            button {class:"hg-button hg-secondary", onclick:move |_| projects.restart(), {t("hagency.refresh")}}
        }
        p {class:"hg-note", {t("hagency.projects.description")}}
        Notice {message}
        if projects().is_none() {p {role:"status", {t("hagency.projects.loading")}}}
        if let Some(Err(error))=projects() {p {role:"alert", "{error}"}}
        if projects().is_some_and(|r| r.is_ok()) && rows(&data,"projects").is_empty() {
            p {{t("hagency.projects.empty")}}
        }
        if !rows(&data,"projects").is_empty() {
            label {class:"hg-field", {t("hagency.project")}
                select {class:"hg-input", value:project_id(), disabled:busy(), onchange:move |e| {project_id.set(e.value());room.set(String::new());message.set(String::new());},
                    option {value:"", {t("hagency.projects.select")}}
                    for item in rows(&data,"projects") {option {value:text(&item,"id"), {name(&item,"spaceId")}}}
                }
            }
        }
        if let Some(item)=selected {
            section {class:"hg-card hg-stack", h2 {{name(&item,"spaceId")}}
                if !text(&item,"topic").is_empty() {p {{text(&item,"topic")}}}
                p {class:"hg-note", {t("hagency.projects.membership_note")}}
                details {summary {{t("hagency.identifiers")}} dl {class:"hg-facts", dt {{t("hagency.project")}} dd {{text(&item,"id")}} dt {{t("hagency.space")}} dd {{text(&item,"spaceId")}}}}
            }
            super::room_discovery::RoomDiscovery {project_id, room}
            super::project_controls::ProjectControls {key:"{project_id()}:project", project_id, room_id:String::new()}
            if !room().is_empty() {
                super::project_controls::ProjectControls {key:"{project_id()}:{room()}", project_id, room_id:room()}
            }
            details {class:"hg-card", summary {{t("hagency.projects.adopt_room")}}
                form {class:"hg-stack", onsubmit:move |e| {
                    e.prevent_default();if busy(){return;}let id=project_id();let room_id=new_room();busy.set(true);
                    spawn(async move {match mutate(&format!("/projects/{}/rooms/adopt",urlencoding::encode(&id)),"POST",json!({"roomId":room_id.trim()})).await {
                        Ok(_)=>{message.set({t("hagency.projects.room_registered")}.into());new_room.set(String::new());},Err(e)=>message.set(e)
                    }busy.set(false);});
                },
                    p {class:"hg-note", {t("hagency.projects.room_admin_note")}}
                    label {class:"hg-field", {t("hagency.room_id")} input {class:"hg-input",required:true,value:new_room(),placeholder:"!room:server",oninput:move |e| new_room.set(e.value())}}
                    div {class:"hg-actions", button {class:"hg-button",disabled:busy(),r#type:"submit",{t("hagency.projects.register_room")}}}
                }
            }
        }
        details {class:"hg-card", summary {{t("hagency.projects.adopt_space")}}
            form {class:"hg-stack", onsubmit:move |e| {
                e.prevent_default();if busy(){return;}let id=space();busy.set(true);
                spawn(async move {match mutate("/projects/adopt","POST",json!({"spaceId":id.trim()})).await {
                    Ok(_)=>{message.set({t("hagency.projects.registered")}.into());space.set(String::new());projects.restart();},Err(e)=>message.set(e)
                }busy.set(false);});
            },
                p {class:"hg-note", {t("hagency.projects.space_admin_note")}}
                label {class:"hg-field", {t("hagency.space_id")} input {class:"hg-input",required:true,value:space(),placeholder:"!space:server",oninput:move |e| space.set(e.value())}}
                div {class:"hg-actions",button {class:"hg-button",disabled:busy(),r#type:"submit",{t("hagency.projects.register")}}}
            }
        }
    }}
}
