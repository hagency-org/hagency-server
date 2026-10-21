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
        div {class:"hg-heading", h2 {"Rooms"}
            button {class:"hg-button hg-secondary",disabled:project_id().is_empty(),onclick:move |_| {rooms.restart();agents.restart();},"Refresh"}
        }
        if rooms().is_none() {p {role:"status","Loading Rooms…"}}
        if let Some((project,Err(error)))=rooms() {if project==project_id() {p {role:"alert","{error}"}}}
        if !data.is_null()&&rows(&data,"rooms").is_empty() {p {class:"hg-note","No registered Rooms you currently belong to."}}
        if !rows(&data,"rooms").is_empty() {
            label {class:"hg-field","Room" select {class:"hg-input",value:room(),onchange:move |e| room.set(e.value()),
                option {value:"","Select a Room"}
                for item in rows(&data,"rooms") {option {value:text(&item,"roomId"),{name(&item,"roomId")}}}
            }}
        }
        if !room().is_empty() {
            h3 {"Agents in this Room"}
            if agents().is_none() {p {role:"status","Loading Agents…"}}
            if let Some((project,selected,Err(error)))=agents() {if project==project_id()&&selected==room() {p {role:"alert","{error}"}}}
            if !roster.is_null()&&rows(&roster,"agents").is_empty() {p {class:"hg-note","No visible Agents in this Room."}}
            for agent in rows(&roster,"agents") {div {class:"hg-stack",key:"{agent}",
                div {class:"hg-heading",strong {{text(&agent,"displayName")}}span {class:"hg-badge","Public service"}if agent["isMine"]==true {span {class:"hg-badge","Mine"}}}
                p {class:"hg-note",{format!("Owner: {} · Binding: {}",text(&agent,"ownerMxid"),text(&agent,"bindingState"))}}
                details {summary {"Matrix identity"}code {{text(&agent,"puppetMxid")}}}
            }}
        }
        p {class:"hg-note","Only joined, registered Rooms are shown. Binding state does not indicate whether an Agent’s device is online."}
    }}
}
