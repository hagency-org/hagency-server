use super::common::*;
use crate::components::ui::icons::Icon;
use dioxus::prelude::*;
use serde_json::{Value, json};

#[component]
pub fn Agents() -> Element {
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
            button {class:"hg-button hg-secondary", disabled:busy(), onclick:move |_| {agents.restart();devices.restart();bindings.restart();},"Refresh"}
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
                    div {class:"hg-heading",h2 {{text(&agent,"displayName")}}span {class:"hg-badge",{text(&agent,"state")}}}
                    p {class:"hg-note",{match device {Some(d)=>format!("Execution device: {}{}",text(&d,"name"),if d["revoked"]==true {" (authorization revoked)"}else{""}),None if device_id.is_empty()=>"No execution device assigned".into(),None=>format!("Execution device: {device_id}")}}}
                    details {summary {"Matrix identity"}code {{text(&agent,"puppetMxid")}}}
                    div {class:"hg-actions",button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| {selected.set(id.clone());project.set(String::new());room.set(String::new());message.set(String::new());command.set(browser_auth::operation_id());},"Manage Agent"}}
                }}}
            }
        }
        if let Some(agent)=active_agent {
            section {class:"hg-card hg-stack",h2 {{text(&agent,"displayName")}}
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
                        h3 {{binding_title(is_direct)}}
                        details {summary {"Room ID"}code {{text(&binding,"roomId")}}}
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
                details {summary {"Add to a Project Room"}
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
                div {class:"hg-actions",button {class:"hg-button hg-secondary",disabled:busy(),onclick:move |_| selected.set(String::new()),"Done"}}
            }
        }
    }}
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
fn binding_title(is_direct: bool) -> &'static str {
    if is_direct {
        "Private chat with you"
    } else {
        "Project Room"
    }
}
fn binding_body(project: &str, room: &str, key: &str) -> Value {
    json!({"projectId":project.trim(),"roomId":room.trim(),"idempotencyKey":key})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_binding_request_contains_only_room_scope_and_original_key() {
        assert_eq!(
            binding_body(" prj_owned ", " !discussion:server ", "bind-key"),
            json!({"projectId":"prj_owned","roomId":"!discussion:server","idempotencyKey":"bind-key"})
        );
    }
    #[test]
    fn binding_card_title_is_readable_and_not_a_room_id() {
        assert_eq!(binding_title(true), "Private chat with you");
        assert_eq!(binding_title(false), "Project Room");
        assert!(!binding_title(true).starts_with('!'));
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
