//! Server-only Appservice credentials and namespace-confined puppet operations.
use crate::{Error, Result, domain::Agent};
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};
use std::time::Duration;
#[derive(Clone)]
pub struct MatrixClient {
    http: Client,
    origin: Url,
    as_token: String,
    service: String,
    server: String,
}
impl MatrixClient {
    pub(crate) fn service_mxid(&self) -> &str {
        &self.service
    }
    pub fn new(origin: Url, as_token: String, server: String) -> Result<Self> {
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| Error::Unavailable("matrix_unavailable"))?;
        Ok(Self {
            http,
            origin,
            as_token,
            service: format!("@_hagency_service:{server}"),
            server,
        })
    }
    fn puppet(&self, user: &str) -> Result<()> {
        let Some((local, server)) = user.split_once(':') else {
            return Err(Error::Invalid("invalid_puppet_identity"));
        };
        if !local.starts_with("@_hagency_")
            || !local[1..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            || server != self.server
        {
            return Err(Error::Invalid("invalid_puppet_identity"));
        }
        Ok(())
    }
    async fn call(
        &self,
        method: Method,
        segments: &[&str],
        user: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        self.puppet(user)?;
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| Error::Invalid("invalid_matrix_origin"))?
            .clear()
            .extend(segments);
        url.query_pairs_mut().append_pair("user_id", user);
        let mut request = self.http.request(method, url).bearer_auth(&self.as_token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| Error::Unavailable("matrix_outcome_unknown"))?;
        if !response.status().is_success() {
            return Err(if response.status().as_u16() == 403 {
                Error::Conflict("matrix_permission_missing")
            } else {
                Error::Unavailable("matrix_request_failed")
            });
        }
        let mut raw = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Unavailable("matrix_outcome_unknown"))?
        {
            if raw.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(Error::Unavailable("matrix_response_too_large"));
            }
            raw.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&raw).map_err(|_| Error::Unavailable("invalid_matrix_response"))
    }
    pub async fn create_probe_room(&self) -> Result<String> {
        let value=self.call(Method::POST,&["_matrix","client","v3","createRoom"],&self.service,
            Some(json!({"preset":"private_chat","visibility":"private","name":"Hagency service readiness","creation_content":{"m.federate":false}}))).await?;
        value["room_id"]
            .as_str()
            .filter(|r| r.starts_with('!'))
            .map(str::to_owned)
            .ok_or(Error::Unavailable("invalid_matrix_response"))
    }
    pub async fn send_probe(&self, room: &str, transaction: &str) -> Result<String> {
        self.send(
            &self.service,
            room,
            transaction,
            json!({"msgtype":"m.notice","body":"Hagency service readiness probe"}),
        )
        .await
    }
    pub async fn ensure_service(&self) -> Result<()> {
        let who = self
            .call(
                Method::GET,
                &["_matrix", "client", "v3", "account", "whoami"],
                &self.service,
                None,
            )
            .await?;
        if who["user_id"] != self.service {
            return Err(Error::Conflict("matrix_identity_mismatch"));
        }
        Ok(())
    }
    pub async fn join_invited_service(&self, room: &str) -> Result<()> {
        // Matrix itself requires an invitation or valid join rule; no join bypass.
        self.call(
            Method::POST,
            &["_matrix", "client", "v3", "join", room],
            &self.service,
            Some(json!({})),
        )
        .await?;
        Ok(())
    }
    pub async fn provision(&self, agent: &Agent, room: &str, invite: bool) -> Result<()> {
        self.provision_identity(agent).await?;
        if invite {
            self.call(
                Method::POST,
                &["_matrix", "client", "v3", "rooms", room, "invite"],
                &self.service,
                Some(json!({"user_id":agent.puppet_mxid})),
            )
            .await?;
        }
        self.call(
            Method::POST,
            &["_matrix", "client", "v3", "join", room],
            &agent.puppet_mxid,
            Some(json!({})),
        )
        .await?;
        Ok(())
    }
    pub async fn provision_identity(&self, agent: &Agent) -> Result<()> {
        let who = self
            .call(
                Method::GET,
                &["_matrix", "client", "v3", "account", "whoami"],
                &agent.puppet_mxid,
                None,
            )
            .await?;
        if who["user_id"] != agent.puppet_mxid {
            return Err(Error::Conflict("matrix_identity_mismatch"));
        }
        self.call(
            Method::PUT,
            &[
                "_matrix",
                "client",
                "v3",
                "profile",
                &agent.puppet_mxid,
                "displayname",
            ],
            &agent.puppet_mxid,
            Some(json!({"displayname":agent.display_name})),
        )
        .await?;
        Ok(())
    }
    pub(crate) async fn managed_verify_identity(&self,agent:&Agent)->Result<()> {
        let who=self.call(Method::GET,&["_matrix","client","v3","account","whoami"],&agent.puppet_mxid,None).await?;
        if who["user_id"]!=agent.puppet_mxid {return Err(Error::Conflict("matrix_identity_mismatch"));} Ok(())
    }
    pub(crate) async fn managed_set_display_name(&self,agent:&Agent)->Result<()> {
        self.call(Method::PUT,&["_matrix","client","v3","profile",&agent.puppet_mxid,"displayname"],&agent.puppet_mxid,Some(json!({"displayname":agent.display_name}))).await?;Ok(())
    }
    pub(crate) async fn managed_invite(&self,agent:&Agent,room:&str)->Result<()> {
        self.call(Method::POST,&["_matrix","client","v3","rooms",room,"invite"],&self.service,Some(json!({"user_id":agent.puppet_mxid}))).await?;Ok(())
    }
    pub(crate) async fn managed_join(&self,agent:&Agent,room:&str)->Result<()> {
        self.call(Method::POST,&["_matrix","client","v3","join",room],&agent.puppet_mxid,Some(json!({}))).await?;Ok(())
    }
    pub async fn send(
        &self,
        puppet: &str,
        room: &str,
        transaction: &str,
        content: Value,
    ) -> Result<String> {
        self.send_typed(puppet, room, transaction, "m.room.message", content)
            .await
    }
    pub(crate) async fn send_processing(
        &self,
        puppet: &str,
        room: &str,
        transaction: &str,
        content: Value,
    ) -> Result<String> {
        self.send_typed(puppet, room, transaction, "m.reaction", content)
            .await
    }
    async fn send_typed(
        &self,
        puppet: &str,
        room: &str,
        transaction: &str,
        event_type: &str,
        content: Value,
    ) -> Result<String> {
        let response = self
            .call(
                Method::PUT,
                &[
                    "_matrix",
                    "client",
                    "v3",
                    "rooms",
                    room,
                    "send",
                    event_type,
                    transaction,
                ],
                puppet,
                Some(content),
            )
            .await?;
        response["event_id"]
            .as_str()
            .filter(|id| id.starts_with('$'))
            .map(str::to_owned)
            .ok_or(Error::Unavailable("invalid_matrix_response"))
    }
    pub async fn leave(&self, puppet: &str, room: &str) -> Result<()> {
        self.call(
            Method::POST,
            &["_matrix", "client", "v3", "rooms", room, "leave"],
            puppet,
            Some(json!({})),
        )
        .await?;
        Ok(())
    }
}
