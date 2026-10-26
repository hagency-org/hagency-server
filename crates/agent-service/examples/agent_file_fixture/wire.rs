use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct State {
    uploads: BTreeMap<String, Vec<u8>>,
    messages: Vec<Value>,
    lose_queue: bool,
    lost_acks: u64,
}
#[derive(Clone)]
pub struct Fixture {
    pub origin: String,
    pub puppet: Arc<Mutex<String>>,
    pub state: Arc<Mutex<State>>,
}
pub fn incoming() -> Vec<u8> {
    (0..131_073).map(|n| n as u8).collect()
}
impl Fixture {
    pub fn room_state(&self, room: &str) -> hagency_agent_service::Result<Vec<Value>> {
        if ![ROOM, SPACE].contains(&room) {
            return Err(hagency_agent_service::Error::NotFound(
                "fixture_room_not_found",
            ));
        }
        let mut state = vec![
            json!({"type":"m.room.create","state_key":"","content":if room==SPACE {json!({"type":"m.space"})}else{json!({})}}),
            json!({"type":"m.room.power_levels","state_key":"","content":{"users":{OWNER:100},"events_default":0,"state_default":50}}),
        ];
        let puppet = self.puppet.lock().unwrap().clone();
        for member in [OWNER, "@bob:test", &puppet, "@_hagency_service:test"] {
            state.push(
                json!({"type":"m.room.member","state_key":member,"content":{"membership":"join"}}),
            );
        }
        if room == SPACE {
            state.push(json!({"type":"m.space.child","state_key":ROOM,"content":{"via":["test"]}}));
        }
        Ok(state)
    }
}
fn token(req: &Request) -> Option<&str> {
    req.headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
}
#[handler]
async fn introspection(req: &mut Request, res: &mut Response) {
    if token(req) != Some("fixture-introspection-secret") {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    }
    let valid = req.form::<String>("token").await.as_deref() == Some(SDK);
    res.render(Json(if valid{json!({"active":true,"sub":"alice","username":"alice","client_id":"sdk-client","scope":"urn:matrix:client:api:*","exp":hagency_agent_service::api::now_ms()/1000+3600})}else{json!({"active":false})}));
}
#[handler]
async fn whoami(req: &mut Request, res: &mut Response) {
    let user = if token(req) == Some(SDK) {
        OWNER.to_owned()
    } else if token(req) == Some(AS) {
        req.query::<String>("user_id").unwrap_or_default()
    } else {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    };
    res.render(Json(
        json!({"user_id":user,"device_id":"JOINT-FIXTURE","is_guest":false}),
    ));
}
#[handler]
async fn create_room(req: &mut Request, res: &mut Response) {
    if token(req) != Some(AS) {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    }
    res.render(Json(json!({"room_id":"!joint-readiness:test"})));
}
#[handler]
async fn upload(req: &mut Request, depot: &Depot, res: &mut Response) {
    let f = depot.get_typed::<Fixture>().unwrap();
    let puppet = f.puppet.lock().unwrap().clone();
    if token(req) != Some(AS) || req.query::<String>("user_id").as_deref() != Some(puppet.as_str())
    {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    }
    req.set_secure_max_size(hagency_agent_service::file_delivery::MAX_BYTES);
    let bytes = match req.payload().await {
        Ok(b) => b.to_vec(),
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            return;
        }
    };
    let mut state = f.state.lock().unwrap();
    let name = format!("outgoing-{}", state.uploads.len());
    state.uploads.insert(name.clone(), bytes);
    res.render(Json(json!({"content_uri":format!("mxc://test/{name}")})));
}
#[handler]
async fn download(req: &mut Request, depot: &Depot, res: &mut Response) {
    if token(req) != Some(SDK) {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    }
    let id = req.param::<String>("media").unwrap_or_default();
    let body = if id == "joint-incoming" {
        Some(incoming())
    } else {
        depot
            .get_typed::<Fixture>()
            .unwrap()
            .state
            .lock()
            .unwrap()
            .uploads
            .get(&id)
            .cloned()
    };
    if let Some(body) = body {
        res.headers_mut().insert(
            "content-type",
            salvo::http::HeaderValue::from_static("application/octet-stream"),
        );
        res.write_body(body).unwrap();
    } else {
        res.status_code(StatusCode::NOT_FOUND);
    }
}
#[handler]
async fn send(req: &mut Request, depot: &Depot, res: &mut Response) {
    let f = depot.get_typed::<Fixture>().unwrap();
    let user = req.query::<String>("user_id").unwrap_or_default();
    let puppet = f.puppet.lock().unwrap().clone();
    if token(req) != Some(AS)
        || ![puppet.as_str(), "@_hagency_service:test"].contains(&user.as_str())
    {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    }
    let content: Value = match req.parse_json().await {
        Ok(v) => v,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            return;
        }
    };
    let room = req.param::<String>("room").unwrap_or_default();
    let room = percent_encoding::percent_decode_str(&room)
        .decode_utf8_lossy()
        .into_owned();
    let transaction = req.param::<String>("transaction").unwrap_or_default();
    let kind = req.param::<String>("kind").unwrap_or_default();
    let id = {
        let mut state = f.state.lock().unwrap();
        let id = format!("$joint-receipt-{}", state.messages.len());
        state.messages.push(json!({"event_id":id,"room_id":room,"sender":user,"type":kind,"content":content,"transaction":transaction}));
        id
    };
    let event = json!({"event_id":id,"room_id":room,"sender":user,"type":kind,"content":content});
    let response = reqwest::Client::new()
        .put(format!(
            "{}_matrix/app/v1/transactions/joint_{transaction}",
            f.origin
        ))
        .bearer_auth(HS)
        .json(&json!({"events":[event]}))
        .send()
        .await;
    if !response.is_ok_and(|r| r.status().is_success()) {
        res.status_code(StatusCode::BAD_GATEWAY);
        return;
    }
    res.render(Json(json!({"event_id":id})));
}
#[handler]
pub async fn lose_queue_ack(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let queue = req.uri().path() == "/hagency/api/v1/execution/files/queue";
    ctrl.call_next(req, depot, res).await;
    if queue && res.status_code.is_none_or(|s| s.is_success()) {
        let mut state = depot.get_typed::<Fixture>().unwrap().state.lock().unwrap();
        if state.lose_queue {
            state.lose_queue = false;
            state.lost_acks += 1;
            res.status_code(StatusCode::SERVICE_UNAVAILABLE);
            res.render(Json(json!({"error":"fixture_lost_original_queue_ack"})));
        }
    }
}
#[handler]
async fn inbound(req: &mut Request, depot: &Depot, res: &mut Response) {
    let f = depot.get_typed::<Fixture>().unwrap();
    let input: Value = req.parse_json().await.unwrap_or_default();
    f.state.lock().unwrap().lose_queue = input["loseQueueAck"] == true;
    let puppet = f.puppet.lock().unwrap().clone();
    let event = json!({"type":"m.room.message","event_id":"$joint-incoming:test","room_id":ROOM,"sender":"@bob:test","content":{"msgtype":"m.file","body":format!("{}\nUse receive_file on this original attachment. Read the original workspace bytes, append the exact bytes \\nfixture output\\n to output.bin, call send_file, query only its original delivery_id until sent and report JOINT_FILES_COMPLETE. Do not send a replacement.",puppet),"filename":"incoming.bin","url":"mxc://test/joint-incoming","info":{"size":incoming().len()},"m.mentions":{"user_ids":[puppet]},"m.relates_to":{"rel_type":"m.thread","event_id":"$joint-root:test","is_falling_back":false}}});
    let result = reqwest::Client::new()
        .put(format!(
            "{}_matrix/app/v1/transactions/joint_incoming",
            f.origin
        ))
        .bearer_auth(HS)
        .json(&json!({"events":[event]}))
        .send()
        .await;
    if result.is_ok_and(|r| r.status().is_success()) {
        res.render(Json(json!({"accepted":true})));
    } else {
        res.status_code(StatusCode::BAD_GATEWAY);
    }
}
#[handler]
async fn snapshot(depot: &Depot, res: &mut Response) {
    let state = depot.get_typed::<Fixture>().unwrap().state.lock().unwrap();
    res.render(Json(json!({"uploads":state.uploads.len(),"messages":state.messages,"lostQueueAcks":state.lost_acks})));
}
pub fn router() -> Router {
    Router::new()
        .push(Router::with_path("auth/introspect").post(introspection))
        .push(Router::with_path("_matrix/client/v3/account/whoami").get(whoami))
        .push(Router::with_path("_matrix/client/v3/createRoom").post(create_room))
        .push(Router::with_path("_matrix/media/v3/upload").post(upload))
        .push(Router::with_path("_matrix/client/v1/media/download/test/{media}").get(download))
        .push(
            Router::with_path("_matrix/client/v3/rooms/{room}/send/{kind}/{transaction}").put(send),
        )
        .push(Router::with_path("fixture/inbound").post(inbound))
        .push(Router::with_path("fixture/snapshot").get(snapshot))
}
