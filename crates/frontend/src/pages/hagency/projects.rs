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
        div {class:"hg-heading", h1 {"Projects"}
            button {class:"hg-button hg-secondary", onclick:move |_| projects.restart(), "Refresh"}
        }
        p {class:"hg-note", "Manage registered Spaces, Room access and service restrictions. Create Projects and chat in Hagency Desktop."}
        Notice {message}
        if projects().is_none() {p {role:"status", "Loading Projects…"}}
        if let Some(Err(error))=projects() {p {role:"alert", "{error}"}}
        if projects().is_some_and(|r| r.is_ok()) && rows(&data,"projects").is_empty() {
            p {"No registered Projects you currently belong to."}
        }
        if !rows(&data,"projects").is_empty() {
            label {class:"hg-field", "Project"
                select {class:"hg-input", value:project_id(), disabled:busy(), onchange:move |e| {project_id.set(e.value());room.set(String::new());message.set(String::new());},
                    option {value:"", "Select a Project"}
                    for item in rows(&data,"projects") {option {value:text(&item,"id"), {name(&item,"spaceId")}}}
                }
            }
        }
        if let Some(item)=selected {
            section {class:"hg-card hg-stack", h2 {{name(&item,"spaceId")}}
                if !text(&item,"topic").is_empty() {p {{text(&item,"topic")}}}
                p {class:"hg-note", "Rooms have independent membership. Joining the Project does not grant access to private Rooms."}
                details {summary {"Identifiers"} dl {class:"hg-facts", dt {"Project"} dd {{text(&item,"id")}} dt {"Space"} dd {{text(&item,"spaceId")}}}}
            }
            super::room_discovery::RoomDiscovery {project_id, room}
            {rsx! {super::project_controls::ProjectControls {key:"{project_id()}:project", project_id, room_id:String::new()}}}
            if !room().is_empty() {
                super::project_controls::ProjectControls {key:"{project_id()}:{room()}", project_id, room_id:room()}
            }
            details {class:"hg-card", summary {"Register an existing Room"}
                form {class:"hg-stack", onsubmit:move |e| {
                    e.prevent_default();if busy(){return;}let id=project_id();let room_id=new_room();busy.set(true);
                    spawn(async move {match mutate(&format!("/projects/{}/rooms/adopt",urlencoding::encode(&id)),"POST",json!({"roomId":room_id.trim()})).await {
                        Ok(_)=>{message.set("Room registered. Refresh Rooms to view it.".into());new_room.set(String::new());},Err(e)=>message.set(e)
                    }busy.set(false);});
                },
                    p {class:"hg-note", "Requires administration rights in this Room. Link it to the selected Space first."}
                    label {class:"hg-field", "Room ID" input {class:"hg-input",required:true,value:new_room(),placeholder:"!room:server",oninput:move |e| new_room.set(e.value())}}
                    div {class:"hg-actions", button {class:"hg-button",disabled:busy(),r#type:"submit","Register Room"}}
                }
            }
        }
        details {class:"hg-card", summary {"Register an existing Space"}
            form {class:"hg-stack", onsubmit:move |e| {
                e.prevent_default();if busy(){return;}let id=space();busy.set(true);
                spawn(async move {match mutate("/projects/adopt","POST",json!({"spaceId":id.trim()})).await {
                    Ok(_)=>{message.set("Project registered. Members may connect Agents by default.".into());space.set(String::new());projects.restart();},Err(e)=>message.set(e)
                }busy.set(false);});
            },
                p {class:"hg-note", "Requires administration rights in the Space. To create a new Project, use Hagency Desktop."}
                label {class:"hg-field", "Space ID" input {class:"hg-input",required:true,value:space(),placeholder:"!space:server",oninput:move |e| space.set(e.value())}}
                div {class:"hg-actions",button {class:"hg-button",disabled:busy(),r#type:"submit","Register Project"}}
            }
        }
    }}
}
