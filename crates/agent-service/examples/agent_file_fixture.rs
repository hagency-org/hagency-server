//! Isolated cross-repository file qualification. Real Server API/PG/workers;
//! explicit loopback Pasion/Matrix doubles. Never mounted by the product backend.
#[path = "agent_file_fixture/wire.rs"]
mod wire;
use hagency_agent_service::{
    api::App,
    appservice::Inbox,
    domain::{AdminFacts, AgentVisibility, BindRoom, CreateAgent, DomainStore, RoomFacts},
    gateway::Gateway,
    identity::Verifier,
    matrix_client::MatrixClient,
    store::{RegisterDevice, Store},
    transport::{Limits, TransportStore},
};
use salvo::prelude::*;
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
const OWNER: &str = "@alice:test";
const ROOM: &str = "!joint-files:test";
const SPACE: &str = "!joint-space:test";
const HS: &str = "fixture-joint-hs-token-only-not-real-secret";
const AS: &str = "fixture-joint-as-token-only-not-real-secret";
const SDK: &str = "fixture-sdk-only";
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL")?;
    let parsed: url::Url = database.parse()?;
    if parsed.host_str() != Some("127.0.0.1") || !parsed.path().starts_with("/agent_files_joint_") {
        return Err("use an isolated loopback agent_files_joint_ database".into());
    }
    let port: u16 = std::env::var("HAGENCY_FILE_FIXTURE_PORT")?.parse()?;
    let origin = format!("http://127.0.0.1:{port}/");
    let public: url::Url = origin.parse()?;
    let issuer = public.join("auth/")?;
    let store = Store::open(&database, "test", issuer.as_str()).await?;
    let domain = DomainStore::open(&database, "test", "_hagency_joint_").await?;
    let transport = TransportStore::open(&database, Limits::default()).await?;
    let inbox = Inbox::open(&database, HS.into()).await?;
    let fixture = wire::Fixture {
        origin: origin.clone(),
        puppet: Arc::new(Mutex::new(String::new())),
        state: Arc::new(Mutex::new(wire::State::default())),
    };
    let gateway_fixture = fixture.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let f = gateway_fixture.clone();
            Box::pin(async move { f.room_state(&room) })
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
    let app = App::new(
        store.clone(),
        verifier.clone(),
        public,
        issuer,
        "test".into(),
    )
    .with_domain(domain.clone(), gateway)
    .with_transport(transport)
    .with_matrix_client(MatrixClient::new(
        origin.parse()?,
        AS.into(),
        "test".into(),
    )?);
    let router = Router::new()
        .hoop(affix_state::inject(fixture.clone()))
        .hoop(wire::lose_queue_ack)
        .push(app.readiness_router())
        .push(app.router())
        .push(inbox.router())
        .push(wire::router());
    let acceptor = TcpListener::new(format!("127.0.0.1:{port}"))
        .try_bind()
        .await?;
    let server = tokio::spawn(async move {
        Server::new(acceptor).serve(router).await;
    });
    let now = hagency_agent_service::api::now_ms();
    // Use real HTTP introspection/whoami verification; Identity is intentionally
    // not constructible outside the production verifier's trust boundary.
    let grant = store.sign_in(verifier.verify(SDK, now).await?, now).await?;
    let device = store
        .register_device(
            &grant.token,
            RegisterDevice {
                installation_id: "joint-bootstrap".into(),
                name: "Fixture bootstrap".into(),
            },
            now,
        )
        .await?;
    let p = store.authenticate(&device.token, now, true).await?;
    let sf = AdminFacts {
        actor_mxid: OWNER.into(),
        room_id: SPACE.into(),
        observed_at_ms: now,
        joined: true,
        can_manage_policy: true,
        is_space: true,
        linked_space_id: None,
    };
    let rf = AdminFacts {
        room_id: ROOM.into(),
        is_space: false,
        linked_space_id: Some(SPACE.into()),
        ..sf.clone()
    };
    let project = domain.register_project(&p, SPACE, &sf, now).await?;
    domain
        .register_room(&p, &project.id, ROOM, &sf, &rf, now)
        .await?;
    let agent = domain
        .create_agent(
            &p,
            CreateAgent {
                display_name: "Joint files fixture".into(),
                idempotency_key: "joint-fixture-agent".into(),
                visibility: AgentVisibility::Public,
            },
            now,
        )
        .await?;
    *fixture.puppet.lock().unwrap() = agent.puppet_mxid.clone();
    // Provisioning and Matrix membership are explicit fixture facts. Keep the
    // actual current allocation/binding lifecycle and its generation fences.
    if agent.state == "creating" {
        domain
            .confirm_identity_provisioned(&agent.id, agent.generation)
            .await?;
    }
    let facts = RoomFacts {
        owner_direct_valid: false,
        owner_mxid: OWNER.into(),
        room_id: ROOM.into(),
        space_id: SPACE.into(),
        observed_at_ms: now,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: Some(agent.puppet_mxid.clone()),
        puppet_in_room: true,
        encrypted: false,
    };
    let created = domain
        .bind_room(
            &p,
            &agent.id,
            BindRoom {
                project_id: project.id,
                room_id: ROOM.into(),
                idempotency_key: "joint-fixture-binding".into(),
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
    let worker = app
        .start_delivery(inbox.clone())
        .ok_or("no delivery worker")?;
    let readiness = app
        .start_matrix_readiness(inbox.clone())
        .ok_or("no readiness worker")?;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !app.startup_ready() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    let info = json!({"origin":origin,"owner":OWNER,"room":ROOM,"requester":"@bob:test","thread":"$joint-root:test","event":"$joint-incoming:test","agent":domain.agent(&p,&agent.id,hagency_agent_service::api::now_ms()).await?,"binding":binding,"fixture":"explicit Pasion/Matrix doubles; actual current Server/PG/API/AS/worker"});
    std::fs::write(
        std::env::var("HAGENCY_FILE_FIXTURE_INFO")?,
        serde_json::to_vec_pretty(&info)?,
    )?;
    println!("Joint file fixture ready (loopback, isolated PostgreSQL)");
    tokio::signal::ctrl_c().await?;
    worker.abort();
    readiness.abort();
    server.abort();
    Ok(())
}
