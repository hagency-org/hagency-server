use super::common::*;
use crate::components::ui::icons::Icon;
use dioxus::prelude::*;
use serde_json::{Value, json};

#[component]
pub fn Agents() -> Element {
    let mut public_agents = use_resource(|| async {
        browser_auth::call("/agents/discover", "GET", None)
            .await
            .map_err(|e| e.message)
    });
    let mut agents = use_resource(|| async {
        browser_auth::call("/agents", "GET", None)
            .await
            .map_err(|e| e.message)
    });
    let mut devices = use_resource(|| async {
        browser_auth::call("/devices", "GET", None)
            .await
            .map_err(|e| e.message)
    });
    let projects = use_resource(|| async {
        browser_auth::call("/projects", "GET", None)
            .await
            .map_err(|e| e.message)
    });
    let mut selected = use_signal(String::new);
    let mut project = use_signal(String::new);
    let mut room = use_signal(String::new);
    let mut command = use_signal(browser_auth::operation_id);
    let mut message = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut bindings = use_resource(move || {
        let id = selected();
        async move {
            let result = if id.is_empty() {
                Ok(json!({"bindings":[]}))
            } else {
                browser_auth::call(&format!("/agents/{id}/bindings"), "GET", None)
                    .await
                    .map_err(|e| e.message)
            };
            (id, result)
        }
    });
    let rooms = use_resource(move || {
        let id = project();
        async move {
            let result = if id.is_empty() {
                Ok(json!({"rooms":[]}))
            } else {
                browser_auth::call(&format!("/projects/{id}/rooms"), "GET", None)
                    .await
                    .map_err(|e| e.message)
            };
            (id, result)
        }
    });
    let data = agents().and_then(Result::ok).unwrap_or(Value::Null);
    let device_data = devices().and_then(Result::ok).unwrap_or(Value::Null);
    let binding_data = bindings()
        .filter(|(id, _)| *id == selected())
        .and_then(|(_, r)| r.ok())
        .unwrap_or(Value::Null);
    let room_data = rooms()
        .filter(|(id, _)| *id == project())
        .and_then(|(_, r)| r.ok())
        .unwrap_or(Value::Null);
    let active_agent = rows(&data, "agents")
        .into_iter()
        .find(|a| text(a, "id") == selected());
    rsx! {div {class:"hg-page hg-stack",
        div {class:"hg-heading", h1 {"My Agents"}
            button {class:"hg-button hg-secondary", disabled:busy(), onclick:move |_| {agents.restart();public_agents.restart();devices.restart();bindings.restart();},"Refresh"}
        }
        p {class:"hg-note", "Agents belong to your account across the server. Use Hagency Desktop to create an Agent, assign its execution device, configure its model and resources, and start responding."}
        Notice {message}
        if agents().is_none() {p {role:"status","Loading Agents…"}}
        if let Some(Err(error))=agents() {p {role:"alert","{error}"}}
        if let Some(Err(error))=devices() {p {role:"alert","Unable to load execution devices: {error}"}}
        if agents().is_some_and(|r|r.is_ok())&&rows(&data,"agents").is_empty() {div {class:"hg-card hg-stack",h2 {"No Agents yet"}p {"Create an Agent in Hagency Desktop. It is assigned to that device and gets a private chat with you automatically."}}}
        div {class:"hg-grid",
            for agent in rows(&data,"agents") {
                {let id=text(&agent,"id");let device_id=text(&agent,"executionDeviceId");let device=rows(&device_data,"devices").into_iter().find(|d|text(d,"id")==device_id);
                rsx! {section {class:"hg-card hg-stack",key:"{id}",
                    div {class:"hg-heading",h2 {{text(&agent,"displayName")}}span {class:"hg-badge", "Mine"}span {class:"hg-badge", {audience(&agent)}}span {class:"hg-badge",{text(&agent,"state")}}}
                    p {class:"hg-note",{match device {Some(d)=>format!("Execution device: {}{}",text(&d,"name"),if d["revoked"]==true {" (authorization revoked)"}else{""}),None if device_id.is_empty()=>"No execution device assigned".into(),None=>format!("Execution device: {device_id}")}}}
                    details {summary {"Matrix identity"}code {{text(&agent,"puppetMxid")}}}
                    div {class:"hg-actions",button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| {selected.set(id.clone());project.set(String::new());room.set(String::new());message.set(String::new());command.set(browser_auth::operation_id());},"Manage Agent"}}
                }}}
            }
        }
        section {class:"hg-card hg-stack", h2 {"Public Agent services"}
            p {class:"hg-note", "Discover services shared by other users. Private Agents are visible only to their owner."}
            if public_agents().is_none() {p {role:"status","Loading public services…"}}
            if let Some(Err(error))=public_agents() {p {role:"alert","{error}"}}
            if let Some(Ok(catalog))=public_agents() {
                if rows(&catalog,"agents").is_empty() {p {class:"hg-note","No public Agent services are available."}}
                for agent in rows(&catalog,"agents") {div {class:"hg-stack",key:"{agent}",
                    div {class:"hg-heading",strong {{text(&agent,"displayName")}}span {class:"hg-badge","Public service"}if agent["isMine"]==true {span {class:"hg-badge","Mine"}}}
                    p {class:"hg-note",{format!("Creator: {}",text(&agent,"ownerMxid"))}}
                    details {summary {"Matrix identity"}code {{text(&agent,"puppetMxid")}}}
                }}
            }
        }
        if let Some(agent)=active_agent {
            section {class:"hg-card hg-stack",h2 {{text(&agent,"displayName")}}
                div {class:"hg-actions",span {class:"hg-badge","Mine"}span {class:"hg-badge",{audience(&agent)}}}
                p {class:"hg-note","Private Agents can only chat with you. Public Agents may serve members in shared Rooms. Leave every shared Room before switching to private."}
                div {class:"hg-actions",
                    for visibility in ["private","public"] {
                        button {class:"hg-button hg-secondary",disabled:busy()||!audience_editable(&agent)||agent["visibility"]==visibility,onclick:move |_| {
                            let id=selected();busy.set(true);spawn(async move {match mutate(&format!("/agents/{id}/visibility"),"PUT",json!({"visibility":visibility})).await {
                                Ok(_)=>{message.set("Service audience saved.".into());agents.restart();public_agents.restart();bindings.restart();},Err(e)=>message.set(audience_error(&e).into())
                            }busy.set(false);});
                        },if visibility=="public" {"Publish service"} else {"Make private"}}
                    }
                }
                p {class:"hg-note", "Server availability controls all of this Agent’s bindings. It does not start or stop the model on its device. Binding state does not prove the device is online."}
                if let Some(action)=availability_action(&text(&agent,"state")) {
                    div {class:"hg-actions",button {class:"hg-button",disabled:busy(),onclick:move |_| {
                        let id=selected();busy.set(true);spawn(async move {match mutate(&format!("/agents/{id}/{action}"),"POST",json!({})).await {
                            Ok(_)=>{message.set(if action=="pause" {"Agent paused on the server."}else{"Agent available on the server. Its device must also be running."}.into());agents.restart();bindings.restart();},Err(e)=>message.set(e)
                        }busy.set(false);});
                    },Icon {name:if action=="pause" {"pause".to_string()}else{"play".to_string()},class:"h-4 w-4".to_string()},if action=="pause" {"Pause Agent"}else {"Restore availability"}}}
                }
                h3 {"Connected chats"}
                if let Some((id,Err(error)))=bindings() {if id==selected(){p {role:"alert","{error}"}}}
                if binding_data.is_null() {p {role:"status","Loading chats…"}}
                for binding in rows(&binding_data,"bindings") {
                    {let id=text(&binding,"id");let is_direct=binding["scopeKind"]=="owner_direct";
                    rsx! {div {class:"hg-card hg-stack",key:"{id}",
                        h3 {if is_direct {"Private chat with you"}else {"Project Room"}}
                        code {{text(&binding,"roomId")}}
                        p {class:"hg-note",{format!("Server binding: {}",text(&binding,"state"))}}
                        div {class:"hg-actions",
                            if let Some(action)=binding_action(&text(&binding,"state")) {
                                {let id=id.clone();rsx! {button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| {
                                    let id=id.clone();busy.set(true);spawn(async move {match mutate(&format!("/bindings/{id}/{action}"),"POST",json!({})).await {
                                        Ok(result)=>{message.set(result);bindings.restart();},Err(e)=>message.set(e)
                                    }busy.set(false);});
                                },Icon {name:if action=="pause" {"pause".to_string()}else{"play".to_string()},class:"h-4 w-4".to_string()},if action=="pause" {"Pause in this chat"}else {"Resume in this chat"}}}}
                            }
                            if !is_direct&&text(&binding,"state")!="left" {
                                button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| {
                                    let id=id.clone();busy.set(true);spawn(async move {match mutate(&format!("/bindings/{id}"),"DELETE",json!({})).await {Ok(result)=>{message.set(result);bindings.restart();},Err(e)=>message.set(e)}busy.set(false);});
                                },"Leave Room"}
                            }
                        }
                    }}}
                }
                if agent["visibility"]=="public" {details {summary {"Add to a Project Room"}
                    form {class:"hg-stack",onsubmit:move |e| {
                        e.prevent_default();if busy()||room().is_empty()||project().is_empty(){return;}
                        let id=selected();let body=binding_body(&project(),&room(),&command());busy.set(true);
                        spawn(async move {match mutate(&format!("/agents/{id}/bindings"),"POST",body).await {
                            Ok(result)=>{message.set(result);command.set(browser_auth::operation_id());bindings.restart();},Err(e)=>message.set(e)
                        }busy.set(false);});
                    },
                        p {class:"hg-note", "You must belong to both the Space and Room, and their Agent access policies must allow you. The Hagency service account needs permission to invite the Agent. Encrypted Rooms are not yet supported for Agents."}
                        if let Some(Err(error))=projects() {p {role:"alert","{error}"}}
                        label {class:"hg-field","Project" select {class:"hg-input",disabled:busy(),value:project(),onchange:move |e| {project.set(e.value());room.set(String::new());command.set(browser_auth::operation_id());},
                            option {value:"","Select a Project"}
                            for item in rows(&projects().and_then(Result::ok).unwrap_or(Value::Null),"projects") {option {value:text(&item,"id"),{name(&item,"spaceId")}}}
                        }}
                        if let Some((id,Err(error)))=rooms() {if id==project(){p {role:"alert","{error}"}}}
                        label {class:"hg-field","Room" select {class:"hg-input",disabled:busy()||project().is_empty(),value:room(),onchange:move |e| {room.set(e.value());command.set(browser_auth::operation_id());},
                            option {value:"","Select a registered Room"}
                            for item in rows(&room_data,"rooms") {option {value:text(&item,"roomId"),{name(&item,"roomId")}}}
                        }}
                        div {class:"hg-actions",button {class:"hg-button",disabled:busy()||room().is_empty(),r#type:"submit","Add Agent"}}
                    }
                }
                }
                div {class:"hg-actions",button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| selected.set(String::new()),"Done"}}
            }
        }
    }}
}
fn audience(agent: &Value) -> &'static str {
    match agent["visibility"].as_str() {
        Some("public") => "Public service",
        Some("private") => "Private · owner chat only",
        _ => "Service audience unavailable. Update the server and refresh.",
    }
}
fn audience_editable(agent: &Value) -> bool {
    matches!(agent["visibility"].as_str(), Some("private" | "public"))
        && matches!(
            agent["state"].as_str(),
            Some("creating" | "active" | "suspended")
        )
}
fn audience_error(error: &str) -> &str {
    match error {
        "leave_shared_rooms_before_private" => {
            "Leave every shared Room and wait for departure to complete before switching to private."
        }
        "private_agent_owner_direct_only" => {
            "Private Agents can only chat with their creator. Publish this service before adding it to a shared Room."
        }
        _ => error,
    }
}
fn availability_action(state: &str) -> Option<&'static str> {
    match state {
        "active" => Some("pause"),
        "suspended" => Some("resume"),
        _ => None,
    }
}
fn binding_action(state: &str) -> Option<&'static str> {
    availability_action(state)
}
fn binding_body(project: &str, room: &str, key: &str) -> Value {
    json!({"projectId":project.trim(),"roomId":room.trim(),"idempotencyKey":key})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_or_retired_audience_cannot_be_published_from_web() {
        for agent in [
            json!({}),
            json!({"visibility":"public","state":"retired"}),
            json!({"visibility":"private","state":"retiring"}),
            json!({"state":"active"}),
        ] {
            assert!(!audience_editable(&agent));
        }
        assert!(audience_editable(
            &json!({"visibility":"private","state":"active"})
        ));
        assert!(audience(&json!({})).contains("unavailable"));
        assert!(audience_error("leave_shared_rooms_before_private").contains("departure"));
    }
    #[test]
    fn browser_binding_request_contains_only_room_scope_and_original_key() {
        assert_eq!(
            binding_body(" prj_owned ", " !discussion:server ", "bind-key"),
            json!({"projectId":"prj_owned","roomId":"!discussion:server","idempotencyKey":"bind-key"})
        );
    }
    #[test]
    fn creating_and_retired_agents_cannot_be_resumed_from_web() {
        assert_eq!(availability_action("active"), Some("pause"));
        assert_eq!(availability_action("suspended"), Some("resume"));
        for state in ["creating", "retired", "joining", "left", ""] {
            assert_eq!(availability_action(state), None);
        }
    }
}
