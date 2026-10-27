//! Actual loopback Synapse qualification. Only OAuth introspection is a double.
//! Never mounted by the product backend; no production account is accepted.
use hagency_agent_service::{
    api::App,
    appservice::Inbox,
    domain::{AgentVisibility, BindRoom, CreateAgent, DomainStore},
    gateway::Gateway,
    identity::Verifier,
    matrix_client::MatrixClient,
    store::{RegisterDevice, Store},
    transport::{Limits, TransportStore},
};
use salvo::prelude::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
const AS: &str = "fixture-joint-as-token-only-not-real-secret";
const HS: &str = "fixture-joint-hs-token-only-not-real-secret";
const OWNER: &str = "@alice:test";
const REQUESTER: &str = "@bob:test";
#[derive(Clone)]
struct Context {
    matrix: url::Url,
    owner: String,
    requester: String,
    room: String,
    puppet: String,
    thread: String,
    inbound: Arc<AtomicBool>,
    uploads: Arc<AtomicU64>,
}
async fn call(
    matrix: &url::Url,
    method: reqwest::Method,
    segments: &[&str],
    token: &str,
    user: Option<&str>,
    body: Option<Value>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut url = matrix.clone();
    url.path_segments_mut()
        .map_err(|_| "matrix origin")?
        .clear()
        .extend(segments);
    if let Some(user) = user {
        url.query_pairs_mut().append_pair("user_id", user);
    }
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(8))
        .build()?;
    let mut request = http.request(method, url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        return Err(format!("isolated Matrix response {status}: {}", value["errcode"]).into());
    }
    Ok(value)
}
#[handler]
async fn introspect(req: &mut Request, depot: &Depot, res: &mut Response) {
    let c = depot.get_typed::<Context>().unwrap();
    let secret = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    // Multiple real owner SDK devices have different tokens. This explicit
    // OAuth bridge checks the actual local homeserver identity for each token;
    // it never treats a token-shaped string or another test user as the owner.
    let authorized = secret == Some("Bearer fixture-introspection-secret");
    let token = req.form::<String>("token").await;
    let valid = if authorized {
        if let Some(token) = token {
            call(
                &c.matrix,
                reqwest::Method::GET,
                &["_matrix", "client", "v3", "account", "whoami"],
                &token,
                None,
                None,
            )
            .await
            .is_ok_and(|v| {
                v["user_id"] == OWNER && v["device_id"].as_str().is_some_and(|d| !d.is_empty())
            })
        } else {
            false
        }
    } else {
        false
    };
    res.render(Json(if valid {json!({"active":true,"sub":"alice","username":"alice","client_id":"sdk-client","scope":"urn:matrix:client:api:*","exp":hagency_agent_service::api::now_ms()/1000+3600})}else{json!({"active":false})}));
}
#[handler]
async fn proxy(req: &mut Request, depot: &Depot, res: &mut Response) {
    let c = depot.get_typed::<Context>().unwrap();
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let target = match c.matrix.join(path) {
        Ok(u) => u,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            return;
        }
    };
    if req.method() == reqwest::Method::POST && req.uri().path() == "/_matrix/media/v3/upload" {
        c.uploads.fetch_add(1, Ordering::SeqCst);
    }
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let mut request = http.request(req.method().clone(), target);
    for name in ["authorization", "content-type"] {
        if let Some(value) = req.headers().get(name) {
            request = request.header(name, value);
        }
    }
    req.set_secure_max_size(16 * 1024 * 1024);
    let body = match req.payload().await {
        Ok(b) => b.clone(),
        Err(_) => {
            res.status_code(StatusCode::PAYLOAD_TOO_LARGE);
            return;
        }
    };
    let result = async {
        let mut response = request.body(body).send().await?;
        let status = response.status();
        let content_type = response.headers().get("content-type").cloned();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err("bounded proxy response".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok::<_, Box<dyn std::error::Error>>((status, content_type, bytes))
    }
    .await;
    match result {
        Ok((status, content_type, bytes)) => {
            res.status_code(status);
            if let Some(h) = content_type {
                res.headers_mut().insert("content-type", h);
            }
            let _ = res.write_body(bytes);
        }
        Err(_) => {
            res.status_code(StatusCode::BAD_GATEWAY);
        }
    }
}
#[handler]
async fn inbound(depot: &Depot, res: &mut Response) {
    let c = depot.get_typed::<Context>().unwrap();
    if c.inbound.swap(true, Ordering::SeqCst) {
        res.status_code(StatusCode::CONFLICT);
        return;
    }
    let result=async {
        let bytes:Vec<u8>=(0..131_073).map(|n|n as u8).collect();
        let upload:Value=reqwest::Client::new().post(c.matrix.join("_matrix/media/v3/upload")?).bearer_auth(&c.requester).header("content-type","application/octet-stream").body(bytes).send().await?.error_for_status()?.json().await?;
        let content=json!({"msgtype":"m.file","body":format!("{} Use receive_file on this original attachment. Read the original workspace bytes, append the exact bytes \\nfixture output\\n to output.bin, call send_file, query only its original delivery_id until sent and report JOINT_FILES_COMPLETE. Do not send a replacement.",c.puppet),"filename":"incoming.bin","url":upload["content_uri"],"info":{"size":131073},"m.mentions":{"user_ids":[c.puppet]},"m.relates_to":{"rel_type":"m.thread","event_id":c.thread,"is_falling_back":false}});
        call(&c.matrix,reqwest::Method::PUT,&["_matrix","client","v3","rooms",&c.room,"send","m.room.message","joint-incoming"],&c.requester,None,Some(content)).await
    }.await;
    match result {
        Ok(value) => res.render(Json(value)),
        Err(_) => {
            res.status_code(StatusCode::BAD_GATEWAY);
        }
    }
}
#[handler]
async fn snapshot(depot: &Depot, res: &mut Response) {
    let c = depot.get_typed::<Context>().unwrap();
    let mut url = c.matrix.clone();
    url.path_segments_mut()
        .unwrap()
        .clear()
        .extend(["_matrix", "client", "v3", "rooms", &c.room, "messages"]);
    url.query_pairs_mut()
        .append_pair("dir", "b")
        .append_pair("limit", "100");
    let result = async {
        reqwest::Client::new()
            .get(url)
            .bearer_auth(&c.owner)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await
    };
    match result.await {Ok(value)=>res.render(Json(json!({"uploads":c.uploads.load(Ordering::SeqCst),"messages":value["chunk"],"realHomeserver":true}))),Err(_)=>{res.status_code(StatusCode::BAD_GATEWAY);}}
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL")?;
    let db: url::Url = database.parse()?;
    if db.host_str() != Some("127.0.0.1") || !db.path().starts_with("/agent_files_joint_") {
        return Err("dedicated loopback database required".into());
    }
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(std::env::var("HAGENCY_SYNAPSE_BOOTSTRAP")?)?)?;
    if bootstrap["owner"]["user_id"] != OWNER || bootstrap["requester"]["user_id"] != REQUESTER {
        return Err("isolated test accounts required".into());
    }
    let matrix: url::Url = bootstrap["matrixOrigin"]
        .as_str()
        .ok_or("matrix origin")?
        .parse()?;
    if matrix.scheme() != "http" || matrix.host_str() != Some("127.0.0.1") {
        return Err("loopback Synapse required".into());
    }
    let owner = bootstrap["owner"]["access_token"]
        .as_str()
        .ok_or("owner token")?
        .to_owned();
    let requester = bootstrap["requester"]["access_token"]
        .as_str()
        .ok_or("requester token")?
        .to_owned();
    let port: u16 = std::env::var("HAGENCY_FILE_FIXTURE_PORT")?.parse()?;
    let public: url::Url = format!("http://127.0.0.1:{port}/").parse()?;
    let issuer = public.join("auth/")?;
    let store = Store::open(&database, "test", issuer.as_str()).await?;
    let domain = DomainStore::open(&database, "test", "_hagency_joint_").await?;
    let transport = TransportStore::open(&database, Limits::default()).await?;
    let known = domain.clone();
    let inbox = Inbox::open(&database, HS.into())
        .await?
        .with_known_user(Arc::new(move |user| {
            let d = known.clone();
            Box::pin(async move {
                if user == "@_hagency_service:test" {
                    Ok(true)
                } else {
                    d.known_puppet(&user).await
                }
            })
        }));
    let state_matrix = matrix.clone();
    let state_owner = owner.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let m = state_matrix.clone();
            let token = state_owner.clone();
            Box::pin(async move {
                let state = call(
                    &m,
                    reqwest::Method::GET,
                    &["_matrix", "client", "v3", "rooms", &room, "state"],
                    &token,
                    None,
                    None,
                )
                .await
                .map_err(|_| {
                    hagency_agent_service::Error::Unavailable("matrix_state_unavailable")
                })?;
                state
                    .as_array()
                    .cloned()
                    .ok_or(hagency_agent_service::Error::Unavailable(
                        "invalid_matrix_state",
                    ))
            })
        }),
        "@_hagency_service:test".into(),
    );
    let verifier = Verifier::new(
        issuer.clone(),
        public.join("auth/introspect")?,
        public.clone(),
        "fixture-introspection-secret".into(),
        "test".into(),
    )?;
    let matrix_client = MatrixClient::new(public.clone(), AS.into(), "test".into())?;
    let app = App::new(
        store.clone(),
        verifier.clone(),
        public.clone(),
        issuer,
        "test".into(),
    )
    .with_domain(domain.clone(), gateway.clone())
    .with_transport(transport)
    .with_matrix_client(matrix_client.clone());
    // Create real Matrix state before exposing the fixture. No fabricated Gateway facts.
    let mut rooms = Vec::new();
    for space in [true, false] {
        let created=call(&matrix,reqwest::Method::POST,&["_matrix","client","v3","createRoom"],&owner,None,Some(json!({"preset":"private_chat","name":"Hagency isolated file qualification","creation_content":if space{json!({"type":"m.space","m.federate":false})}else{json!({"m.federate":false})},"invite":[REQUESTER,"@_hagency_service:test"]}))).await?;
        let room = created["room_id"].as_str().ok_or("room")?.to_owned();
        for (token, user) in [
            (requester.as_str(), None),
            (AS, Some("@_hagency_service:test")),
        ] {
            call(
                &matrix,
                reqwest::Method::POST,
                &["_matrix", "client", "v3", "join", &room],
                token,
                user,
                Some(json!({})),
            )
            .await?;
        }
        rooms.push(room);
    }
    let space = rooms[0].clone();
    let room = rooms[1].clone();
    call(
        &matrix,
        reqwest::Method::PUT,
        &[
            "_matrix",
            "client",
            "v3",
            "rooms",
            &space,
            "state",
            "m.space.child",
            &room,
        ],
        &owner,
        None,
        Some(json!({"via":["test"]})),
    )
    .await?;
    let root = call(
        &matrix,
        reqwest::Method::PUT,
        &[
            "_matrix",
            "client",
            "v3",
            "rooms",
            &room,
            "send",
            "m.room.message",
            "joint-root",
        ],
        &requester,
        None,
        Some(json!({"msgtype":"m.text","body":"Isolated file qualification root"})),
    )
    .await?;
    let context = Context {
        matrix,
        owner: owner.clone(),
        requester,
        room: room.clone(),
        puppet: String::new(),
        thread: root["event_id"].as_str().ok_or("root event")?.into(),
        inbound: Arc::new(AtomicBool::new(false)),
        uploads: Arc::new(AtomicU64::new(0)),
    };
    // Mutable setup context is replaced before fixture actions can be reached.
    let setup = Router::new()
        .hoop(affix_state::inject(context.clone()))
        .push(app.readiness_router())
        .push(app.router())
        .push(inbox.router())
        .push(Router::with_path("auth/introspect").post(introspect))
        .push(Router::with_path("_matrix/{**path}").goal(proxy));
    let acceptor = TcpListener::new(format!("127.0.0.1:{port}"))
        .try_bind()
        .await?;
    let server = tokio::spawn(async move {
        Server::new(acceptor).serve(setup).await;
    });
    let now = hagency_agent_service::api::now_ms();
    let grant = store
        .sign_in(verifier.verify(&owner, now).await?, now)
        .await?;
    let device = store
        .register_device(
            &grant.token,
            RegisterDevice {
                installation_id: "synapse-bootstrap".into(),
                name: "Synapse qualification".into(),
            },
            now,
        )
        .await?;
    let p = store.authenticate(&device.token, now, true).await?;
    let sf = gateway.admin(&space, OWNER, None).await?;
    let rf = gateway.admin(&room, OWNER, Some(&space)).await?;
    let project = domain.register_project(&p, &space, &sf, now).await?;
    domain
        .register_room(&p, &project.id, &room, &sf, &rf, now)
        .await?;
    let agent = domain
        .create_agent(
            &p,
            CreateAgent {
                display_name: "Synapse file qualification".into(),
                idempotency_key: "synapse-file-agent".into(),
                visibility: AgentVisibility::Public,
            },
            now,
        )
        .await?;
    call(&context.matrix,reqwest::Method::POST,&["_matrix","client","v3","register"],AS,None,Some(json!({"type":"m.login.application_service","username":agent.puppet_mxid.split(':').next().unwrap().trim_start_matches('@'),"inhibit_login":true}))).await?;
    matrix_client.provision(&agent, &room, true).await?;
    domain
        .confirm_identity_provisioned(&agent.id, agent.generation)
        .await?;
    let facts = gateway
        .room(&room, &space, OWNER, Some(&agent.puppet_mxid))
        .await?;
    let created = domain
        .bind_room(
            &p,
            &agent.id,
            BindRoom {
                project_id: project.id,
                room_id: room.clone(),
                idempotency_key: "synapse-file-binding".into(),
            },
            &facts,
            now,
        )
        .await?;
    let binding = domain
        .activate_binding(
            &p,
            &created.binding.id,
            created.binding.generation,
            &facts,
            now,
        )
        .await?;
    // Swap the listening router only after setup is fully stopped and joined.
    server.abort();
    let _ = server.await;
    let mut context = context;
    context.puppet = agent.puppet_mxid.clone();
    let router = Router::new()
        .hoop(affix_state::inject(context.clone()))
        .push(app.readiness_router())
        .push(app.router())
        .push(inbox.router())
        .push(Router::with_path("auth/introspect").post(introspect))
        .push(Router::with_path("fixture/inbound").post(inbound))
        .push(Router::with_path("fixture/snapshot").get(snapshot))
        .push(Router::with_path("_matrix/{**path}").goal(proxy));
    let acceptor = TcpListener::new(format!("127.0.0.1:{port}"))
        .try_bind()
        .await?;
    let server = tokio::spawn(async move {
        Server::new(acceptor).serve(router).await;
    });
    let worker = app.start_delivery(inbox.clone()).ok_or("delivery worker")?;
    let readiness = app
        .start_matrix_readiness(inbox.clone())
        .ok_or("readiness worker")?;
    tokio::time::timeout(Duration::from_secs(20), async {
        while !app.startup_ready() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    let info = json!({"origin":public,"owner":OWNER,"room":room,"requester":REQUESTER,"thread":context.thread,"agent":domain.agent(&p,&agent.id,hagency_agent_service::api::now_ms()).await?,"binding":binding,"realHomeserver":true,"fixture":"actual Synapse/API/PG/AS/workers; explicit OAuth introspection bridge"});
    std::fs::write(
        std::env::var("HAGENCY_FILE_FIXTURE_INFO")?,
        serde_json::to_vec_pretty(&info)?,
    )?;
    println!("Actual Synapse file qualification ready; loopback only");
    tokio::signal::ctrl_c().await?;
    worker.abort();
    readiness.abort();
    server.abort();
    Ok(())
}
