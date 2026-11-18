use super::common::*;
use dioxus::prelude::*;
use serde_json::{Value, json};

#[component]
pub fn RoomDiscovery(project_id: Signal<String>, room: Signal<String>) -> Element {
    let mut rooms = use_resource(move || {
        let project = project_id();
        async move {
            let value = if project.is_empty() {
                Ok(json!({"rooms":[]}))
            } else {
                browser_auth::call(
                    &format!("/projects/{}/rooms", urlencoding::encode(&project)),
                    "GET",
                    None,
                )
                .await
                .map_err(|e| e.message)
            };
            (project, value)
        }
    });
    let mut agents = use_resource(move || {
        let project = project_id();
        let selected = room();
        async move {
            let value = if project.is_empty() || selected.is_empty() {
                Ok(json!({"agents":[]}))
            } else {
                browser_auth::call(
                    &format!(
                        "/projects/{}/rooms/{}/agents",
                        urlencoding::encode(&project),
                        urlencoding::encode(&selected)
                    ),
                    "GET",
                    None,
                )
                .await
                .map_err(|e| e.message)
            };
            (project, selected, value)
        }
    });
    let data = rooms()
        .filter(|(project, _)| *project == project_id())
        .and_then(|(_, v)| v.ok())
        .unwrap_or(Value::Null);
    let roster = agents()
        .filter(|(project, selected, _)| *project == project_id() && *selected == room())
        .and_then(|(_, _, v)| v.ok())
        .unwrap_or(Value::Null);
    rsx! {section {class:"hg-card hg-stack",
        div {class:"hg-heading", h2 {{t("hagency.rooms.title")}}
            button {class:"hg-button hg-secondary",disabled:project_id().is_empty(),onclick:move |_| {rooms.restart();agents.restart();},{t("hagency.refresh")}}
        }
        if rooms().is_none() {p {role:"status",{t("hagency.rooms.loading")}}}
        if let Some((project,Err(error)))=rooms() {if project==project_id() {p {role:"alert","{error}"}}}
        if !data.is_null()&&rows(&data,"rooms").is_empty() {p {class:"hg-note",{t("hagency.rooms.empty")}}}
        if !rows(&data,"rooms").is_empty() {
            label {class:"hg-field",{t("hagency.room")} select {class:"hg-input",value:room(),onchange:move |e| room.set(e.value()),
                option {value:"",{t("hagency.rooms.select")}}
                for item in rows(&data,"rooms") {option {value:text(&item,"roomId"),{name(&item,"roomId")}}}
            }}
        }
        if !room().is_empty() {
            h3 {{t("hagency.rooms.agents")}}
            if agents().is_none() {p {role:"status",{t("hagency.agents.loading")}}}
            if let Some((project,selected,Err(error)))=agents() {if project==project_id()&&selected==room() {p {role:"alert","{error}"}}}
            if !roster.is_null()&&rows(&roster,"agents").is_empty() {p {class:"hg-note",{t("hagency.rooms.no_agents")}}}
            for agent in rows(&roster,"agents") {div {class:"hg-stack",key:"{agent}",
                strong {{text(&agent,"displayName")}}
                p {class:"hg-note",{t_with("hagency.rooms.owner_binding", &[("owner", &text(&agent,"ownerMxid")), ("state", &state_label(&text(&agent,"bindingState")))])}}
                details {summary {{t("hagency.matrix_identity")}}code {{text(&agent,"puppetMxid")}}}
            }}
        }
        p {class:"hg-note",{t("hagency.rooms.note")}}
    }}
}
