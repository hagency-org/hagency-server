use super::*;
use crate::domain::shared_protocol::{CURRENT, HEADER};
use crate::management_shared_work_types::{
    SharedWorkRecoverInput, SharedWorkResultTicket, SharedWorkResultUpload,
};

#[derive(QueryableByName)]
struct Raw {
    #[diesel(sql_type=Text)]
    value: String,
}
async fn received(db: &mut AsyncPgConnection) -> i64 {
    db.batch_execute("SELECT pg_sleep(0.002)").await.unwrap();
    sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS count")
        .get_result::<Count>(db)
        .await
        .unwrap()
        .count
}
async fn forget(f: &Fixture, db: &mut AsyncPgConnection) {
    sql_query(
        "DELETE FROM hagency_agent_v1.management_shared_executor_protocols WHERE device_id=$1",
    )
    .bind::<Text, _>(f.owner.p.device_id.as_ref().unwrap())
    .execute(db)
    .await
    .unwrap();
}
async fn notice(f: &Fixture, p: &crate::store::Principal, room: &str) -> Value {
    f.domain
        .shared_approvals(p, serde_json::from_value(json!({"roomId":room})).unwrap())
        .await
        .unwrap()
}
async fn post_protocol(
    s: &Service,
    path: &str,
    token: &str,
    body: Value,
    headers: &[&str],
    status: StatusCode,
) -> Value {
    let mut request = TestClient::post(format!(
        "https://example.test/hagency/api/v1/management/{path}"
    ))
    .add_header("host", "example.test", true)
    .bearer_auth(token)
    .json(&body);
    for header in headers {
        request = request.add_header(HEADER, *header, false);
    }
    let mut response = Box::pin(request.send(s)).await;
    let actual = response.status_code;
    let value: Value = response.take_json().await.unwrap();
    assert_eq!(actual, Some(status), "{path}: {value}");
    value
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_protocol_absent_worker_retains_codex_and_claude_messages_for_current_worker() {
    for claude in [false, true] {
        let (f, request, assignment, matrix) = Box::pin(prepared_pool(true)).await;
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        if claude {
            sql_query("UPDATE hagency_agent_v1.management_contributions SET runtime='claude' WHERE id=(SELECT pool_contribution_id FROM hagency_agent_v1.management_agent_requests WHERE id=$1)")
                .bind::<Text,_>(&request.id).execute(&mut db).await.unwrap();
        }
        forget(&f, &mut db).await;
        let event = crate::transport::RoutedEvent {
            event_id: "$before_first_poll".into(),
            room_id: request.room_id.clone(),
            sender_mxid: f.outsider.p.mxid.clone(),
            body: "Keep this exact original message".into(),
            mentioned_mxids: Default::default(),
            thread_root: Some("$original_thread".into()),
            encrypted: false,
            is_edit: false,
        };
        let at = received(&mut db).await;
        for _ in 0..2 {
            f.domain
                .route_shared_event(&f.policy, &event, at, &matrix.gateway)
                .await
                .unwrap();
        }
        let before=sql_query("SELECT row_to_json(i)::text AS value FROM hagency_agent_v1.management_shared_intake i WHERE assignment_id=$1 AND event_id=$2")
            .bind::<Text,_>(&assignment.id).bind::<Text,_>(&event.event_id).get_result::<Raw>(&mut db).await.unwrap().value;
        assert_eq!(
            serde_json::from_str::<Value>(&before).unwrap()["state"],
            "pending"
        );
        let blocked = notice(&f, &f.outsider.p, &request.room_id).await;
        assert_eq!(blocked["executions"][0]["state"], "blocked");
        assert_eq!(
            blocked["executions"][0]["reason"],
            "owner_daemon_update_required"
        );
        assert_eq!(blocked["executions"][0]["eventId"], event.event_id);
        assert_eq!(
            notice(&f, &f.admin.p, &request.room_id).await["executions"],
            json!([])
        );
        assert!(sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1) AS matched")
            .bind::<Text,_>(&assignment.id).get_result::<Check>(&mut db).await.unwrap().matched);
        let service = svc(&f, matrix.gateway.clone());
        // A dashboard with the header is still not a live worker observation.
        post_protocol(
            &service,
            "shared-execution/approvals/query",
            &f.owner.token,
            json!({"roomId":request.room_id}),
            &[CURRENT],
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            notice(&f, &f.outsider.p, &request.room_id).await["executions"][0]["state"],
            "blocked"
        );
        // Another authenticated account cannot qualify this installation.
        post_protocol(
            &service,
            "shared-execution/runtime/poll-v4",
            &f.outsider.token,
            json!({}),
            &[CURRENT],
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            notice(&f, &f.outsider.p, &request.room_id).await["executions"][0]["state"],
            "blocked"
        );
        let polled = post_protocol(
            &service,
            "shared-execution/runtime/poll-v4",
            &f.owner.token,
            json!({}),
            &[CURRENT],
            StatusCode::OK,
        )
        .await;
        let runtime: SharedRuntimeDelivery =
            serde_json::from_value(polled["delivery"].clone()).unwrap();
        assert_eq!(
            runtime.command.runtime().unwrap(),
            if claude { "claude" } else { "codex" }
        );
        let after=sql_query("SELECT row_to_json(i)::text AS value FROM hagency_agent_v1.management_shared_intake i WHERE assignment_id=$1 AND event_id=$2")
            .bind::<Text,_>(&assignment.id).bind::<Text,_>(&event.event_id).get_result::<Raw>(&mut db).await.unwrap().value;
        assert_eq!(
            before, after,
            "protocol recovery must not rewrite/recreate intake"
        );
        let activity = save(&f, &runtime, "qualified", 101)
            .await
            .unwrap()
            .activity
            .unwrap();
        let work = f
            .domain
            .poll_shared_work(&f.owner.p, &f.policy, activity.authority, &matrix.gateway)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (work.event_id, work.body, work.thread_root),
            (event.event_id, event.body, "$original_thread".into())
        );
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_protocol_rejects_old_ack_but_never_blocks_started_settlement() {
    let (f, request, _, _, matrix) = Box::pin(prepared()).await;
    let (runtime, activity) = qualify(&f).await;
    let work = work::queued(&f, &request, &matrix, &activity, "protocol_ack").await;
    let input = SharedWorkInput {
        authority: activity.authority.clone(),
        work_id: work.id.clone(),
        work_digest: allocation_digest(&work).unwrap(),
    };
    let service = svc(&f, matrix.gateway.clone());
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    for headers in [
        vec![],
        vec!["native-context-v2;claude-v4"],
        vec![CURRENT, CURRENT],
    ] {
        let reply = post_protocol(
            &service,
            "shared-execution/work/ack",
            &f.owner.token,
            serde_json::to_value(&input).unwrap(),
            &headers,
            StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(reply["code"], "owner_daemon_update_required");
        assert!(sql_query("SELECT state='queued' AND acknowledged_at_ms IS NULL AND started_at_ms IS NULL AS matched FROM hagency_agent_v1.management_shared_work WHERE id=$1")
            .bind::<Text,_>(&work.id).get_result::<Check>(&mut db).await.unwrap().matched);
    }
    post_protocol(
        &service,
        "shared-execution/work/ack",
        &f.owner.token,
        serde_json::to_value(&input).unwrap(),
        &[CURRENT],
        StatusCode::OK,
    )
    .await;
    f.domain
        .start_current_work(&f.owner.p, &f.policy, input, &matrix.gateway)
        .await
        .unwrap();
    // Once admitted, expired proof cannot kill a turn, revoke its activity or
    // obstruct the original usage/answer receipt.
    sql_query("UPDATE hagency_agent_v1.management_shared_executor_protocols SET observed_at_ms=1,expires_at_ms=2 WHERE device_id=$1")
        .bind::<Text,_>(f.owner.p.device_id.as_ref().unwrap()).execute(&mut db).await.unwrap();
    post_protocol(
        &service,
        "shared-execution/activity/renew",
        &f.owner.token,
        json!({"authority":activity.authority,"renewalId":"settle_after_protocol_expiry"}),
        &[],
        StatusCode::OK,
    )
    .await;
    assert!(sql_query("SELECT state='started' AS matched FROM hagency_agent_v1.management_shared_work WHERE id=$1")
        .bind::<Text,_>(&work.id).get_result::<Check>(&mut db).await.unwrap().matched);
    let recovery = SharedWorkRecoverInput {
        runtime_command_digest: runtime.command_digest.clone(),
        work_digest: allocation_digest(&work).unwrap(),
        observation_id: None,
    };
    let response = post_protocol(
        &service,
        &format!("shared-execution/work/{}/recover", work.id),
        &f.owner.token,
        serde_json::to_value(recovery).unwrap(),
        &[],
        StatusCode::OK,
    )
    .await;
    let ticket: SharedWorkResultTicket =
        serde_json::from_value(response["ticket"].clone()).unwrap();
    let upload = SharedWorkResultUpload {
        work_digest: ticket.work_digest.clone(),
        token: ticket.token,
        receipt: SharedWorkReceipt {
            version: 1,
            observation_id: "protocol_completion".into(),
            work_id: work.id.clone(),
            work_digest: ticket.work_digest,
            execution_id: work.execution_id.clone(),
            call_id: "protocol_call".into(),
            runtime_command_digest: runtime.command_digest,
            process_digest: "a".repeat(64),
            ledger_revision: 220,
            state: "completed".into(),
            known_consumed_tokens: 17,
            retained_tokens: Some(17),
            usage_complete: true,
            body_digest: Some(crate::hash("Finished answer")),
            body: Some("Finished answer".into()),
        },
    };
    post_protocol(
        &service,
        &format!("shared-execution/work/{}/result", work.id),
        &f.owner.token,
        serde_json::to_value(upload).unwrap(),
        &[],
        StatusCode::OK,
    )
    .await;
    assert!(sql_query("SELECT state='completed' AND usage_complete AND known_consumed_tokens=17 AS matched FROM hagency_agent_v1.management_shared_work WHERE id=$1")
        .bind::<Text,_>(&work.id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_protocol_scope_freshness_and_encrypted_pending_visibility() {
    let (f, request, assignment, matrix) = Box::pin(prepared_pool(true)).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let event = crate::transport::RoutedEvent {
        event_id: "$encrypted_pending_protocol".into(),
        room_id: request.room_id.clone(),
        sender_mxid: f.outsider.p.mxid.clone(),
        body: String::new(),
        mentioned_mxids: Default::default(),
        thread_root: None,
        encrypted: true,
        is_edit: false,
    };
    forget(&f, &mut db).await;
    f.domain
        .route_shared_event(&f.policy, &event, now_ms(), &matrix.gateway)
        .await
        .unwrap();
    let status = notice(&f, &f.outsider.p, &request.room_id).await;
    assert_eq!(
        status["executions"][0]["reason"],
        "owner_daemon_update_required"
    );
    assert!(status["executions"][0].get("body").is_none());
    assert_eq!(
        notice(&f, &f.admin.p, &request.room_id).await["executions"],
        json!([])
    );
    let before=sql_query("SELECT row_to_json(c)::text AS value FROM hagency_agent_v1.management_shared_e2ee_candidates c WHERE assignment_id=$1 AND event_id=$2")
        .bind::<Text,_>(&assignment.id).bind::<Text,_>(&event.event_id).get_result::<Raw>(&mut db).await.unwrap().value;
    f.domain
        .observe_shared_daemon_protocol(&f.owner.p, true)
        .await
        .unwrap();
    assert_eq!(
        notice(&f, &f.outsider.p, &request.room_id).await["executions"],
        json!([])
    );
    sql_query("UPDATE hagency_agent_v1.management_shared_executor_protocols SET observed_at_ms=1,expires_at_ms=2 WHERE device_id=$1")
        .bind::<Text,_>(f.owner.p.device_id.as_ref().unwrap()).execute(&mut db).await.unwrap();
    assert_eq!(
        notice(&f, &f.outsider.p, &request.room_id).await["executions"][0]["reason"],
        "owner_daemon_unavailable"
    );
    f.domain
        .observe_shared_daemon_protocol(&f.owner.p, true)
        .await
        .unwrap();
    sql_query("UPDATE hagency_agent_v1.management_shared_executor_protocols SET generation=generation+1 WHERE device_id=$1")
        .bind::<Text,_>(f.owner.p.device_id.as_ref().unwrap()).execute(&mut db).await.unwrap();
    assert_eq!(
        notice(&f, &f.outsider.p, &request.room_id).await["executions"][0]["reason"],
        "owner_daemon_update_required"
    );
    f.domain
        .observe_shared_daemon_protocol(&f.owner.p, true)
        .await
        .unwrap();
    db.batch_execute("INSERT INTO hagency_agent_v1.sessions(id,user_id,token_hash,client_id,valid_until_ms) SELECT id||'_protocol_old',user_id,token_hash||'_protocol_old',client_id,valid_until_ms FROM hagency_agent_v1.sessions WHERE id=(SELECT session_id FROM hagency_agent_v1.management_shared_executor_protocols LIMIT 1)").await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_shared_executor_protocols SET session_id=session_id||'_protocol_old' WHERE device_id=$1")
        .bind::<Text,_>(f.owner.p.device_id.as_ref().unwrap()).execute(&mut db).await.unwrap();
    assert_eq!(
        notice(&f, &f.outsider.p, &request.room_id).await["executions"][0]["reason"],
        "owner_daemon_update_required"
    );
    f.domain
        .observe_shared_daemon_protocol(&f.owner.p, true)
        .await
        .unwrap();
    let after=sql_query("SELECT row_to_json(c)::text AS value FROM hagency_agent_v1.management_shared_e2ee_candidates c WHERE assignment_id=$1 AND event_id=$2")
        .bind::<Text,_>(&assignment.id).bind::<Text,_>(&event.event_id).get_result::<Raw>(&mut db).await.unwrap().value;
    assert_eq!(
        before, after,
        "protocol observations cannot consume ciphertext candidates"
    );
}
