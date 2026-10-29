//! Device-only operations. Membership observations always come from the gateway.
use crate::{
    Error, Result,
    api::now_ms,
    gateway::Gateway,
    store::Principal,
    transport::{DeliveryFacts, ExecutionHistorySnapshot, LeaseRef, SubmitReply, TransportStore},
};
use salvo::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};

async fn body<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T> {
    req.parse_json()
        .await
        .map_err(|_| Error::Invalid("invalid_arguments"))
}
async fn facts(t: &TransportStore, g: &Gateway, p: &Principal, id: &str) -> Result<DeliveryFacts> {
    let header = t.owned_event_header(p, id, now_ms()).await?;
    let scope = t.binding_scope(&header.binding_id).await?;
    g.delivery(
        &scope.room_id,
        &scope.space_id,
        &scope.owner_mxid,
        &header.requester_mxid,
        &scope.puppet_mxid,
    )
    .await
}
pub(crate) async fn call(
    t: &TransportStore,
    g: &Gateway,
    p: &Principal,
    req: &mut Request,
) -> Result<Value> {
    if req.method() != salvo::http::Method::POST {
        return Err(Error::Invalid("unsupported_operation"));
    }
    let path = req.uri().path().to_owned();
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Acquire {
        agent_id: String,
        ttl_ms: i64,
        takeover: bool,
        history_snapshot: ExecutionHistorySnapshot,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct History {
        agent_id: String,
        cursor: Option<String>,
        snapshot: Option<ExecutionHistorySnapshot>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Renew {
        lease: LeaseRef,
        ttl_ms: i64,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Ref {
        lease: LeaseRef,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Poll {
        lease: LeaseRef,
        binding_id: String,
        limit: usize,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Ack {
        lease: LeaseRef,
        dispatch_id: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Start {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Finish {
        lease: LeaseRef,
        dispatch_id: String,
        execution_id: String,
        outcome: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Reply {
        lease: LeaseRef,
        reply: SubmitReply,
    }
    match path.as_str() {
        "/hagency/api/v1/execution/history" => {
            let b: History = body(req).await?;
            Ok(
                json!({"history":t.execution_history(p,&b.agent_id,b.cursor.as_deref(),b.snapshot.as_ref(),now_ms()).await?}),
            )
        }
        "/hagency/api/v1/execution/leases/acquire" => {
            let b: Acquire = body(req).await?;
            Ok(
                json!({"lease":t.acquire_lease(p,&b.agent_id,b.ttl_ms,b.takeover,&b.history_snapshot,now_ms()).await?}),
            )
        }
        "/hagency/api/v1/execution/leases/renew" => {
            let b: Renew = body(req).await?;
            Ok(json!({"lease":t.renew_lease(p,&b.lease,b.ttl_ms,now_ms()).await?}))
        }
        "/hagency/api/v1/execution/leases/release" => {
            let b: Ref = body(req).await?;
            t.release_lease(p, &b.lease, now_ms()).await?;
            Ok(json!({"released":true}))
        }
        "/hagency/api/v1/execution/events/poll" => {
            let b: Poll = body(req).await?;
            let mut events = Vec::new();
            for header in t
                .event_candidates_for_binding(p, &b.lease, &b.binding_id, b.limit, now_ms())
                .await?
            {
                let f = match facts(t, g, p, &header.id).await {
                    Ok(f) => f,
                    Err(Error::Unauthorized(_)) => continue,
                    Err(e) => return Err(e),
                };
                match t.claim_event(p, &b.lease, &header.id, &f, now_ms()).await {
                    Ok(event) => events.push(event),
                    Err(Error::Unauthorized(_) | Error::Conflict(_)) => {
                        t.reject_event_delivery(&header.id, &f, now_ms()).await?;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            // Scope revocation skips one event; device/session expiry must still
            // fail the entire response, including bodies collected earlier.
            t.event_candidates(p, &b.lease, 1, now_ms()).await?;
            Ok(json!({"events":events}))
        }
        "/hagency/api/v1/execution/events/ack" => {
            let b: Ack = body(req).await?;
            t.acknowledge(p, &b.lease, &b.dispatch_id, now_ms()).await?;
            Ok(json!({"acknowledged":true}))
        }
        "/hagency/api/v1/execution/events/start" => {
            let b: Start = body(req).await?;
            let f = facts(t, g, p, &b.dispatch_id).await?;
            Ok(
                json!({"execution":t.start_execution(p,&b.lease,&b.dispatch_id,&b.execution_id,&f,now_ms()).await?}),
            )
        }
        "/hagency/api/v1/execution/events/processing" => {
            let b: Start = body(req).await?;
            let f = facts(t, g, p, &b.dispatch_id).await?;
            Ok(
                json!({"processing":t.mark_processing(p,&b.lease,&b.dispatch_id,&b.execution_id,&f,now_ms()).await?}),
            )
        }
        "/hagency/api/v1/execution/events/authorize-tool" => {
            let b: Start = body(req).await?;
            let f = facts(t, g, p, &b.dispatch_id).await?;
            Ok(
                json!({"dispatch":t.authorize_tool_execution(p,&b.lease,&b.dispatch_id,&b.execution_id,&f,now_ms()).await?}),
            )
        }
        "/hagency/api/v1/execution/events/finish" => {
            let b: Finish = body(req).await?;
            t.finish_without_reply(
                p,
                &b.lease,
                &b.dispatch_id,
                &b.execution_id,
                &b.outcome,
                now_ms(),
            )
            .await?;
            Ok(json!({"finished":true}))
        }
        "/hagency/api/v1/execution/replies" => {
            let b: Reply = body(req).await?;
            let f = facts(t, g, p, &b.reply.dispatch_id).await?;
            Ok(json!({"reply":t.submit_reply(p,&b.lease,b.reply,&f,now_ms()).await?}))
        }
        "/hagency/api/v1/execution/replies/reconcile-known" => {
            let b: Reply = body(req).await?;
            let f = facts(t, g, p, &b.reply.dispatch_id).await?;
            Ok(json!({"reply":t.reconcile_known_reply(p,&b.lease,b.reply,&f,now_ms()).await?}))
        }
        "/hagency/api/v1/execution/replies/encrypted" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Encrypted {
                lease: LeaseRef,
                reply: crate::transport::EncryptedReply,
            }
            let b: Encrypted = body(req).await?;
            let f = facts(t, g, p, &b.reply.dispatch_id).await?;
            Ok(json!({"reply":t.record_encrypted_reply(p,&b.lease,b.reply,&f,now_ms()).await?}))
        }
        _ => Err(Error::Invalid("unsupported_operation")),
    }
}
