//! Disposable loopback integration fixture. Uses the real Agent API and PostgreSQL
//! with explicit test doubles for Pasion introspection and Matrix membership.
//! Never serves the fixture identity endpoints in the production backend.
use hagency_agent_service::{
    api::App, domain::DomainStore, gateway::Gateway, identity::Verifier, store::Store,
};
use salvo::prelude::*;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
#[derive(Clone)]
struct Members(Arc<RwLock<bool>>);
fn actor(token: &str) -> Option<&'static str> {
    match token {
        "fixture-oauth-alice" => Some("alice"),
        "fixture-oauth-bob" => Some("bob"),
        "fixture-oauth-outsider" => Some("outsider"),
        _ => None,
    }
}
#[handler]
async fn introspection(req: &mut Request, res: &mut Response) {
    if req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        != Some("Bearer fixture-introspection-secret")
    {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    }
    let token = req.form::<String>("token").await.unwrap_or_default();
    if let Some(name) = actor(&token) {
        res.render(Json(json!({"active":true,"sub":name,"username":name,"client_id":"fixture-native-client","scope":"urn:matrix:client:api:*","exp":hagency_agent_service::api::now_ms()/1000+3600})));
    } else {
        res.render(Json(json!({"active":false})));
    }
}
#[handler]
async fn whoami(req: &mut Request, res: &mut Response) {
    let name = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .and_then(actor);
    if let Some(name) = name {
        res.render(Json(
            json!({"user_id":format!("@{name}:example.test"),"is_guest":false}),
        ));
    } else {
        res.status_code(StatusCode::UNAUTHORIZED);
    }
}
#[handler]
async fn leave(depot: &Depot, res: &mut Response) {
    *depot.get_typed::<Members>().unwrap().0.write().unwrap() = false;
    res.render(Json(json!({"left":true})));
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL")?;
    let database: url::Url = url.parse()?;
    if database.host_str() != Some("127.0.0.1")
        || !database
            .path()
            .trim_start_matches('/')
            .starts_with("kanban_")
    {
        return Err("use a disposable loopback kanban_ database".into());
    }
    let port = std::env::var("HAGENCY_BOARD_FIXTURE_PORT")
        .unwrap_or_else(|_| "55474".into())
        .parse::<u16>()?;
    let origin = format!("http://127.0.0.1:{port}/");
    let public: url::Url = origin.parse()?;
    let issuer = public.join("auth/")?;
    let store = Store::open(&url, "example.test", issuer.as_str()).await?;
    let domain = DomainStore::open(&url, "example.test", "_hagency_fixture_").await?;
    let members = Members(Arc::new(RwLock::new(true)));
    let live = members.clone();
    let gateway = Gateway::new(
        Arc::new(move |space| {
            let live = live.clone();
            Box::pin(async move {
                if space != "!board:example.test" {
                    return Err(hagency_agent_service::Error::NotFound(
                        "fixture_space_not_found",
                    ));
                }
                let mut state = vec![
                    json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
                    json!({"type":"m.room.power_levels","state_key":"","content":{"users":{"@alice:example.test":100},"state_default":50}}),
                    json!({"type":"m.room.member","state_key":"@alice:example.test","content":{"membership":"join"}}),
                ];
                if *live.0.read().unwrap() {
                    state.push(json!({"type":"m.room.member","state_key":"@bob:example.test","content":{"membership":"join"}}));
                }
                Ok(state)
            })
        }),
        "@_hagency_service:example.test".into(),
    );
    let verifier = Verifier::new(
        issuer.clone(),
        public.join("auth/introspect")?,
        public.clone(),
        "fixture-introspection-secret".into(),
        "example.test".into(),
    )?;
    let app = App::new(
        store,
        verifier,
        public.clone(),
        issuer,
        "example.test".into(),
    )
    .with_domain(domain, gateway);
    let router = Router::new()
        .hoop(affix_state::inject(members))
        .push(app.router())
        .push(Router::with_path("auth/introspect").post(introspection))
        .push(Router::with_path("_matrix/client/v3/account/whoami").get(whoami))
        .push(Router::with_path("fixture/bob/leave").post(leave));
    let acceptor = TcpListener::new(format!("127.0.0.1:{port}")).bind().await;
    let server = tokio::spawn(async move {
        Server::new(acceptor).serve(router).await;
    });
    let client = reqwest::Client::new();
    let grant: Value = client
        .post(public.join("hagency/api/v1/sessions/pasion")?)
        .json(&json!({"accessToken":"fixture-oauth-alice"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let project: Value = client
        .post(public.join("hagency/api/v1/projects/adopt")?)
        .bearer_auth(grant["token"].as_str().ok_or("no session")?)
        .json(&json!({"spaceId":"!board:example.test"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let info = json!({"origin":origin,"projectId":project["project"]["id"],"identitySource":"fixture Pasion/Matrix; real native authorization and board PostgreSQL/API"});
    if let Ok(path) = std::env::var("HAGENCY_BOARD_FIXTURE_INFO") {
        std::fs::write(path, serde_json::to_vec_pretty(&info)?)?;
    }
    println!("{info}");
    server.await?;
    Ok(())
}
