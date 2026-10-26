use crate::{
    Error, Result,
    api::now_ms,
    appservice::Inbox,
    gateway::Gateway,
    matrix_client::MatrixClient,
    transport::{RoutedEvent, TransportStore},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The durable context root fixes reply shape for the lifetime of the intent.
/// Only newly routed top-level owner DMs store their exact Room ID here; old
/// event-root intents retain their original thread content and transaction ID.
fn reply_content(owner_direct: bool, room: &str, root: &str, body: &str) -> Value {
    let mut content = json!({"msgtype":"m.text","body":body});
    if !owner_direct || root != room {
        content["m.relates_to"] = json!({"rel_type":"m.thread","event_id":root,
            "is_falling_back":true,"m.in_reply_to":{"event_id":root}});
    }
    content
}

fn pause_notice_content(owner_direct: bool, room: &str, root: &str) -> Value {
    let mut content = reply_content(
        owner_direct,
        room,
        root,
        crate::transport::pause_notices::BODY,
    );
    // Matrix bots conventionally ignore notices, preventing automated reply loops.
    content["msgtype"] = json!("m.notice");
    content
}

fn request_notice_content(
    owner_direct: bool,
    room: &str,
    root: &str,
    event: &str,
    body: &str,
) -> Value {
    let mut content = reply_content(owner_direct, room, root, body);
    if let Some(relation) = content
        .get_mut("m.relates_to")
        .and_then(Value::as_object_mut)
    {
        relation.remove("m.in_reply_to");
        relation.insert("is_falling_back".into(), json!(false));
    }
    content["org.hagency.request_event_id"] = json!(event);
    content["msgtype"] = json!("m.notice");
    content
}

/// Only canonical plaintext text/media events enter the model queue. In particular,
/// HTML-looking mentions, edits, and encrypted bodies do not create prompts.
fn parse_event(raw: &Value) -> Option<RoutedEvent> {
    if raw["type"] != "m.room.message"
        || raw.get("state_key").is_some()
        || !matches!(
            raw["content"]["msgtype"].as_str(),
            Some("m.text" | "m.file" | "m.image" | "m.audio" | "m.video")
        )
    {
        return None;
    }
    let c = &raw["content"];
    if c["msgtype"] != "m.text" && crate::attachments::Attachment::from_content(c).is_err() {
        return None;
    }
    if c["m.relates_to"]["rel_type"] == "m.replace" {
        return None;
    }
    let event_id = raw["event_id"].as_str()?;
    let room = raw["room_id"].as_str()?;
    let sender = raw["sender"].as_str()?;
    let body = c["body"].as_str()?;
    if !event_id.starts_with('$')
        || !room.starts_with('!')
        || !sender.starts_with('@')
        || event_id.len() > 255
        || room.len() > 255
        || sender.len() > 255
        || body.is_empty()
        || body.len() > 64 * 1024
    {
        return None;
    }
    let mentioned_mxids: BTreeSet<String> = c["m.mentions"]["user_ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let thread_root = if c["m.relates_to"]["rel_type"] == "m.thread" {
        Some(c["m.relates_to"]["event_id"].as_str()?.to_owned())
    } else {
        None
    };
    Some(RoutedEvent {
        event_id: event_id.into(),
        room_id: room.into(),
        sender_mxid: sender.into(),
        body: body.into(),
        mentioned_mxids,
        thread_root,
        encrypted: false,
        is_edit: false,
    })
}
async fn route(inbox: &Inbox, t: &TransportStore, g: &Gateway, cursor: &mut String) -> Result<()> {
    let transactions = if cursor.is_empty() {
        inbox.pending().await?
    } else {
        t.routing_candidates_after(cursor, 10).await?
    };
    let mut unavailable_rooms = BTreeSet::new();
    for transaction in transactions {
        *cursor = transaction.id.clone();
        let mut retry = false;
        for raw in transaction.body["events"].as_array().into_iter().flatten() {
            let Some(event) = parse_event(raw) else {
                continue;
            };
            let scopes = match t.routing_scopes(&event.room_id).await {
                Ok(scopes) => scopes,
                Err(_) => {
                    retry = true;
                    continue;
                }
            };
            for scope in scopes {
                if unavailable_rooms.contains(&scope.room_id) {
                    retry = true;
                    continue;
                }
                let observed = tokio::time::timeout(
                    std::time::Duration::from_secs(8),
                    g.delivery(
                        &scope.room_id,
                        &scope.space_id,
                        &scope.owner_mxid,
                        &event.sender_mxid,
                        &scope.puppet_mxid,
                    ),
                )
                .await;
                let f = match observed {
                    Ok(Ok(f)) => f,
                    _ => {
                        // No fresh facts were obtained, so this is not proof of
                        // definitive authorization denial. Retain routing intent.
                        unavailable_rooms.insert(scope.room_id.clone());
                        retry = true;
                        continue;
                    }
                };
                match t
                    .ingest_routed_attachment_received(
                        &scope.binding_id,
                        event.clone(),
                        if raw["content"]["msgtype"] == "m.text" {
                            None
                        } else {
                            Some(crate::attachments::Attachment::from_content(
                                &raw["content"],
                            )?)
                        },
                        &f,
                        transaction.received_at_ms,
                        now_ms(),
                    )
                    .await
                {
                    // A permanently rejected message cannot block subsequent
                    // transactions. Storage/overload failures retain the intent.
                    Ok(_) => (),
                    Err(e @ (Error::Unauthorized(_) | Error::Invalid(_) | Error::Conflict(_))) => {
                        inbox
                            .rejected(
                                &transaction.id,
                                &event.event_id,
                                &scope.binding_id,
                                &e.to_string(),
                            )
                            .await?;
                    }
                    Err(_) => retry = true,
                }
            }
        }
        if retry {
            continue;
        }
        // A crash before this write causes an idempotent replay of routing only.
        inbox.routed(&transaction.id).await?;
    }
    Ok(())
}
async fn send(
    t: &TransportStore,
    g: &Gateway,
    m: &MatrixClient,
    cursor: &mut String,
) -> Result<()> {
    let mut unavailable_rooms = BTreeSet::new();
    for candidate in t.reply_candidates_after(cursor, 20, now_ms()).await? {
        *cursor = candidate.id.clone();
        let scope = match t.binding_scope(&candidate.binding_id).await {
            Ok(scope) => scope,
            Err(Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        if unavailable_rooms.contains(&scope.room_id) {
            continue;
        }
        let observed = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            g.delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.owner_mxid,
                &candidate.requester_mxid,
                &scope.puppet_mxid,
            ),
        )
        .await;
        let f = match observed {
            Ok(Ok(f)) => f,
            _ => {
                unavailable_rooms.insert(scope.room_id.clone());
                continue;
            }
        };
        let intent = match t.claim_reply(&candidate.id, &f, now_ms()).await {
            Ok(intent) => intent,
            Err(Error::Unauthorized(_) | Error::Conflict(_)) => {
                t.block_reply_delivery(&candidate.id, &f, now_ms()).await?;
                continue;
            }
            Err(e) => return Err(e),
        };
        let worker = intent
            .worker_token
            .as_deref()
            .ok_or(Error::Unavailable("missing_sender_claim"))?;
        let content = reply_content(
            scope.space_id.is_empty(),
            &intent.room_id,
            &intent.thread_root,
            &intent.body,
        );
        let outcome = m
            .send(
                &intent.puppet_mxid,
                &intent.room_id,
                &intent.matrix_txn_id,
                content,
            )
            .await;
        if matches!(outcome, Err(Error::Conflict("matrix_permission_missing"))) {
            t.confirm_reply_permission_denied(&intent.id, worker, now_ms())
                .await?;
        } else {
            t.confirm_reply(
                &intent.id,
                worker,
                outcome.as_ref().ok().map(String::as_str),
                now_ms(),
            )
            .await?;
        }
    }
    Ok(())
}
async fn send_pause_notices(
    t: &TransportStore,
    g: &Gateway,
    m: &MatrixClient,
    cursor: &mut String,
) -> Result<()> {
    let mut unavailable_rooms = BTreeSet::new();
    for candidate in t.pause_notice_candidates(cursor, now_ms()).await? {
        *cursor = candidate.id.clone();
        let scope = match t.pause_notice_scope(&candidate.binding_id).await {
            Ok(scope) => scope,
            Err(Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        if unavailable_rooms.contains(&scope.room_id) {
            continue;
        }
        let observed = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            g.delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.owner_mxid,
                &candidate.requester_mxid,
                &scope.puppet_mxid,
            ),
        )
        .await;
        let f = match observed {
            Ok(Ok(f)) => f,
            _ => {
                unavailable_rooms.insert(scope.room_id.clone());
                continue;
            }
        };
        let notice = match t.claim_pause_notice(&candidate.id, &f, now_ms()).await {
            Ok(n) => n,
            Err(Error::Conflict(_) | Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        let worker = notice
            .worker_token
            .as_deref()
            .ok_or(Error::Unavailable("missing_sender_claim"))?;
        let content = pause_notice_content(
            scope.space_id.is_empty(),
            &scope.room_id,
            &notice.thread_root,
        );
        let outcome = m
            .send(
                &scope.puppet_mxid,
                &scope.room_id,
                &notice.matrix_txn_id,
                content,
            )
            .await;
        t.confirm_pause_notice(&notice.id, worker, &outcome, now_ms())
            .await?;
    }
    Ok(())
}
async fn send_request_notices(
    t: &TransportStore,
    g: &Gateway,
    m: &MatrixClient,
    cursor: &mut String,
) -> Result<()> {
    let mut unavailable_rooms = BTreeSet::new();
    for candidate in t.request_notice_candidates(cursor, now_ms()).await? {
        *cursor = candidate.id.clone();
        let scope = match t.binding_scope(&candidate.binding_id).await {
            Ok(scope) => scope,
            Err(Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        if unavailable_rooms.contains(&scope.room_id) {
            continue;
        }
        let observed = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            g.delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.owner_mxid,
                &candidate.requester_mxid,
                &scope.puppet_mxid,
            ),
        )
        .await;
        let f = match observed {
            Ok(Ok(f)) => f,
            _ => {
                unavailable_rooms.insert(scope.room_id.clone());
                continue;
            }
        };
        let notice = match t.claim_request_notice(&candidate.id, &f, now_ms()).await {
            Ok(n) => n,
            Err(Error::Conflict(_) | Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        let worker = notice
            .worker_token
            .as_deref()
            .ok_or(Error::Unavailable("missing_sender_claim"))?;
        let mut content = request_notice_content(
            scope.space_id.is_empty(),
            &scope.room_id,
            &notice.thread_root,
            &notice.event_id,
            crate::transport::request_notices::body_for(&notice.kind, &notice.device_state),
        );
        content["org.hagency.request_notice"] = json!({"version":1,"kind":notice.kind,
            "requestEventId":notice.event_id,"deviceState":notice.device_state,"deviceName":notice.device_name,
            "observedAtMs":now_ms()});
        let outcome = m
            .send(
                &scope.puppet_mxid,
                &scope.room_id,
                &notice.matrix_txn_id,
                content,
            )
            .await;
        t.confirm_request_notice(&notice.id, worker, &outcome, now_ms())
            .await?;
    }
    Ok(())
}
async fn send_processing(
    t: &TransportStore,
    g: &Gateway,
    m: &MatrixClient,
    cursor: &mut String,
) -> Result<()> {
    let mut unavailable_rooms = BTreeSet::new();
    for candidate in t.processing_candidates(cursor, 20, now_ms()).await? {
        *cursor = candidate.id.clone();
        let scope = match t.binding_scope(&candidate.binding_id).await {
            Ok(scope) => scope,
            Err(Error::Unauthorized(_)) => continue,
            Err(e) => return Err(e),
        };
        if unavailable_rooms.contains(&scope.room_id) {
            continue;
        }
        let observed = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            g.delivery(
                &scope.room_id,
                &scope.space_id,
                &scope.owner_mxid,
                &candidate.requester_mxid,
                &scope.puppet_mxid,
            ),
        )
        .await;
        let f = match observed {
            Ok(Ok(f)) => f,
            _ => {
                unavailable_rooms.insert(scope.room_id.clone());
                continue;
            }
        };
        let intent = match t.claim_processing(&candidate.id, &f, now_ms()).await {
            Ok(intent) => intent,
            Err(Error::Unauthorized(_) | Error::Conflict(_)) => {
                t.block_processing(&candidate.id, &f, now_ms()).await?;
                continue;
            }
            Err(e) => return Err(e),
        };
        let worker = intent
            .worker_token
            .as_deref()
            .ok_or(Error::Unavailable("missing_sender_claim"))?;
        let outcome = m
            .send_processing(
                &intent.puppet_mxid,
                &intent.room_id,
                &intent.matrix_txn_id,
                intent.content.clone(),
            )
            .await;
        let denied = matches!(outcome, Err(Error::Conflict("matrix_permission_missing")));
        t.confirm_processing(
            &intent.id,
            worker,
            outcome.as_ref().ok().map(String::as_str),
            denied,
            now_ms(),
        )
        .await?;
    }
    Ok(())
}
pub(crate) fn start(
    inbox: Inbox,
    t: TransportStore,
    g: Gateway,
    m: MatrixClient,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut route_cursor = String::new();
        let mut reply_cursor = String::new();
        let mut processing_cursor = String::new();
        let mut pause_cursor = String::new();
        let mut request_cursor = String::new();
        // Each durable queue advances on its own clock. A slow Matrix lookup
        // or retry in one queue must not stall intake or another queue's replies.
        // These futures are owned by the parent task, so aborting it drops all
        // in-flight work together rather than leaking detached workers.
        macro_rules! worker_loop {
            ($pass:expr) => {
                async {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        interval.tick().await;
                        let _ = $pass.await;
                    }
                }
            };
        }
        tokio::join!(
            worker_loop!(t.invalidate_stale_dispatches(now_ms())),
            worker_loop!(route(&inbox, &t, &g, &mut route_cursor)),
            worker_loop!(send(&t, &g, &m, &mut reply_cursor)),
            worker_loop!(send_processing(&t, &g, &m, &mut processing_cursor)),
            worker_loop!(send_pause_notices(&t, &g, &m, &mut pause_cursor)),
            worker_loop!(send_request_notices(&t, &g, &m, &mut request_cursor))
        );
    })
}
pub(crate) fn start_cleanup(
    domain: crate::domain::DomainStore,
    g: Gateway,
    m: MatrixClient,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut cursor = String::new();
        let mut cleanup_cursor = (String::new(), String::new());
        let mut identity_cursor = String::new();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            // A cancelled transaction can replace the DomainStore connection
            // and release its session lock. Never cache leadership across ticks.
            if !domain.try_membership_controller().await.unwrap_or(false) {
                continue;
            }
            if let Ok(agents) = domain
                .identity_provisioning_candidates(&identity_cursor, 20)
                .await
            {
                for agent in agents {
                    identity_cursor = agent.id.clone();
                    if m.provision_identity(&agent).await.is_ok() {
                        let _ = domain
                            .confirm_identity_provisioned(&agent.id, agent.generation)
                            .await;
                    }
                }
            }
            // Fair full cursor sweep includes already-left bindings. Matrix can
            // finish a timed-out join after retirement was locally confirmed.
            if let Ok(scopes) = domain.reconciliation_scopes(&cursor, 20).await {
                if scopes.is_empty() {
                    cursor.clear();
                }
                for scope in scopes {
                    let (Some(binding), Some(room), Some(space), Some(generation)) = (
                        scope.binding_id.as_deref(),
                        scope.room_id.as_deref(),
                        scope.space_id.as_deref(),
                        scope.binding_generation,
                    ) else {
                        continue;
                    };
                    cursor = binding.to_owned();
                    if matches!(
                        scope.binding_state.as_deref(),
                        Some("active" | "joining" | "suspended")
                    ) {
                        let Ok(Ok(facts)) = tokio::time::timeout(
                            std::time::Duration::from_secs(8),
                            g.room(room, space, &scope.owner_mxid, Some(&scope.puppet_mxid)),
                        )
                        .await
                        else {
                            continue;
                        };
                        // Revocation precedes Matrix departure and survives logout
                        // and transient failures. Cleanup re-reads its generation.
                        if domain
                            .retire_membership_loss_trusted(binding, generation, &facts, now_ms())
                            .await
                            .is_err()
                        {
                            continue;
                        }
                        if scope.binding_state.as_deref() != Some("joining") {
                            continue;
                        }
                    }
                    if scope.binding_state.as_deref() != Some("joining") {
                        // A fresh generation check keeps a stale terminal sweep
                        // from issuing leave after the owner requested rebind.
                        if !domain
                            .departure_desired_trusted(binding, generation)
                            .await
                            .unwrap_or(false)
                        {
                            continue;
                        }
                        let Ok(facts) = g
                            .room(room, space, &scope.owner_mxid, Some(&scope.puppet_mxid))
                            .await
                        else {
                            continue;
                        };
                        // Recheck after the Matrix GET; an owner may have rebound
                        // a terminal scope while that observation was in flight.
                        if !domain
                            .departure_desired_trusted(binding, generation)
                            .await
                            .unwrap_or(false)
                        {
                            continue;
                        }
                        if facts.puppet_in_room && m.leave(&scope.puppet_mxid, room).await.is_err()
                        {
                            continue;
                        }
                        if let Ok(observed) = g
                            .room(room, space, &scope.owner_mxid, Some(&scope.puppet_mxid))
                            .await
                        {
                            let _ = domain
                                .confirm_left_trusted(binding, generation, &observed, now_ms())
                                .await;
                        }
                        continue;
                    }
                    if g.service_invited(room).await.unwrap_or(false) {
                        let _ = m.join_invited_service(room).await;
                    }
                    let Ok(facts) = g
                        .room(room, space, &scope.owner_mxid, Some(&scope.puppet_mxid))
                        .await
                    else {
                        continue;
                    };
                    let Ok(agent) = domain
                        .verify_provisioning_trusted(binding, generation, &facts, false, now_ms())
                        .await
                    else {
                        continue;
                    };
                    if !facts.puppet_in_room {
                        let Ok(invited) = g.puppet_invited(room, &scope.puppet_mxid).await else {
                            continue;
                        };
                        if m.provision(&agent, room, !invited).await.is_err() {
                            continue;
                        }
                    }
                    if let Ok(observed) = g
                        .room(room, space, &scope.owner_mxid, Some(&scope.puppet_mxid))
                        .await
                    {
                        let _ = domain
                            .verify_provisioning_trusted(
                                binding,
                                generation,
                                &observed,
                                true,
                                now_ms(),
                            )
                            .await;
                    }
                }
            }
            let Ok(scopes) = domain
                .cleanup_scopes_after(&cleanup_cursor.0, &cleanup_cursor.1, 20)
                .await
            else {
                continue;
            };
            if scopes.is_empty() {
                cleanup_cursor = (String::new(), String::new());
            }
            for scope in scopes {
                cleanup_cursor = (
                    scope.agent_id.clone(),
                    scope.binding_id.clone().unwrap_or_default(),
                );
                let Some(binding) = scope.binding_id.as_deref() else {
                    let _ = domain
                        .confirm_retired_trusted(&scope.agent_id, scope.agent_generation, now_ms())
                        .await;
                    continue;
                };
                let (Some(room), Some(space), Some(generation)) = (
                    scope.room_id.as_deref(),
                    scope.space_id.as_deref(),
                    scope.binding_generation,
                ) else {
                    continue;
                };
                let Ok(mut facts) = g
                    .room(room, space, &scope.owner_mxid, Some(&scope.puppet_mxid))
                    .await
                else {
                    continue;
                };
                if !domain
                    .departure_desired_trusted(binding, generation)
                    .await
                    .unwrap_or(false)
                {
                    continue;
                }
                if facts.puppet_in_room {
                    if m.leave(&scope.puppet_mxid, room).await.is_err() {
                        continue;
                    }
                    let Ok(observed) = g
                        .room(room, space, &scope.owner_mxid, Some(&scope.puppet_mxid))
                        .await
                    else {
                        continue;
                    };
                    facts = observed;
                }
                let _ = domain
                    .confirm_left_trusted(binding, generation, &facts, now_ms())
                    .await;
            }
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_reply_root_preserves_flat_dm_and_historical_threads() {
        let flat = reply_content(true, "!dm:x", "!dm:x", "reply");
        assert_eq!(flat, json!({"msgtype":"m.text","body":"reply"}));
        let legacy = reply_content(true, "!dm:x", "$original", "reply");
        assert_eq!(legacy["m.relates_to"]["rel_type"], "m.thread");
        assert_eq!(legacy["m.relates_to"]["event_id"], "$original");
        assert_eq!(
            legacy,
            reply_content(false, "!project:x", "$original", "reply")
        );
        assert_eq!(legacy, reply_content(true, "!dm:x", "$original", "reply"));
    }
    #[test]
    fn paused_service_feedback_is_a_notice_and_never_routes_back_to_inference() {
        let flat = pause_notice_content(true, "!dm:x", "!dm:x");
        assert_eq!(flat["msgtype"], "m.notice");
        assert!(flat.get("m.relates_to").is_none());
        assert!(flat.get("m.mentions").is_none());
        let threaded = pause_notice_content(false, "!room:x", "$request");
        assert_eq!(threaded["m.relates_to"]["event_id"], "$request");
        let raw = json!({"type":"m.room.message","event_id":"$notice","room_id":"!room:x","sender":"@external-bot:x","content":threaded});
        assert!(parse_event(&raw).is_none());
    }
    #[test]
    fn request_feedback_replies_to_exact_event_without_triggering_other_agents() {
        for owner_direct in [false, true] {
            let content = request_notice_content(
                owner_direct,
                "!room:x",
                if owner_direct { "!room:x" } else { "$thread" },
                "$request",
                crate::transport::request_notices::body("failed"),
            );
            assert_eq!(content["msgtype"], "m.notice");
            assert_eq!(content["org.hagency.request_event_id"], "$request");
            assert!(content["m.relates_to"].get("m.in_reply_to").is_none());
            if !owner_direct {
                assert_eq!(content["m.relates_to"]["event_id"], "$thread");
            }
            assert!(parse_event(&json!({"type":"m.room.message","event_id":"$notice","room_id":"!room:x","sender":"@bot:x","content":content})).is_none());
        }
    }
    #[test]
    fn canonical_media_retains_caption_mentions_and_original_thread() {
        let mut event = json!({"type":"m.room.message","event_id":"$media","room_id":"!room:x","sender":"@u:x","content":{"msgtype":"m.file","body":"Analyze this\nwith care","url":"mxc://media.test/file","m.mentions":{"user_ids":["@bot:x"]},"m.relates_to":{"rel_type":"m.thread","event_id":"$root"}}});
        let parsed = parse_event(&event).unwrap();
        assert_eq!(parsed.body, "Analyze this\nwith care");
        assert!(parsed.mentioned_mxids.contains("@bot:x"));
        assert_eq!(parsed.thread_root.as_deref(), Some("$root"));
        event["content"]["url"] = json!("https://arbitrary.test/file");
        assert!(parse_event(&event).is_none());
        event["content"]["url"] = json!("mxc://media.test/file");
        event["content"]["m.relates_to"]["rel_type"] = json!("m.replace");
        assert!(parse_event(&event).is_none());
        event["type"] = json!("m.room.encrypted");
        assert!(parse_event(&event).is_none());
    }
    #[test]
    fn canonical_mentions_and_threads_only() {
        let raw = json!({"type":"m.room.message","event_id":"$e","room_id":"!r:x","sender":"@u:x",
            "content":{"msgtype":"m.text","body":"hello","formatted_body":"<a href=\"https://matrix.to/#/@bot:x\">bot</a>"}});
        assert!(parse_event(&raw).unwrap().mentioned_mxids.is_empty());
        let mut mention = raw.clone();
        mention["content"]["m.mentions"] = json!({"user_ids":["@bot:x"]});
        mention["content"]["m.relates_to"] = json!({"rel_type":"m.thread","event_id":"$root"});
        let parsed = parse_event(&mention).unwrap();
        assert!(parsed.mentioned_mxids.contains("@bot:x"));
        assert_eq!(parsed.thread_root.as_deref(), Some("$root"));
        mention["content"]["m.relates_to"]["rel_type"] = json!("m.replace");
        assert!(parse_event(&mention).is_none());
        let mut encrypted = raw;
        encrypted["type"] = json!("m.room.encrypted");
        assert!(parse_event(&encrypted).is_none());
    }
}

#[cfg(test)]
#[path = "worker_fairness_tests.rs"]
mod fairness_tests;
