use crate::{
    Error, Result,
    identity::Verifier,
    store::{RegisterDevice, Store},
};
use salvo::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use url::Url;

#[derive(Clone)]
pub struct App {
    pub store: Store,
    verifier: Verifier,
    public: Url,
    issuer: Url,
    server_name: String,
    startup_ready: Arc<AtomicBool>,
    matrix_client: Option<crate::matrix_client::MatrixClient>,
    domain: Option<(crate::domain::DomainStore, crate::gateway::Gateway)>,
    transport: Option<crate::transport::TransportStore>,
}
impl App {
    pub fn new(
        store: Store,
        verifier: Verifier,
        public: Url,
        issuer: Url,
        server_name: String,
    ) -> Self {
        Self {
            store,
            verifier,
            public,
            issuer,
            server_name,
            startup_ready: Arc::new(AtomicBool::new(true)),
            matrix_client: None,
            domain: None,
            transport: None,
        }
    }
    pub fn with_matrix_client(mut self, matrix: crate::matrix_client::MatrixClient) -> Self {
        self.startup_ready.store(false, Ordering::Release);
        self.matrix_client = Some(matrix);
        self
    }
    pub fn with_domain(
        mut self,
        domain: crate::domain::DomainStore,
        gateway: crate::gateway::Gateway,
    ) -> Self {
        self.domain = Some((domain, gateway));
        self
    }
    pub fn with_transport(mut self, transport: crate::transport::TransportStore) -> Self {
        self.transport = Some(transport);
        self
    }
    pub fn start_delivery(
        &self,
        inbox: crate::appservice::Inbox,
    ) -> Option<tokio::task::JoinHandle<()>> {
        Some(crate::workers::start(
            inbox,
            self.transport.clone()?,
            self.domain.as_ref()?.1.clone(),
            self.matrix_client.clone()?,
        ))
    }
    pub fn start_cleanup(&self) -> Option<tokio::task::JoinHandle<()>> {
        let (domain, gateway) = self.domain.as_ref()?;
        Some(crate::workers::start_cleanup(
            domain.clone(),
            gateway.clone(),
            self.matrix_client.clone()?,
        ))
    }
    pub fn router(&self) -> Router {
        Router::with_path("api/hagency/v1/{**path}")
            .hoop(salvo::size_limiter::max_size(131072))
            .goal(self.clone())
    }
    pub fn readiness_router(&self) -> Router {
        Router::with_path("readyz").get(StartupReadiness(self.startup_ready.clone()))
    }
    pub fn startup_ready(&self) -> bool {
        self.startup_ready.load(Ordering::Acquire)
    }
    pub fn start_matrix_readiness(
        &self,
        inbox: crate::appservice::Inbox,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let matrix = self.matrix_client.clone()?;
        let ready = self.startup_ready.clone();
        Some(tokio::spawn(async move {
            let transaction = format!("readiness_{}", crate::secret_token());
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let result = async {
                    matrix.ensure_service().await?;
                    let room = match inbox.probe_room().await? {
                        Some(room) => room,
                        None => {
                            let room = matrix.create_probe_room().await?;
                            inbox.retain_probe_room(&room).await?;
                            inbox
                                .probe_room()
                                .await?
                                .ok_or(Error::Unavailable("readiness_unavailable"))?
                        }
                    };
                    let event = matrix.send_probe(&room, &transaction).await?;
                    for _ in 0..30 {
                        if inbox.observed_event(&event).await? {
                            return Ok::<(), Error>(());
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    Err(Error::Unavailable("appservice_roundtrip_unconfirmed"))
                }
                .await;
                if result.is_ok() {
                    ready.store(true, Ordering::Release);
                    return;
                }
            }
        }))
    }
    async fn accept_service_invitation(
        &self,
        gateway: &crate::gateway::Gateway,
        room: &str,
    ) -> Result<()> {
        if gateway.service_invited(room).await?
            && let Some(matrix) = &self.matrix_client
        {
            matrix.join_invited_service(room).await?;
        }
        Ok(())
    }
    async fn provision_creation(
        &self,
        _domain: &crate::domain::DomainStore,
        _gateway: &crate::gateway::Gateway,
        _p: &crate::store::Principal,
        creation: crate::domain::AgentBinding,
        _space: &str,
    ) -> Result<Value> {
        // The committed binding is the durable command. Only the reconciler
        // performs Matrix joins, so HTTP cancellation never drops the intent.
        Ok(
            json!({"commandState":if creation.binding.state=="active"{"active"}else{"pending"},"creation":creation,"pendingReason":null}),
        )
    }
    async fn call(&self, req: &mut Request) -> Result<Value> {
        let hosts = req.headers().get_all("host").iter().collect::<Vec<_>>();
        let expected = self.public[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
        if hosts.len() != 1
            || hosts[0].to_str().ok() != Some(expected.as_str())
            || req.headers().contains_key("origin")
            || req.headers().contains_key("cookie")
            || req.headers().contains_key("x-forwarded-host")
            || req.headers().get_all("authorization").iter().count() > 1
        {
            return Err(Error::Unauthorized("native_origin_required"));
        }
        let path = req.uri().path().to_owned();
        let method = req.method().clone();
        if path == "/api/hagency/v1/readiness" && method == salvo::http::Method::GET {
            return Ok(json!({"startupRoundtripConfirmed":self.startup_ready()}));
        }
        if path == "/api/hagency/v1/discovery" && method == salvo::http::Method::GET {
            return Ok(
                json!({"product":"hagency-server","version":env!("CARGO_PKG_VERSION"),
                "capabilities":["pasion-oauth","owner-agent-appservice-v1","projects-matrix-spaces-v1","device-execution-v1","execution-history-v1","global-agent-identity-v2","execution-device-v1","owner-direct-v1","processing-reaction-v1"],
                "protocolVersion":3,"serverName":self.server_name,"homeserver":self.public,
                "serviceMxid":format!("@_hagency_service:{}",self.server_name),"issuer":self.issuer,"authorizationEndpoint":self.issuer.join("authorize").unwrap(),
                "tokenEndpoint":self.issuer.join("oauth2/token").unwrap(),
                "registrationEndpoint":self.issuer.join("oauth2/registration").unwrap(),"authorizationWindowMs":30000}),
            );
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Proof {
            access_token: String,
        }
        let now = now_ms();
        if path == "/api/hagency/v1/sessions/pasion" && method == salvo::http::Method::POST {
            let body: Proof = req
                .parse_json()
                .await
                .map_err(|_| Error::Invalid("invalid_arguments"))?;
            let identity = self.verifier.verify(&body.access_token, now_ms()).await?;
            return serde_json::to_value(self.store.sign_in(identity, now_ms()).await?)
                .map_err(|_| Error::Unavailable("encoding_failed"));
        }
        let credential = req
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .ok_or(Error::Unauthorized("authentication_required"))?
            .to_owned();
        if path == "/api/hagency/v1/sessions/current/renew" && method == salvo::http::Method::POST {
            let body: Proof = req
                .parse_json()
                .await
                .map_err(|_| Error::Invalid("invalid_arguments"))?;
            let identity = self.verifier.verify(&body.access_token, now_ms()).await?;
            return Ok(json!({"validUntilMs":self.store.renew(&credential,identity,now).await?}));
        }
        if path == "/api/hagency/v1/sessions/current" && method == salvo::http::Method::DELETE {
            self.store.sign_out(&credential).await?;
            return Ok(json!({"signedOut":true}));
        }
        if path == "/api/hagency/v1/identity" && method == salvo::http::Method::GET {
            let p = self.store.authenticate(&credential, now, false).await?;
            return Ok(
                json!({"userId":p.user_id,"mxid":p.mxid,"subject":p.subject,"clientId":p.client_id,"validUntilMs":p.valid_until_ms,"issuer":self.issuer}),
            );
        }
        if path == "/api/hagency/v1/devices" && method == salvo::http::Method::POST {
            let body: RegisterDevice = req
                .parse_json()
                .await
                .map_err(|_| Error::Invalid("invalid_arguments"))?;
            return serde_json::to_value(
                self.store
                    .register_device(&credential, body, now_ms())
                    .await?,
            )
            .map_err(|_| Error::Unavailable("encoding_failed"));
        }
        if let Some(id) = path.strip_prefix("/api/hagency/v1/devices/")
            && method == salvo::http::Method::DELETE
        {
            self.store.revoke_device(&credential, id, now).await?;
            return Ok(json!({"revoked":true}));
        }
        if path.starts_with("/api/hagency/v1/execution/") {
            let principal = self.store.authenticate(&credential, now_ms(), true).await?;
            let transport = self
                .transport
                .as_ref()
                .ok_or(Error::Unavailable("transport_unavailable"))?;
            let gateway = &self
                .domain
                .as_ref()
                .ok_or(Error::Unavailable("domain_unavailable"))?
                .1;
            return crate::api_transport::call(transport, gateway, &principal, req).await;
        }
        let device_management = (path == "/api/hagency/v1/agents"
            && method == salvo::http::Method::POST)
            || (path.ends_with("/execution-device") && method == salvo::http::Method::PUT);
        let principal = self
            .store
            .authenticate(&credential, now, device_management)
            .await?;
        if let Some((domain, gateway)) = &self.domain {
            if path == "/api/hagency/v1/agents" && method == salvo::http::Method::GET {
                return Ok(json!({"agents":domain.agents(&principal,now).await?}));
            }
            if path == "/api/hagency/v1/projects" && method == salvo::http::Method::GET {
                let mut projects = Vec::new();
                for project in domain.projects(&principal, now).await? {
                    if let Some(metadata) = gateway
                        .member_metadata(&project.space_id, &principal.mxid)
                        .await?
                    {
                        let mut value = serde_json::to_value(project)
                            .map_err(|_| Error::Unavailable("invalid_project_response"))?;
                        value["name"] = metadata["name"].clone();
                        value["topic"] = metadata["topic"].clone();
                        projects.push(value);
                    }
                }
                self.store
                    .authenticate(&credential, now_ms(), false)
                    .await?;
                return Ok(json!({"projects":projects}));
            }
            if path == "/api/hagency/v1/projects/adopt" && method == salvo::http::Method::POST {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Adopt {
                    space_id: String,
                }
                let body: Adopt = req
                    .parse_json()
                    .await
                    .map_err(|_| Error::Invalid("invalid_arguments"))?;
                let facts = gateway.admin(&body.space_id, &principal.mxid, None).await?;
                return Ok(
                    json!({"project":domain.register_project(&principal,&body.space_id,&facts,now_ms()).await?}),
                );
            }
            if path == "/api/hagency/v1/devices" && method == salvo::http::Method::GET {
                return Ok(json!({"devices":domain.owner_devices(&principal,now_ms()).await?}));
            }
            if path == "/api/hagency/v1/agents" && method == salvo::http::Method::POST {
                if !self.startup_ready() {
                    return Err(Error::Unavailable("appservice_startup_not_ready"));
                }
                let body: crate::domain::CreateAgent = req
                    .parse_json()
                    .await
                    .map_err(|_| Error::Invalid("invalid_arguments"))?;
                let principal = self.store.authenticate(&credential, now_ms(), true).await?;
                let agent = domain.create_agent(&principal, body, now_ms()).await?;
                return Ok(
                    json!({"commandState":if agent.state=="active" {"active"} else {"pending"},"creation":{"agent":agent},"pendingReason":null}),
                );
            }
            let parts = path.split('/').collect::<Vec<_>>();
            if parts.len() == 7
                && parts[4] == "agents"
                && parts[6] == "owner-direct"
                && method == salvo::http::Method::GET
            {
                let agent = domain.agent(&principal, parts[5], now_ms()).await?;
                let binding = domain
                    .bindings(&principal, &agent.id, now_ms())
                    .await?
                    .into_iter()
                    .find(|b| b.scope_kind == "owner_direct");
                return Ok(
                    json!({"ownerDirectRoomId":agent.owner_direct_room_id,"binding":binding}),
                );
            }
            if parts.len() == 7
                && parts[4] == "agents"
                && parts[6] == "owner-direct"
                && method == salvo::http::Method::POST
            {
                if !self.startup_ready() {
                    return Err(Error::Unavailable("appservice_startup_not_ready"));
                }
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct OwnerDirect {
                    room_id: String,
                }
                let body: OwnerDirect = req
                    .parse_json()
                    .await
                    .map_err(|_| Error::Invalid("invalid_arguments"))?;
                let agent = domain.agent(&principal, parts[5], now_ms()).await?;
                let facts = gateway
                    .room(&body.room_id, "", &principal.mxid, Some(&agent.puppet_mxid))
                    .await?;
                let creation = domain
                    .adopt_owner_direct(&principal, &agent.id, &body.room_id, &facts, now_ms())
                    .await?;
                return Ok(
                    json!({"ownerDirectRoomId":body.room_id,"commandState":if creation.binding.state=="active" {"active"}else{"pending"},"creation":creation}),
                );
            }
            if parts.len() == 7
                && parts[4] == "agents"
                && parts[6] == "execution-device"
                && method == salvo::http::Method::PUT
            {
                let principal = self.store.authenticate(&credential, now_ms(), true).await?;
                let body: crate::domain::SetExecutionDevice = req
                    .parse_json()
                    .await
                    .map_err(|_| Error::Invalid("invalid_arguments"))?;
                return Ok(
                    json!({"agent":domain.set_execution_device(&principal,parts[5],body,now_ms()).await?}),
                );
            }

            if parts.len() == 7
                && parts[4] == "projects"
                && parts[6] == "rooms"
                && method == salvo::http::Method::GET
            {
                let project = domain.project(&principal, parts[5], now_ms()).await?;
                if !gateway.visible(&project.space_id, &principal.mxid).await? {
                    return Err(Error::Unauthorized("project_membership_required"));
                }
                let mut pending = domain
                    .registered_rooms(&principal, &project.id, now_ms())
                    .await?
                    .into_iter();
                let mut checks = tokio::task::JoinSet::new();
                let mut rooms = Vec::new();
                loop {
                    while checks.len() < 8 {
                        let Some(room) = pending.next() else {
                            break;
                        };
                        let gateway = gateway.clone();
                        let actor = principal.mxid.clone();
                        let space = project.space_id.clone();
                        checks.spawn(async move {
                            let facts = tokio::time::timeout(
                                std::time::Duration::from_secs(8),
                                gateway.room(&room.room_id, &space, &actor, None),
                            )
                            .await
                            .map_err(|_| Error::Unavailable("matrix_state_timeout"))??;
                            Ok::<_, Error>((
                                room,
                                facts.owner_in_space && facts.owner_in_room && facts.room_in_space,
                            ))
                        });
                    }
                    let Some(checked) = checks.join_next().await else {
                        break;
                    };
                    let (room, visible) =
                        checked.map_err(|_| Error::Unavailable("matrix_state_unavailable"))??;
                    if visible
                        && let Some(metadata) = gateway
                            .member_metadata(&room.room_id, &principal.mxid)
                            .await?
                    {
                        let mut value = serde_json::to_value(room)
                            .map_err(|_| Error::Unavailable("invalid_room_response"))?;
                        value["name"] = metadata["name"].clone();
                        value["topic"] = metadata["topic"].clone();
                        rooms.push(value);
                    }
                }
                self.store
                    .authenticate(&credential, now_ms(), false)
                    .await?;
                rooms.sort_by(|a, b| a["roomId"].as_str().cmp(&b["roomId"].as_str()));
                return Ok(json!({"rooms":rooms}));
            }
            if parts.len() == 7 && parts[4] == "commands" && method == salvo::http::Method::GET {
                let creation = domain
                    .command_status(&principal, parts[5], parts[6], now_ms())
                    .await?;
                let state = match creation
                    .binding
                    .as_ref()
                    .map(|b| b.state.as_str())
                    .unwrap_or(creation.agent.state.as_str())
                {
                    "active" => "active",
                    "joining" | "creating" => "pending",
                    other => other,
                };
                return Ok(
                    json!({"operation":parts[5],"idempotencyKey":parts[6],"commandState":state,"creation":creation}),
                );
            }
            if (parts.len() == 6 || parts.len() == 7) && parts[4] == "bindings" {
                let binding = domain.binding(&principal, parts[5], now_ms()).await?;
                if parts.len() == 6 && method == salvo::http::Method::GET {
                    return Ok(json!({"binding":binding}));
                }
                if parts.len() == 7
                    && parts[6] == "reply-policy"
                    && method == salvo::http::Method::PUT
                {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct ReplyPolicy {
                        thread_auto_reply: bool,
                    }
                    let body: ReplyPolicy = req
                        .parse_json()
                        .await
                        .map_err(|_| Error::Invalid("invalid_arguments"))?;
                    return Ok(
                        json!({"binding":domain.set_thread_auto_reply(&principal,&binding.id,body.thread_auto_reply,now_ms()).await?}),
                    );
                }
                if parts.len() == 6 && method == salvo::http::Method::DELETE {
                    // Durable desired state comes first. A trusted lifecycle worker
                    // completes the actual Matrix departure independently of login.
                    return Ok(
                        json!({"binding":domain.leave_binding(&principal,&binding.id,now_ms()).await?}),
                    );
                }
                if parts.len() == 7 && method == salvo::http::Method::POST {
                    if parts[6] == "pause" {
                        return Ok(
                            json!({"binding":domain.suspend_binding(&principal,&binding.id,now_ms()).await?}),
                        );
                    }
                    if parts[6] == "resume" {
                        let agent = domain
                            .agent(&principal, &binding.agent_id, now_ms())
                            .await?;
                        let space = match &binding.project_id {
                            Some(project) => {
                                domain
                                    .project(&principal, project, now_ms())
                                    .await?
                                    .space_id
                            }
                            None => String::new(),
                        };
                        let facts = gateway
                            .room(
                                &binding.room_id,
                                &space,
                                &principal.mxid,
                                Some(&agent.puppet_mxid),
                            )
                            .await?;
                        return Ok(
                            json!({"binding":domain.resume_binding(&principal,&binding.id,&facts,now_ms()).await?}),
                        );
                    }
                }
            }
            if parts.len() == 7
                && parts[4] == "projects"
                && parts[6] == "creation-policy"
                && method == salvo::http::Method::PUT
            {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Policy {
                    expected_revision: i64,
                    policy: crate::domain::CreationPolicy,
                }
                let body: Policy = req
                    .parse_json()
                    .await
                    .map_err(|_| Error::Invalid("invalid_arguments"))?;
                let project = domain.project(&principal, parts[5], now_ms()).await?;
                let facts = gateway
                    .admin(&project.space_id, &principal.mxid, None)
                    .await?;
                return Ok(
                    json!({"project":domain.set_project_policy(&principal,&project.id,body.expected_revision,body.policy,&facts,now_ms()).await?}),
                );
            }
            if parts.len() == 7
                && parts[4] == "projects"
                && parts[6] == "service-state"
                && method == salvo::http::Method::GET
            {
                let project = domain.project(&principal, parts[5], now_ms()).await?;
                let facts = gateway
                    .admin(&project.space_id, &principal.mxid, None)
                    .await?;
                if !facts.joined {
                    return Err(Error::Unauthorized("project_membership_required"));
                }
                let mut state = domain
                    .service_state(&principal, &project.id, None, now_ms())
                    .await?;
                state["canManagePolicy"] = json!(facts.is_space && facts.can_manage_policy);
                return Ok(state);
            }
            if parts.len() == 7
                && parts[4] == "projects"
                && matches!(parts[6], "pause-service" | "clear-service-pause")
                && method == salvo::http::Method::POST
            {
                if !req
                    .payload()
                    .await
                    .map_err(|_| Error::Invalid("invalid_arguments"))?
                    .is_empty()
                {
                    return Err(Error::Invalid("invalid_arguments"));
                }
                let project = domain.project(&principal, parts[5], now_ms()).await?;
                let facts = gateway
                    .admin(&project.space_id, &principal.mxid, None)
                    .await?;
                let paused = parts[6] == "pause-service";
                let affected = if paused {
                    domain
                        .suspend_project_bindings(&principal, &project.id, &facts, now_ms())
                        .await?
                } else {
                    domain
                        .clear_project_pause(&principal, &project.id, &facts, now_ms())
                        .await?
                };
                return Ok(
                    json!({"servicePaused":paused,"affectedBindings":affected,"ownerResumeRequired":!paused}),
                );
            }
            if parts.len() == 9 && parts[4] == "projects" && parts[6] == "rooms" {
                let room = percent_encoding::percent_decode_str(parts[7])
                    .decode_utf8()
                    .map_err(|_| Error::Invalid("invalid_room_id"))?
                    .into_owned();
                if room.contains('/') || room.contains('?') || room.contains('#') {
                    return Err(Error::Invalid("invalid_room_id"));
                }
                let project = domain.project(&principal, parts[5], now_ms()).await?;
                if parts[8] == "agents" && method == salvo::http::Method::GET {
                    // A Room member need not be a Space member. Conversely Space
                    // membership alone never reveals a private Room's roster.
                    let facts = gateway
                        .admin(&room, &principal.mxid, Some(&project.space_id))
                        .await?;
                    if !facts.joined
                        || facts.linked_space_id.as_deref() != Some(project.space_id.as_str())
                    {
                        return Err(Error::Unauthorized("room_membership_required"));
                    }
                    let mut agents = Vec::new();
                    for agent in domain
                        .room_roster(&principal, &project.id, &room, now_ms())
                        .await?
                    {
                        if gateway.visible(&room, &agent.puppet_mxid).await? {
                            agents.push(agent);
                        }
                    }
                    self.store
                        .authenticate(&credential, now_ms(), false)
                        .await?;
                    return Ok(json!({"agents":agents}));
                }
                if parts[8] == "service-state" && method == salvo::http::Method::GET {
                    if !gateway.visible(&project.space_id, &principal.mxid).await?
                        || !gateway.visible(&room, &principal.mxid).await?
                    {
                        return Err(Error::Unauthorized("room_membership_required"));
                    }
                    let facts = gateway
                        .admin(&room, &principal.mxid, Some(&project.space_id))
                        .await?;
                    let mut state = domain
                        .service_state(&principal, &project.id, Some(&room), now_ms())
                        .await?;
                    state["canManagePolicy"] = json!(
                        facts.joined
                            && facts.linked_space_id.as_deref() == Some(project.space_id.as_str())
                            && facts.can_manage_policy
                    );
                    return Ok(state);
                }
                let facts = gateway
                    .admin(room.as_str(), &principal.mxid, Some(&project.space_id))
                    .await?;
                if parts[8] == "creation-policy" && method == salvo::http::Method::PUT {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct Policy {
                        expected_revision: i64,
                        policy: crate::domain::RoomCreationPolicy,
                    }
                    let body: Policy = req
                        .parse_json()
                        .await
                        .map_err(|_| Error::Invalid("invalid_arguments"))?;
                    return Ok(
                        json!({"room":domain.set_room_policy(&principal,room.as_str(),&project.id,body.expected_revision,body.policy,&facts,now_ms()).await?}),
                    );
                }
                if matches!(parts[8], "pause-service" | "clear-service-pause")
                    && method == salvo::http::Method::POST
                {
                    if !req
                        .payload()
                        .await
                        .map_err(|_| Error::Invalid("invalid_arguments"))?
                        .is_empty()
                    {
                        return Err(Error::Invalid("invalid_arguments"));
                    }
                    let paused = parts[8] == "pause-service";
                    let affected = if paused {
                        domain
                            .suspend_room_bindings(
                                &principal,
                                &project.id,
                                room.as_str(),
                                &facts,
                                now_ms(),
                            )
                            .await?
                    } else {
                        domain
                            .clear_room_pause(
                                &principal,
                                &project.id,
                                room.as_str(),
                                &facts,
                                now_ms(),
                            )
                            .await?
                    };
                    return Ok(
                        json!({"servicePaused":paused,"affectedBindings":affected,"ownerResumeRequired":!paused}),
                    );
                }
            }
            if parts.len() == 6 && parts[4] == "agents" {
                if method == salvo::http::Method::GET {
                    return Ok(json!({"agent":domain.agent(&principal,parts[5],now_ms()).await?}));
                }
                if method == salvo::http::Method::DELETE {
                    return Ok(
                        json!({"agent":domain.retire_agent(&principal,parts[5],now_ms()).await?}),
                    );
                }
            }
            if parts.len() == 7 && parts[4] == "agents" {
                if parts[6] == "bindings" && method == salvo::http::Method::GET {
                    return Ok(
                        json!({"bindings":domain.bindings(&principal,parts[5],now_ms()).await?}),
                    );
                }
                if parts[6] == "bindings" && method == salvo::http::Method::POST {
                    if !self.startup_ready() {
                        return Err(Error::Unavailable("appservice_startup_not_ready"));
                    }
                    let body: crate::domain::BindRoom = req
                        .parse_json()
                        .await
                        .map_err(|_| Error::Invalid("invalid_arguments"))?;
                    let agent = domain.agent(&principal, parts[5], now_ms()).await?;
                    let project = domain
                        .project(&principal, &body.project_id, now_ms())
                        .await?;
                    self.accept_service_invitation(gateway, &body.room_id)
                        .await?;
                    let facts = gateway
                        .room(
                            &body.room_id,
                            &project.space_id,
                            &principal.mxid,
                            Some(&agent.puppet_mxid),
                        )
                        .await?;
                    let creation = domain
                        .bind_room(&principal, &agent.id, body, &facts, now_ms())
                        .await?;
                    return self
                        .provision_creation(
                            domain,
                            gateway,
                            &principal,
                            creation,
                            &project.space_id,
                        )
                        .await;
                }
                if method == salvo::http::Method::POST && parts[6] == "pause" {
                    return Ok(
                        json!({"agent":domain.suspend_agent(&principal,parts[5],now_ms()).await?}),
                    );
                }
                if method == salvo::http::Method::POST && parts[6] == "resume" {
                    return Ok(
                        json!({"agent":domain.resume_agent(&principal,parts[5],now_ms()).await?}),
                    );
                }
            }
            if parts.len() == 8
                && parts[4] == "projects"
                && parts[6] == "rooms"
                && parts[7] == "adopt"
                && method == salvo::http::Method::POST
            {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Adopt {
                    room_id: String,
                }
                let body: Adopt = req
                    .parse_json()
                    .await
                    .map_err(|_| Error::Invalid("invalid_arguments"))?;
                let project = domain.project(&principal, parts[5], now).await?;
                let space = gateway
                    .admin(&project.space_id, &principal.mxid, None)
                    .await?;
                let room = gateway
                    .admin(&body.room_id, &principal.mxid, Some(&project.space_id))
                    .await?;
                return Ok(
                    json!({"room":domain.register_room(&principal,&project.id,&body.room_id,&space,&room,now_ms()).await?}),
                );
            }
        }
        Err(Error::Invalid("unsupported_operation"))
    }
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
#[handler]
impl App {
    async fn handle(&self, req: &mut Request, res: &mut Response) {
        res.add_header("cache-control", "no-store", true).unwrap();
        match self.call(req).await {
            Ok(value) => {
                if value["commandState"] == "pending" {
                    res.status_code(StatusCode::ACCEPTED);
                }
                res.render(Json(value));
            }
            Err(error) => {
                res.status_code(StatusCode::from_u16(error.status()).unwrap());
                res.render(Json(json!({"code":error.to_string()})));
            }
        }
    }
}

#[derive(Clone)]
struct StartupReadiness(Arc<AtomicBool>);
#[handler]
impl StartupReadiness {
    async fn handle(&self, res: &mut Response) {
        let ready = self.0.load(Ordering::Acquire);
        res.status_code(if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        });
        res.render(Json(json!({"startupRoundtripConfirmed":ready})));
    }
}
