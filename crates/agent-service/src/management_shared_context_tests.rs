use super::*;
use crate::Error;
use crate::management_shared_context_types::*;

pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    db.batch_execute(include_str!("management_pool_agents_remove_test.sql")).await.unwrap();
    db.batch_execute(include_str!("management_project_manager_remove_test.sql")).await.unwrap();
    db.batch_execute(include_str!("management_agent_budget_remove_test.sql")).await.unwrap();
    db.batch_execute("DROP TRIGGER require_shared_context_start ON hagency_agent_v1.management_shared_work;DROP INDEX hagency_agent_v1.shared_work_context_scope;DROP TABLE hagency_agent_v1.management_shared_context_starts,hagency_agent_v1.management_shared_contexts,hagency_agent_v1.management_shared_context_sequences,hagency_agent_v1.management_shared_reply_deliveries,hagency_agent_v1.management_shared_input_modes;DROP FUNCTION hagency_agent_v1.require_shared_context_start(),hagency_agent_v1.validate_shared_context(),hagency_agent_v1.retain_shared_context();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=17;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=17)").await.unwrap();
}
fn read_input(w: &SharedWorkItem) -> SharedWorkEvidenceInput {
    SharedWorkEvidenceInput {
        work_digest: allocation_digest(w).unwrap(),
        runtime_command_digest: w.runtime_command_digest.clone(),
    }
}
fn input(a: &SharedActivityLease, w: &SharedWorkItem) -> SharedWorkInput {
    SharedWorkInput {
        authority: a.authority.clone(),
        work_id: w.id.clone(),
        work_digest: allocation_digest(w).unwrap(),
    }
}
fn context_input(a: &SharedActivityLease, w: &SharedWorkItem) -> SharedContextInput {
    SharedContextInput {
        authority: a.authority.clone(),
        work_digest: allocation_digest(w).unwrap(),
    }
}
fn start_input(
    a: &SharedActivityLease,
    w: &SharedWorkItem,
    c: &SharedContextRecord,
) -> SharedContextStartInput {
    SharedContextStartInput {
        authority: a.authority.clone(),
        work_digest: allocation_digest(w).unwrap(),
        context_digest: c.context_digest.clone(),
        input_digest: c.input_digest.clone(),
    }
}
async fn queue(
    f: &Fixture,
    r: &ManagedAgentRequest,
    m: &MatrixFixture,
    a: &SharedActivityLease,
    id: &str,
    root: &str,
    body: &str,
) -> SharedWorkItem {
    let mut e = work::message(f, r, id).await;
    e.thread_root = Some(root.into());
    e.body = body.into();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("SELECT pg_sleep(0.002)").await.unwrap();
    let received =
        sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS count")
            .get_result::<Count>(&mut db)
            .await
            .unwrap()
            .count;
    f.domain
        .route_shared_event(&f.policy, &e, received, &m.gateway)
        .await
        .unwrap();
    let w = f
        .domain
        .poll_shared_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(w.event_id, format!("${id}"));
    w
}
#[allow(clippy::too_many_arguments)]
async fn complete(
    f: &Fixture,
    r: &ManagedAgentRequest,
    m: &MatrixFixture,
    d: &SharedRuntimeDelivery,
    a: &SharedActivityLease,
    id: &str,
    root: &str,
    revision: i64,
    body: &str,
    usage: i64,
) -> SharedWorkItem {
    let w = queue(f, r, m, a, id, root, "Prior user request").await;
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(a, &w), &m.gateway)
        .await
        .unwrap();
    if f.policy.shared_thread_context() {
        let c = f
            .domain
            .freeze_shared_context(
                &f.owner.p,
                &f.policy,
                &w.id,
                context_input(a, &w),
                &m.gateway,
            )
            .await
            .unwrap();
        assert!(
            f.domain
                .start_shared_context_work(
                    &f.owner.p,
                    &f.policy,
                    &w.id,
                    start_input(a, &w, &c),
                    &m.gateway
                )
                .await
                .unwrap()
                .start
                .newly_started
        );
    } else {
        assert!(
            f.domain
                .start_shared_work(&f.owner.p, &f.policy, input(a, &w), &m.gateway)
                .await
                .unwrap()
                .newly_started
        );
    }
    let t = work::ticket(f, d, &w).await;
    let mut out = work::outcome(d, &w, &t, revision, "completed");
    out.receipt.body = Some(body.into());
    out.receipt.body_digest = Some(crate::hash(body));
    out.receipt.known_consumed_tokens = usage;
    out.receipt.retained_tokens = Some(usage);
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, out)
        .await
        .unwrap();
    w
}
async fn send(f: &Fixture, m: &MatrixFixture, s: &work::Sender) {
    f.domain
        .send_shared_replies(&f.policy, &m.gateway, &s.matrix, &mut String::new())
        .await
        .unwrap();
}

async fn formatted_followup(legacy: bool, large: bool) {
    let (mut f, r, _admin, _assignment, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let s = work::sender().await;
    let body = if large { "**<&>**\n".repeat(2500) } else { "## Prior answer\n\n**Keep the original Markdown.**".into() };
    let prior = complete(&f, &r, &m, &d, &a, "format_prior", "$format", 201, &body, 1).await;
    let mut content = crate::workers::reply_content(false, &prior.room_id, &prior.thread_root, &body);
    if legacy {
        // Seed exact pre-formatting-version bytes in this disposable database.
        // Production and migration paths never update an immutable sent reply.
        content.as_object_mut().unwrap().remove("format");
        content.as_object_mut().unwrap().remove("formatted_body");
        let bytes = serde_json::to_string(&content).unwrap();
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_shared_replies DISABLE TRIGGER USER").await.unwrap();
        sql_query("UPDATE hagency_agent_v1.management_shared_replies SET content=$2,content_digest=$3 WHERE work_id=$1")
            .bind::<Text,_>(&prior.id).bind::<Text,_>(&bytes).bind::<Text,_>(crate::hash(&bytes)).execute(&mut db).await.unwrap();
        db.batch_execute("ALTER TABLE hagency_agent_v1.management_shared_replies ENABLE TRIGGER USER").await.unwrap();
    }
    assert_eq!(content.get("formatted_body").is_some(), !legacy && !large);
    send(&f, &m, &s).await;
    let first = f.domain.observe_shared_reply(&f.owner.p, &prior.id, read_input(&prior)).await.unwrap().delivery.unwrap();
    assert_eq!(first.receipt.body, body);
    assert_eq!(first.receipt.formatted_body.as_deref(), content.get("formatted_body").and_then(Value::as_str));
    f.policy = f.policy.with_shared_thread_context(true);
    let next = queue(&f, &r, &m, &a, "format_next", "$format", "Compare with H100").await;
    f.domain.acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &next), &m.gateway).await.unwrap();
    let context = f.domain.freeze_shared_context(&f.owner.p, &f.policy, &next.id, context_input(&a, &next), &m.gateway).await.unwrap();
    assert_eq!(context.context.history.len(), 1);
    assert_eq!(context.context.history[0].delivery_digest, first.receipt_digest);
    assert_eq!(context.context.history[0].delivery.body, body);
    let again = f.domain.observe_shared_reply(&f.owner.p, &prior.id, read_input(&prior)).await.unwrap().delivery.unwrap();
    assert_eq!(allocation_encoding(&first).unwrap(), allocation_encoding(&again).unwrap());
    assert!(f.domain.start_shared_context_work(&f.owner.p, &f.policy, &next.id, start_input(&a, &next, &context), &m.gateway).await.unwrap().start.newly_started);
    assert_eq!(s.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_historical_plain_markdown_followup_preserves_delivery() {
    formatted_followup(true, false).await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_rich_markdown_followup_preserves_delivery() {
    formatted_followup(false, false).await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_large_markdown_fallback_followup_preserves_delivery() {
    formatted_followup(false, true).await;
}

#[test]
fn shared_context_canonical_renderer_delivery_and_history_fixture() {
    let v: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-thread-context-v1.json"
    ))
    .unwrap();
    let w: SharedWorkItem = serde_json::from_value(v["work"].clone()).unwrap();
    let c: SharedThreadContext = serde_json::from_value(v["context"].clone()).unwrap();
    let d: SharedReplyDelivery = serde_json::from_value(v["delivery"].clone()).unwrap();
    d.validate().unwrap();
    assert_eq!(allocation_digest(&d).unwrap(), v["deliveryDigest"]);
    assert_eq!(allocation_digest(&c).unwrap(), v["contextDigest"]);
    let rendered = render_context(&c, &w).unwrap();
    assert_eq!(rendered, v["renderedInput"]);
    assert_eq!(crate::hash(&rendered), v["inputDigest"]);
    assert_eq!(rendered.len(), v["inputBytes"].as_u64().unwrap() as usize);
    for field in ["threadRoot", "assignmentId", "workId", "eventId"] {
        let mut value = v["context"].clone();
        value[field] = json!("substituted");
        let bad: SharedThreadContext = serde_json::from_value(value).unwrap();
        assert!(render_context(&bad, &w).is_err());
    }
    let mut bad = c.clone();
    bad.history[0].received_at_ms = w.created_at_ms + 1;
    assert!(render_context(&bad, &w).is_err());
    let mut bad = c.clone();
    bad.history[0].delivery.body.push('x');
    assert!(render_context(&bad, &w).is_err());
    let mut bad = c;
    bad.history.push(bad.history[0].clone());
    assert!(render_context(&bad, &w).is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_exact_delivery_thread_scope_and_frozen_replay() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let s = work::sender().await;
    let one = complete(
        &f,
        &r,
        &m,
        &d,
        &a,
        "ctx_one",
        "$same",
        201,
        "First delivered answer",
        1,
    )
    .await;
    send(&f, &m, &s).await;
    let other = complete(
        &f,
        &r,
        &m,
        &d,
        &a,
        "ctx_other",
        "$other",
        202,
        "Other thread secret",
        1,
    )
    .await;
    send(&f, &m, &s).await;
    let late = complete(
        &f,
        &r,
        &m,
        &d,
        &a,
        "ctx_late",
        "$same",
        203,
        "Not yet delivered",
        1,
    )
    .await;
    f.policy = f.policy.with_shared_thread_context(true);
    let w = queue(&f, &r, &m, &a, "ctx_current", "$same", "What did you say?").await;
    let mode = f
        .domain
        .shared_work_input_mode(&f.owner.p, &w.id, read_input(&w))
        .await
        .unwrap();
    assert_eq!(mode.mode, "thread_context_v1");
    assert!(mode.context_digest.is_none());
    let c = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(c.context.history.len(), 1);
    assert_eq!(c.context.history[0].work_id, one.id);
    assert_eq!(c.context.history[0].delivery.body, "First delivered answer");
    assert_ne!(c.context.history[0].work_id, other.id);
    assert_ne!(c.context.history[0].work_id, late.id);
    send(&f, &m, &s).await;
    let again = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(
        allocation_digest(&c).unwrap(),
        allocation_digest(&again).unwrap()
    );
    let observed = f
        .domain
        .observe_shared_reply(&f.owner.p, &one.id, read_input(&one))
        .await
        .unwrap()
        .delivery
        .unwrap();
    assert_eq!(
        observed.receipt_digest,
        c.context.history[0].delivery_digest
    );
    let count = s.calls.lock().unwrap().len();
    let _ = f
        .domain
        .observe_shared_reply(&f.owner.p, &one.id, read_input(&one))
        .await
        .unwrap();
    assert_eq!(s.calls.lock().unwrap().len(), count);
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("UPDATE hagency_agent_v1.management_shared_contexts SET input_digest=repeat('f',64) WHERE work_id=$1").bind::<Text,_>(&w.id).execute(&mut db).await.is_err());
    assert!(
        sql_query(
            "DELETE FROM hagency_agent_v1.management_shared_reply_deliveries WHERE work_id=$1"
        )
        .bind::<Text, _>(&one.id)
        .execute(&mut db)
        .await
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_start_exact_input_http_replay_disabled_recovery_and_stop() {
    let (mut f, r, admin, _assignment, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_thread_context(true);
    let (d, a) = qualify(&f).await;
    let w = queue(&f, &r, &m, &a, "ctx_start", "$start", "Current request").await;
    let service = svc(&f, m.gateway.clone());
    let base = format!("shared-execution/work/{}", w.id);
    let mode = post(
        &service,
        &format!("{base}/input-mode"),
        &f.owner.token,
        json!(read_input(&w)),
        StatusCode::OK,
    )
    .await;
    assert_eq!(mode["mode"], "thread_context_v1");
    for actor in [&f.outsider, &admin, &f.admin] {
        post(
            &service,
            &format!("{base}/input-mode"),
            &actor.token,
            json!(read_input(&w)),
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    post(
        &service,
        "shared-execution/work/start",
        &f.owner.token,
        json!(input(&a, &w)),
        StatusCode::CONFLICT,
    )
    .await;
    let mut response = post(
        &service,
        &format!("{base}/context"),
        &f.owner.token,
        json!(context_input(&a, &w)),
        StatusCode::OK,
    )
    .await;
    response.as_object_mut().unwrap().remove("observedAtMs");
    let c: SharedContextRecord = serde_json::from_value(response).unwrap();
    let mut bad = start_input(&a, &w, &c);
    bad.input_digest = "f".repeat(64);
    post(
        &service,
        &format!("{base}/context/start"),
        &f.owner.token,
        json!(bad),
        StatusCode::CONFLICT,
    )
    .await;
    let first = post(
        &service,
        &format!("{base}/context/start"),
        &f.owner.token,
        json!(start_input(&a, &w, &c)),
        StatusCode::OK,
    )
    .await;
    assert_eq!(first["start"]["newlyStarted"], true);
    let again = post(
        &service,
        &format!("{base}/context/start"),
        &f.owner.token,
        json!(start_input(&a, &w, &c)),
        StatusCode::OK,
    )
    .await;
    assert_eq!(again["start"]["newlyStarted"], false);
    assert_eq!(first["start"]["work"], again["start"]["work"]);
    attempts::stop(&f, &d, 301).await;
    assert!(
        f.domain
            .freeze_shared_context(
                &f.owner.p,
                &f.policy,
                &w.id,
                context_input(&a, &w),
                &m.gateway
            )
            .await
            .is_err()
    );
    f.policy = f
        .policy
        .with_shared_thread_context(false)
        .with_shared_execution(false);
    assert!(
        f.domain
            .start_shared_context_work(
                &f.owner.p,
                &f.policy,
                &w.id,
                start_input(&a, &w, &c),
                &m.gateway
            )
            .await
            .is_err()
    );
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let recovered = reopened
        .recover_shared_context(
            &f.owner.p,
            &w.id,
            SharedContextRecoverInput {
                work_digest: allocation_digest(&w).unwrap(),
                runtime_command_digest: w.runtime_command_digest.clone(),
                context_digest: c.context_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        allocation_digest(&recovered).unwrap(),
        allocation_digest(&c).unwrap()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_wrong_and_replaced_generation_cannot_freeze_or_start() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_thread_context(true);
    let (_d, a) = qualify(&f).await;
    let w = queue(&f, &r, &m, &a, "ctx_auth", "$auth", "Current request").await;
    let mut bad = f
        .auth
        .authenticate(&f.owner.token, now_ms(), true)
        .await
        .unwrap();
    bad.device_generation = Some(bad.device_generation.unwrap() + 1);
    assert!(
        f.domain
            .freeze_shared_context(&bad, &f.policy, &w.id, context_input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
    let mut bad = f
        .auth
        .authenticate(&f.owner.token, now_ms(), true)
        .await
        .unwrap();
    bad.device_id = Some("other-device".into());
    assert!(
        f.domain
            .shared_work_input_mode(&bad, &w.id, read_input(&w))
            .await
            .is_err()
    );
    let mut wrong = read_input(&w);
    wrong.runtime_command_digest = "f".repeat(64);
    assert!(
        f.domain
            .shared_work_input_mode(&f.owner.p, &w.id, wrong)
            .await
            .is_err()
    );
    let c = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway,
        )
        .await
        .unwrap();
    let token = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Replacement".into(),
            },
            now_ms(),
        )
        .await
        .unwrap()
        .token;
    let current = f.auth.authenticate(&token, now_ms(), true).await.unwrap();
    assert!(current.device_generation.unwrap() > f.owner.p.device_generation.unwrap());
    for principal in [&f.owner.p, &current] {
        assert!(
            f.domain
                .freeze_shared_context(
                    principal,
                    &f.policy,
                    &w.id,
                    context_input(&a, &w),
                    &m.gateway
                )
                .await
                .is_err()
        );
        assert!(
            f.domain
                .start_shared_context_work(
                    principal,
                    &f.policy,
                    &w.id,
                    start_input(&a, &w, &c),
                    &m.gateway
                )
                .await
                .is_err()
        );
    }
    let recovered = f
        .domain
        .recover_shared_context(
            &current,
            &w.id,
            SharedContextRecoverInput {
                work_digest: allocation_digest(&w).unwrap(),
                runtime_command_digest: w.runtime_command_digest.clone(),
                context_digest: c.context_digest.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        allocation_digest(&recovered).unwrap(),
        allocation_digest(&c).unwrap()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_bounds_keep_newest_whole_pairs_without_silent_truncation() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let s = work::sender().await;
    for i in 0..10 {
        complete(
            &f,
            &r,
            &m,
            &d,
            &a,
            &format!("ctx_bound_{i}"),
            "$bounds",
            201 + i,
            &format!("Reply {i}"),
            1,
        )
        .await;
        send(&f, &m, &s).await;
    }
    f.policy = f.policy.with_shared_thread_context(true);
    let w = queue(&f, &r, &m, &a, "ctx_bounds", "$bounds", "Recall").await;
    let c = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(c.context.history.len(), 8);
    assert!(c.context.omitted_earlier);
    assert_eq!(c.context.history[0].event_id, "$ctx_bound_2");
    assert_eq!(c.context.history[7].event_id, "$ctx_bound_9");
    assert!(render_context(&c.context, &w).unwrap().len() <= MAX_CONTEXT_INPUT_BYTES);
    // Legal near-limit replies force byte-based omission before the pair cap.
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let s = work::sender().await;
    let text = "r".repeat(8192);
    for i in 0..5 {
        complete(
            &f,
            &r,
            &m,
            &d,
            &a,
            &format!("ctx_large_{i}"),
            "$large",
            201 + i,
            &text,
            1,
        )
        .await;
        send(&f, &m, &s).await;
    }
    f.policy = f.policy.with_shared_thread_context(true);
    let w = queue(&f, &r, &m, &a, "ctx_after_large", "$large", "Recall").await;
    let c = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway,
        )
        .await
        .unwrap();
    assert!(c.context.history.len() < 5);
    assert!(c.context.omitted_earlier);
    assert_eq!(c.context.history.last().unwrap().event_id, "$ctx_large_4");
    assert!(
        c.context
            .history
            .iter()
            .all(|pair| pair.delivery.body == text)
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_known_overrun_never_mints_another_call_or_changes_budget() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let s = work::sender().await;
    let allocated = d.command.reserve_command.requested_tokens;
    let used = allocated + 717;
    let old = complete(
        &f,
        &r,
        &m,
        &d,
        &a,
        "ctx_overrun",
        "$budget",
        201,
        "Real completed result",
        used,
    )
    .await;
    send(&f, &m, &s).await;
    f.policy = f.policy.with_shared_thread_context(true);
    let w = queue(&f, &r, &m, &a, "ctx_overrun_next", "$budget", "Next").await;
    let c = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(c.context.history.len(), 1);
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    assert!(matches!(
        f.domain
            .start_shared_context_work(
                &f.owner.p,
                &f.policy,
                &w.id,
                start_input(&a, &w, &c),
                &m.gateway
            )
            .await,
        Err(Error::Conflict("shared_budget_exhausted"))
    ));
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT old.known_consumed_tokens=$2 AND old.retained_tokens=$2 AND w.started_at_ms IS NULL AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_context_starts WHERE work_id=w.id) AS matched FROM hagency_agent_v1.management_shared_work old,hagency_agent_v1.management_shared_work w WHERE old.id=$1 AND w.id=$3").bind::<Text,_>(&old.id).bind::<BigInt,_>(used).bind::<Text,_>(&w.id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_schema17_atomic_upgrade_preserves_original_plain_work() {
    let (f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = queue(&f, &r, &m, &a, "ctx_migration", "$old", "Original bytes").await;
    let before = allocation_digest(&w).unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute("CREATE TABLE hagency_agent_v1.management_shared_context_starts(dummy text)")
        .await
        .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=17 AND to_regclass('hagency_agent_v1.management_shared_input_modes') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_context_starts")
        .await
        .unwrap();
    let reopened = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let mode = reopened
        .shared_work_input_mode(&f.owner.p, &w.id, read_input(&w))
        .await
        .unwrap();
    assert_eq!(mode.mode, "plain_text_v1");
    assert!(mode.context_digest.is_none());
    reopened
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    let started = reopened
        .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    assert_eq!(allocation_digest(&started.work).unwrap(), before);
    assert_eq!(history(&f, &d).await.command_digest, d.command_digest);
    let p = ManagementPolicy::new(f.admin.p.mxid.clone(), "example.test").unwrap();
    assert!(!p.shared_thread_context());
    let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
        .add_header("host", "example.test", true)
        .send(&svc(&f, m.gateway.clone()))
        .await;
    assert!(
        response.take_json::<Value>().await.unwrap()["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!(SHARED_CONTEXT_CAPABILITY))
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_demand_mode_and_stop_run_or_admin_change_during_start() {
    for replace in [false, true] {
        let (mut f, r, _admin, assignment, m) = Box::pin(prepared()).await;
        f.policy = f
            .policy
            .with_shared_durable_intake(true)
            .with_shared_demands(true)
            .with_shared_successors(true)
            .with_shared_thread_context(true);
        intake_tests::arm(&f, &assignment, &m).await;
        let (_d, a) = qualify(&f).await;
        let w = queue(
            &f,
            &r,
            &m,
            &a,
            &format!("ctx_aba_{replace}"),
            "$aba",
            "Only current Run",
        )
        .await;
        let c = f
            .domain
            .freeze_shared_context(
                &f.owner.p,
                &f.policy,
                &w.id,
                context_input(&a, &w),
                &m.gateway,
            )
            .await
            .unwrap();
        assert_eq!(c.context.control_revision, Some(2));
        assert!(c.context.demand_binding_digest.is_some());
        f.domain
            .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .unwrap();
        let (gateway, entered, release) = intake_tests::delayed_matrix(&m);
        let pending = f.domain.start_shared_context_work(
            &f.owner.p,
            &f.policy,
            &w.id,
            start_input(&a, &w, &c),
            &gateway,
        );
        let change = async {
            entered.notified().await;
            if replace {
                invalidate_authority(&f, &r.project_id).await;
            } else {
                f.domain
                    .change_shared_run_control(
                        &f.owner.p,
                        &f.policy,
                        &assignment.id,
                        intake_tests::change("ctx_stop", 2, "paused"),
                        &m.gateway,
                    )
                    .await
                    .unwrap();
                f.domain
                    .change_shared_run_control(
                        &f.outsider.p,
                        &f.policy,
                        &assignment.id,
                        intake_tests::change("ctx_run", 3, "running"),
                        &m.gateway,
                    )
                    .await
                    .unwrap();
            }
            release.close();
        };
        let (result, _) = tokio::join!(pending, change);
        assert!(result.is_err());
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_context_starts WHERE work_id=$1) AND (SELECT started_at_ms IS NULL FROM hagency_agent_v1.management_shared_work WHERE id=$1) AS matched").bind::<Text,_>(&w.id).get_result::<Check>(&mut db).await.unwrap().matched);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_sent_evidence_survives_incomplete_usage_and_does_not_release_hold() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let w = queue(&f, &r, &m, &a, "ctx_partial", "$partial", "First request").await;
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    f.domain
        .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    let t = work::ticket(&f, &d, &w).await;
    let mut out = work::outcome(&d, &w, &t, 201, "completed");
    out.receipt.usage_complete = false;
    out.receipt.retained_tokens = Some(80);
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, out)
        .await
        .unwrap();
    let s = work::sender().await;
    send(&f, &m, &s).await;
    f.policy = f.policy.with_shared_thread_context(true);
    let next = queue(&f, &r, &m, &a, "ctx_partial_next", "$partial", "Follow up").await;
    let c = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &next.id,
            context_input(&a, &next),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(c.context.history.len(), 1);
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &next), &m.gateway)
        .await
        .unwrap();
    assert!(matches!(
        f.domain
            .start_shared_context_work(
                &f.owner.p,
                &f.policy,
                &next.id,
                start_input(&a, &next, &c),
                &m.gateway
            )
            .await,
        Err(Error::Conflict("shared_work_unsettled"))
    ));
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT known_consumed_tokens=17 AND retained_tokens=80 AND NOT usage_complete AS matched FROM hagency_agent_v1.management_shared_work WHERE id=$1").bind::<Text,_>(&w.id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_queued_followup_includes_prior_answer_sent_before_freeze() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let s = work::sender().await;
    let prior = complete(
        &f,
        &r,
        &m,
        &d,
        &a,
        "ctx_queued_prior",
        "$queued",
        201,
        "Answer delivered after follow-up arrival",
        1,
    )
    .await;
    f.policy = f.policy.with_shared_thread_context(true);
    let w = queue(
        &f,
        &r,
        &m,
        &a,
        "ctx_queued_current",
        "$queued",
        "Queued follow-up",
    )
    .await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("SELECT pg_sleep(0.003)").await.unwrap();
    send(&f, &m, &s).await;
    let c = f
        .domain
        .freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway,
        )
        .await
        .unwrap();
    assert_eq!(c.context.history.len(), 1);
    let pair = &c.context.history[0];
    assert_eq!(pair.work_id, prior.id);
    assert!(pair.received_at_ms < w.created_at_ms);
    assert!(pair.delivery.sent_at_ms > w.created_at_ms);
    assert_eq!(
        pair.delivery.body,
        "Answer delivered after follow-up arrival"
    );
    assert_eq!(
        allocation_digest(&c).unwrap(),
        allocation_digest(
            &f.domain
                .freeze_shared_context(
                    &f.owner.p,
                    &f.policy,
                    &w.id,
                    context_input(&a, &w),
                    &m.gateway
                )
                .await
                .unwrap()
        )
        .unwrap()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_concurrent_instances_freeze_once_and_start_once() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_shared_thread_context(true);
    let (_d, a) = qualify(&f).await;
    let w = queue(
        &f,
        &r,
        &m,
        &a,
        "ctx_concurrent",
        "$concurrent",
        "One exact input",
    )
    .await;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        f.domain.freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway
        ),
        other.freeze_shared_context(
            &f.owner.p,
            &f.policy,
            &w.id,
            context_input(&a, &w),
            &m.gateway
        )
    );
    let one = one.unwrap();
    assert_eq!(
        allocation_digest(&one).unwrap(),
        allocation_digest(&two.unwrap()).unwrap()
    );
    assert_eq!(one.context.context_ordinal, 1);
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    let (first, replay) = tokio::join!(
        f.domain.start_shared_context_work(
            &f.owner.p,
            &f.policy,
            &w.id,
            start_input(&a, &w, &one),
            &m.gateway
        ),
        other.start_shared_context_work(
            &f.owner.p,
            &f.policy,
            &w.id,
            start_input(&a, &w, &one),
            &m.gateway
        )
    );
    assert_ne!(
        first.unwrap().start.newly_started,
        replay.unwrap().start.newly_started
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT (SELECT count(*) FROM hagency_agent_v1.management_shared_context_starts WHERE work_id=$1)=1 AND (SELECT ordinal FROM hagency_agent_v1.management_shared_context_sequences WHERE assignment_id=$2 AND room_id=$3 AND thread_root=$4)=1 AS matched")
        .bind::<Text,_>(&w.id).bind::<Text,_>(&w.assignment_id).bind::<Text,_>(&w.room_id).bind::<Text,_>(&w.thread_root).get_result::<Check>(&mut db).await.unwrap().matched);
}
