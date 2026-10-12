//! Persist transaction and routing intent together before acknowledging Matrix.
use crate::{Error, Result, hash};
use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Jsonb, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use salvo::prelude::*;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

#[derive(Clone, Copy)]
pub struct Limits {
    pub max_pending: i64,
    pub max_records: i64,
    pub max_bytes: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pending: 10000,
            max_records: 20000,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}
#[derive(Clone)]
pub struct Inbox {
    db: Arc<Mutex<AsyncPgConnection>>,
    hs_token: String,
    limits: Limits,
    known_user: Option<KnownUser>,
}
pub type KnownUser =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<bool>> + Send>> + Send + Sync>;
#[derive(diesel::QueryableByName)]
pub(crate) struct PendingTransaction {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Jsonb)]
    pub body: Value,
    #[diesel(sql_type=BigInt)]
    pub received_at_ms: i64,
}
impl Inbox {
    pub(crate) async fn probe_room(&self) -> Result<Option<String>> {
        #[derive(diesel::QueryableByName)]
        struct Room {
            #[diesel(sql_type=Text)]
            room_id: String,
        }
        Ok(
            sql_query("SELECT room_id FROM hagency_agent_v1.readiness_room WHERE singleton")
                .load::<Room>(&mut *self.db.lock().await)
                .await?
                .into_iter()
                .next()
                .map(|r| r.room_id),
        )
    }
    pub(crate) async fn retain_probe_room(&self, room: &str) -> Result<()> {
        sql_query("INSERT INTO hagency_agent_v1.readiness_room(singleton,room_id) VALUES(true,$1) ON CONFLICT DO NOTHING").bind::<Text,_>(room).execute(&mut *self.db.lock().await).await?;
        Ok(())
    }
    pub(crate) async fn observed_event(&self, event: &str) -> Result<bool> {
        #[derive(diesel::QueryableByName)]
        struct Observed {
            #[diesel(sql_type=Bool)]
            matched: bool,
        }
        Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.readiness_receipts WHERE event_id=$2) OR EXISTS(SELECT 1 FROM hagency_agent_v1.inbound_transactions WHERE body->'events' @> $1::jsonb) AS matched").bind::<Jsonb,_>(json!([{"event_id":event}])).bind::<Text,_>(event).get_result::<Observed>(&mut *self.db.lock().await).await?.matched)
    }
    pub(crate) async fn pending(&self) -> Result<Vec<PendingTransaction>> {
        Ok(sql_query("SELECT t.id,t.body,t.received_at_ms FROM hagency_agent_v1.inbound_transactions t JOIN hagency_agent_v1.routing_jobs j ON j.transaction_id=t.id WHERE j.state='pending' ORDER BY j.received_at_ms,t.id LIMIT 10")
            .load(&mut *self.db.lock().await).await?)
    }
    pub(crate) async fn routed(&self, id: &str) -> Result<()> {
        let mut guard = self.db.lock().await;
        (*guard).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            sql_query("UPDATE hagency_agent_v1.routing_jobs SET state='routed' WHERE transaction_id=$1").bind::<Text,_>(id).execute(db).await?;
            // All recipient queues have durably accepted or definitively rejected
            // the event. Keep permanent id/digest tombstones, discard duplicate
            // plaintext. Unknown execution/reply/cost records are untouched.
            sql_query("UPDATE hagency_agent_v1.inbound_transactions SET body='{\"events\":[]}'::jsonb WHERE id=$1 AND EXISTS(SELECT 1 FROM hagency_agent_v1.routing_jobs WHERE transaction_id=$1 AND state='routed')").bind::<Text,_>(id).execute(db).await?;
            Ok(())
        }).await
    }
    pub async fn open(url: &str, hs_token: String) -> Result<Self> {
        if hs_token.len() < 32 {
            return Err(Error::Invalid("invalid_appservice_credential"));
        }
        let mut db = AsyncPgConnection::establish(url)
            .await
            .map_err(|_| Error::Unavailable("database_unavailable"))?;
        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            // PostgreSQL IF NOT EXISTS alone does not serialize concurrent
            // catalog/type creation. Use the same inbox transaction lock as
            // acceptance/compaction before creating this fresh schema subset.
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            db.batch_execute("CREATE TABLE IF NOT EXISTS hagency_agent_v1.inbound_transactions (id text PRIMARY KEY,digest text NOT NULL,body jsonb NOT NULL,received_at_ms bigint NOT NULL); CREATE TABLE IF NOT EXISTS hagency_agent_v1.routing_jobs (transaction_id text PRIMARY KEY REFERENCES hagency_agent_v1.inbound_transactions(id),state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','routed')),received_at_ms bigint NOT NULL)").await?;
            db.batch_execute("CREATE TABLE IF NOT EXISTS hagency_agent_v1.routing_rejections (transaction_id text NOT NULL REFERENCES hagency_agent_v1.inbound_transactions(id) ON DELETE CASCADE,event_id text NOT NULL,binding_id text NOT NULL,reason text NOT NULL,PRIMARY KEY(transaction_id,event_id,binding_id))").await?;
            db.batch_execute("CREATE TABLE IF NOT EXISTS hagency_agent_v1.readiness_room (singleton boolean PRIMARY KEY CHECK(singleton),room_id text NOT NULL)").await?;
            db.batch_execute("CREATE TABLE IF NOT EXISTS hagency_agent_v1.readiness_receipts (event_id text PRIMARY KEY,received_at_ms bigint NOT NULL)").await?;
            Ok(())
        }).await?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            hs_token,
            limits: Limits::default(),
            known_user: None,
        })
    }
    pub fn with_limits(mut self, limits: Limits) -> Result<Self> {
        if limits.max_pending <= 0
            || limits.max_records < limits.max_pending
            || limits.max_bytes <= 0
        {
            return Err(Error::Invalid("invalid_queue_limits"));
        }
        self.limits = limits;
        Ok(self)
    }
    pub(crate) async fn rejected(
        &self,
        transaction: &str,
        event: &str,
        binding: &str,
        reason: &str,
    ) -> Result<()> {
        // Bounded metadata samples per transaction, without duplicating bodies.
        sql_query("INSERT INTO hagency_agent_v1.routing_rejections(transaction_id,event_id,binding_id,reason) SELECT $1,$2,$3,$4 WHERE (SELECT count(*) FROM hagency_agent_v1.routing_rejections WHERE transaction_id=$1)<16 ON CONFLICT DO NOTHING")
            .bind::<Text,_>(transaction).bind::<Text,_>(event).bind::<Text,_>(binding).bind::<Text,_>(reason)
            .execute(&mut *self.db.lock().await).await?;
        Ok(())
    }
    pub fn router(&self) -> Router {
        Router::with_path("_matrix/app/v1")
            .push(
                Router::with_path("transactions/{transaction_id}")
                    .hoop(salvo::size_limiter::max_size(2 * 1024 * 1024))
                    .put(self.clone()),
            )
            .push(
                Router::with_path("ping")
                    .hoop(salvo::size_limiter::max_size(4096))
                    .post(self.clone()),
            )
            .push(Router::with_path("users/{user_id}").get(self.clone()))
            .push(Router::with_path("rooms/{room_alias}").get(self.clone()))
    }
    pub fn with_known_user(mut self, known: KnownUser) -> Self {
        self.known_user = Some(known);
        self
    }
    pub async fn accept(&self, id: &str, body: Value, now: i64) -> Result<()> {
        if id.is_empty() || id.len() > 255 || id.chars().any(char::is_control) {
            return Err(Error::Invalid("invalid_transaction_id"));
        }
        if !body.is_object()
            || body
                .get("events")
                .is_none_or(|events| !events.is_array() || events.as_array().unwrap().len() > 5000)
        {
            return Err(Error::Invalid("invalid_transaction"));
        }
        let encoded = serde_json::to_string(&canonical(&body))
            .map_err(|_| Error::Invalid("invalid_transaction"))?;
        if encoded.len() > 2 * 1024 * 1024 {
            return Err(Error::Invalid("transaction_too_large"));
        }
        let digest = hash(&encoded);
        let limits = self.limits;
        let mut db = self.db.lock().await;
        (*db).transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection| {
            #[derive(diesel::QueryableByName)] struct Existing {#[diesel(sql_type=Text)] digest:String}
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            let existing=sql_query("SELECT digest FROM hagency_agent_v1.inbound_transactions WHERE id=$1").bind::<Text,_>(id).load::<Existing>(db).await?;
            if let Some(old)=existing.as_slice().first() {
                return if old.digest==digest {Ok(())} else {Err(Error::Conflict("transaction_content_mismatch"))};
            }
            #[derive(diesel::QueryableByName)] struct Count {#[diesel(sql_type=BigInt)] count:i64}
            let pending=sql_query("SELECT count(*) AS count FROM hagency_agent_v1.routing_jobs WHERE state='pending'").get_result::<Count>(db).await?;
            #[derive(diesel::QueryableByName)] struct Capacity {#[diesel(sql_type=BigInt)] records:i64,#[diesel(sql_type=BigInt)] bytes:i64}
            let capacity=sql_query("SELECT count(*) AS records,coalesce(sum(octet_length(t.body::text)),0)::bigint AS bytes FROM hagency_agent_v1.inbound_transactions t JOIN hagency_agent_v1.routing_jobs j ON j.transaction_id=t.id WHERE j.state='pending'").get_result::<Capacity>(db).await?;
            #[derive(diesel::QueryableByName)] struct Incoming {#[diesel(sql_type=BigInt)] bytes:i64}
            let payload_bytes=sql_query("SELECT octet_length($1::jsonb::text)::bigint AS bytes").bind::<Jsonb,_>(&body).get_result::<Incoming>(db).await?.bytes;
            if pending.count>=limits.max_pending || capacity.records>=limits.max_records || capacity.bytes.saturating_add(payload_bytes)>limits.max_bytes {return Err(Error::Unavailable("appservice_queue_full"));}
            sql_query("INSERT INTO hagency_agent_v1.readiness_receipts(event_id,received_at_ms) SELECT e->>'event_id',$2 FROM jsonb_array_elements($1->'events') e WHERE e->>'type'='m.room.message' AND split_part(e->>'sender',':',1)='@_hagency_service' AND e->'content'->>'msgtype'='m.notice' AND e->'content'->>'body'='Hagency service readiness probe' AND length(e->>'event_id') BETWEEN 2 AND 255 AND EXISTS(SELECT 1 FROM hagency_agent_v1.readiness_room r WHERE r.room_id=e->>'room_id') ON CONFLICT DO NOTHING")
                .bind::<Jsonb,_>(&body).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("DELETE FROM hagency_agent_v1.readiness_receipts WHERE received_at_ms<$1").bind::<BigInt,_>(now.saturating_sub(86_400_000)).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.inbound_transactions(id,digest,body,received_at_ms) VALUES($1,$2,$3,$4)").bind::<Text,_>(id).bind::<Text,_>(digest).bind::<Jsonb,_>(body).bind::<BigInt,_>(now).execute(db).await?;
            sql_query("INSERT INTO hagency_agent_v1.routing_jobs(transaction_id,received_at_ms) VALUES($1,$2)").bind::<Text,_>(id).bind::<BigInt,_>(now).execute(db).await?;
            Ok(())
        }).await
    }
    fn authenticated(&self, req: &Request) -> bool {
        let headers = req
            .headers()
            .get_all("authorization")
            .iter()
            .collect::<Vec<_>>();
        let query = req
            .uri()
            .query()
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .filter(|(k, _)| k == "access_token")
                    .map(|(_, v)| v.into_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if headers.len() > 1 || query.len() > 1 {
            return false;
        }
        let bearer = headers
            .as_slice()
            .first()
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "));
        let supplied = match (bearer, query.as_slice().first()) {
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b.as_str()),
            _ => None,
        };
        supplied.is_some_and(|value| {
            value.len() == self.hs_token.len()
                && bool::from(value.as_bytes().ct_eq(self.hs_token.as_bytes()))
        })
    }
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(object) => serde_json::to_value(
            object
                .iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .unwrap(),
        Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}
#[handler]
impl Inbox {
    async fn handle(&self, req: &mut Request, res: &mut Response) {
        let result = async {
            if !self.authenticated(req) {
                return Err(Error::Unauthorized("appservice_authentication_required"));
            }
            if req.uri().path() == "/_matrix/app/v1/ping" {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Ping {
                    transaction_id: Option<String>,
                }
                let ping: Ping = req
                    .parse_json()
                    .await
                    .map_err(|_| Error::Invalid("invalid_ping"))?;
                if ping
                    .transaction_id
                    .as_ref()
                    .is_some_and(|s| s.len() > 255 || s.chars().any(char::is_control))
                {
                    return Err(Error::Invalid("invalid_ping"));
                }
                return Ok(());
            }
            if let Some(user) = req.param::<String>("user_id") {
                if let Some(known) = &self.known_user
                    && known(user).await?
                {
                    return Ok(());
                }
                return Err(Error::NotFound("unknown_appservice_user"));
            }
            if req.param::<String>("room_alias").is_some() {
                return Err(Error::NotFound("unknown_appservice_room"));
            }
            let id = req
                .param::<String>("transaction_id")
                .ok_or(Error::Invalid("invalid_transaction_id"))?;
            let body = req
                .parse_json::<Value>()
                .await
                .map_err(|_| Error::Invalid("invalid_transaction"))?;
            self.accept(&id, body, crate::api::now_ms()).await
        }
        .await;
        match result {
            Ok(()) => res.render(Json(json!({}))),
            Err(error) => {
                let (status, code) = match error {
                    Error::Unauthorized(_) => (403, "M_FORBIDDEN"),
                    Error::Invalid(_) => (400, "M_BAD_JSON"),
                    Error::NotFound(_) => (404, "M_NOT_FOUND"),
                    _ => (error.status(), "M_UNKNOWN"),
                };
                res.status_code(StatusCode::from_u16(status).unwrap());
                res.render(Json(json!({"errcode":code,"error":error.to_string()})));
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn durable_transactions_are_idempotent_and_not_acked_on_conflict() {
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        crate::store::Store::open(&url, "example.test", "https://example.test/auth/")
            .await
            .unwrap();
        let inbox = Inbox::open(&url, "h".repeat(64)).await.unwrap();
        let id = crate::secret_token();
        inbox.accept(&id, json!({"events":[]}), 1).await.unwrap();
        inbox.accept(&id, json!({"events":[]}), 2).await.unwrap();
        assert!(
            inbox
                .accept(&id, json!({"events":[{"event_id":"$changed"}]}), 2)
                .await
                .is_err()
        );
        let reopened = Inbox::open(&url, "h".repeat(64)).await.unwrap();
        reopened.accept(&id, json!({"events":[]}), 3).await.unwrap();
        #[derive(diesel::QueryableByName)]
        struct Job {
            #[diesel(sql_type=Text)]
            state: String,
        }
        let jobs =
            sql_query("SELECT state FROM hagency_agent_v1.routing_jobs WHERE transaction_id=$1")
                .bind::<Text, _>(&id)
                .load::<Job>(&mut *reopened.db.lock().await)
                .await
                .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].state, "pending");
        let service = Service::new(inbox.router());
        use salvo::test::TestClient;
        let url = format!(
            "http://example.test/_matrix/app/v1/transactions/{}",
            crate::secret_token()
        );
        let denied = TestClient::put(&url)
            .json(&json!({"events":[]}))
            .send(&service)
            .await;
        assert_eq!(denied.status_code, Some(StatusCode::FORBIDDEN));
        let accepted = TestClient::put(format!("{url}?access_token={}", "h".repeat(64)))
            .json(&json!({"events":[]}))
            .send(&service)
            .await;
        assert_eq!(accepted.status_code, Some(StatusCode::OK));
    }
    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn routed_plaintext_is_compacted_without_forgetting_transaction_digest_or_probe_proof() {
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        crate::store::Store::open(&url, "example.test", "https://example.test/auth/")
            .await
            .unwrap();
        let inbox = Inbox::open(&url, "h".repeat(64)).await.unwrap();
        let room = format!("!probe_{}:example.test", crate::secret_token());
        inbox.retain_probe_room(&room).await.unwrap();
        // Obtain the shared first-writer probe room; other tests may open Inbox.
        let room = inbox.probe_room().await.unwrap().unwrap();
        let event = format!("${}", crate::secret_token());
        let body = json!({"events":[{"type":"m.room.message","room_id":room,"sender":"@_hagency_service:example.test","event_id":event,"content":{"msgtype":"m.notice","body":"Hagency service readiness probe"}}]});
        let id = crate::secret_token();
        inbox
            .accept(&id, body.clone(), crate::api::now_ms())
            .await
            .unwrap();
        assert!(inbox.observed_event(&event).await.unwrap());
        inbox.routed(&id).await.unwrap();
        assert!(inbox.observed_event(&event).await.unwrap());
        inbox
            .accept(&id, body.clone(), crate::api::now_ms())
            .await
            .unwrap();
        let mut changed = body;
        changed["events"][0]["content"]["body"] = json!("changed");
        assert!(
            inbox
                .accept(&id, changed, crate::api::now_ms())
                .await
                .is_err()
        );
        #[derive(diesel::QueryableByName)]
        struct Compact {
            #[diesel(sql_type=Jsonb)]
            body: Value,
        }
        let stored =
            sql_query("SELECT body FROM hagency_agent_v1.inbound_transactions WHERE id=$1")
                .bind::<Text, _>(&id)
                .get_result::<Compact>(&mut *inbox.db.lock().await)
                .await
                .unwrap();
        assert_eq!(stored.body, json!({"events":[]}));
        assert!(!inbox.pending().await.unwrap().iter().any(|t| t.id == id));
        let impostor_id = crate::secret_token();
        let impostor_event = format!("${}", crate::secret_token());
        inbox.accept(&impostor_id, json!({"events":[{"type":"m.room.message","room_id":room,"sender":"@xhagencyxservice:example.test","event_id":impostor_event,"content":{"msgtype":"m.notice","body":"Hagency service readiness probe"}}]}), crate::api::now_ms()).await.unwrap();
        inbox.routed(&impostor_id).await.unwrap();
        assert!(!inbox.observed_event(&impostor_event).await.unwrap());
    }
    #[test]
    fn hashes_semantic_json_consistently() {
        let a: Value = serde_json::from_str(r#"{"events":[],"nested":{"x":1,"y":2}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"nested":{"y":2,"x":1},"events":[]}"#).unwrap();
        assert_eq!(
            serde_json::to_vec(&canonical(&a)).unwrap(),
            serde_json::to_vec(&canonical(&b)).unwrap()
        );
    }
}
