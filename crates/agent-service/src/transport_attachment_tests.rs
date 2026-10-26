use super::*;
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_attachment_http_requires_exact_execution_and_current_matrix_membership() {
    use crate::{api::App, gateway::Gateway, identity::Verifier};
    use salvo::{
        Service,
        http::StatusCode,
        test::{ResponseExt, TestClient},
    };
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicBool, Ordering};
    use url::Url;
    let f = fixture().await;
    let joined = Arc::new(AtomicBool::new(true));
    let gateway = Gateway::new(
        {
            let facts = f.facts.clone();
            let joined = joined.clone();
            Arc::new(move |room| {
                let facts = facts.clone();
                let joined = joined.clone();
                Box::pin(async move {
                    let member = |mxid: &str, present: bool| json!({"type":"m.room.member","state_key":mxid,"content":{"membership":if present {"join"} else {"leave"}}});
                    if room == facts.space_id {
                        Ok(vec![
                            member(&facts.owner_mxid, true),
                            json!({"type":"m.space.child","state_key":facts.room_id,"content":{"via":["example.test"]}}),
                        ])
                    } else {
                        assert_eq!(room, facts.room_id);
                        Ok(vec![
                            member(&facts.owner_mxid, true),
                            member(&facts.puppet_mxid, true),
                            member(
                                &facts.requester_mxid,
                                joined.as_ref().load(Ordering::Acquire),
                            ),
                            json!({"type":"m.room.create","state_key":"","content":{}}),
                        ])
                    }
                })
            })
        },
        "@_hagency_service:example.test".into(),
    );
    let issuer = Url::parse("https://example.test/auth/").unwrap();
    let origin = Url::parse("https://example.test/").unwrap();
    let verifier = Verifier::new(
        issuer.clone(),
        issuer.join("introspect").unwrap(),
        origin.clone(),
        "unused-test-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let app = App::new(
        f.auth.clone(),
        verifier,
        origin,
        issuer,
        "example.test".into(),
    )
    .with_domain(f.domain.clone(), gateway);
    let without_transport = Service::new(app.clone().router());
    let service = Service::new(app.with_transport(f.transport.clone()).router());
    for (app, supported) in [(&without_transport, false), (&service, true)] {
        let mut response = TestClient::get("https://example.test/hagency/api/v1/discovery")
            .add_header("host", "example.test", true)
            .send(app)
            .await;
        assert_eq!(response.status_code, Some(StatusCode::OK));
        let discovery = response.take_json::<Value>().await.unwrap();
        assert_eq!(
            discovery["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!(crate::attachments::CAPABILITY)),
            supported
        );
        assert!(
            !discovery["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!(crate::file_delivery::CAPABILITY)),
            "metadata-only deployments cannot promise delivery"
        );
    }
    async fn post(
        service: &Service,
        path: &str,
        token: &str,
        body: Value,
        status: StatusCode,
    ) -> Value {
        let mut response = TestClient::post(format!("https://example.test{path}"))
            .add_header("host", "example.test", true)
            .bearer_auth(token)
            .json(&body)
            .send(service)
            .await;
        let actual = response.status_code;
        let value = response.take_json::<Value>().await.unwrap();
        assert_eq!(actual, Some(status), "response: {value}");
        value
    }
    let now = crate::api::now_ms();
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: lease.agent_id,
        epoch: lease.epoch,
    };
    let attachment=crate::attachments::Attachment::from_content(&json!({"msgtype":"m.file","body":"report.txt","url":"mxc://media.test/http-source","info":{"size":5}})).unwrap();
    let result = f
        .transport
        .ingest_routed_attachment_received(
            &f.binding,
            event(&f, "http-media", true, None),
            Some(attachment.clone()),
            &f.facts,
            now,
            now,
        )
        .await
        .unwrap();
    let RouteResult::Queued { dispatch_id: id } = result else {
        panic!("new file request must queue")
    };
    let poll = "/hagency/api/v1/execution/events/poll";
    let old = post(
        &service,
        poll,
        &f.device_token,
        json!({"lease":lease,"bindingId":f.binding,"limit":1}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(old["events"], json!([]));
    let offered = post(
        &service,
        poll,
        &f.device_token,
        json!({"lease":lease,"bindingId":f.binding,"limit":1,"acceptFiles":true}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(offered["events"][0]["id"], id);
    // The regular dispatch envelope stays compatible and contains no locator/key.
    assert!(!offered.to_string().contains("mxc://"));
    let path = "/hagency/api/v1/execution/files/attachments";
    let request = json!({"lease":lease,"dispatchId":id,"executionId":"http-media-execution"});
    post(
        &service,
        path,
        &f.credential,
        request.clone(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    post(
        &service,
        path,
        &f.device_token,
        request.clone(),
        StatusCode::CONFLICT,
    )
    .await;
    running(&f, &lease, &id, "http-media-execution").await;
    let accepted = post(
        &service,
        path,
        &f.device_token,
        request.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        accepted,
        json!({"dispatchId":id,"executionId":"http-media-execution","attachments":[attachment]})
    );
    let mut foreign = request.clone();
    foreign["executionId"] = json!("foreign-execution");
    post(
        &service,
        path,
        &f.device_token,
        foreign,
        StatusCode::CONFLICT,
    )
    .await;
    let mut extra = request.clone();
    extra["url"] = json!("https://arbitrary.test/file");
    post(
        &service,
        path,
        &f.device_token,
        extra,
        StatusCode::BAD_REQUEST,
    )
    .await;
    joined.store(false, Ordering::Release);
    post(
        &service,
        path,
        &f.device_token,
        request.clone(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    joined.store(true, Ordering::Release);
    post(
        &service,
        path,
        &f.device_token,
        request,
        StatusCode::CONFLICT,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_attachment_admission_is_atomic_private_and_excluded_from_legacy_poll() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let attachment=crate::attachments::Attachment::from_content(&serde_json::json!({"msgtype":"m.file","body":"analyze this","filename":"input.bin","url":"mxc://example.test/exact-original","info":{"size":19}})).unwrap();
    let source = event(&f, "attachment-original", true, None);
    let queued = f
        .transport
        .ingest_routed_attachment_received(
            &f.binding,
            source.clone(),
            Some(attachment.clone()),
            &f.facts,
            now,
            now,
        )
        .await
        .unwrap();
    let id = match queued {
        RouteResult::Queued { dispatch_id } => dispatch_id,
        _ => panic!("expected original queue"),
    };
    assert!(
        matches!(f.transport.ingest_routed_attachment_received(&f.binding,source.clone(),Some(attachment.clone()),&f.facts,now,now).await.unwrap(),RouteResult::Duplicate{dispatch_id} if dispatch_id==id)
    );
    let changed=crate::attachments::Attachment::from_content(&serde_json::json!({"msgtype":"m.file","body":"analyze this","filename":"input.bin","url":"mxc://example.test/changed"})).unwrap();
    assert!(
        f.transport
            .ingest_routed_attachment_received(
                &f.binding,
                source,
                Some(changed),
                &f.facts,
                now,
                now
            )
            .await
            .is_err()
    );
    let text = enqueue(&f, "ordinary-after-attachment").await;
    let lease = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 30_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let legacy = f
        .transport
        .event_candidates_for_binding(&f.p, &lease, &f.binding, 1, now)
        .await
        .unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].id, text);
    let current = f
        .transport
        .event_candidates_with_attachments(&f.p, &lease, &f.binding, 10, now)
        .await
        .unwrap();
    assert!(current.iter().any(|v| v.id == id));
    assert!(
        f.transport
            .execution_attachments(&f.p, &lease, &id, "original-execution", &f.facts, now)
            .await
            .is_err()
    );
    running(&f, &lease, &id, "original-execution").await;
    assert_eq!(
        f.transport
            .execution_attachments(&f.p, &lease, &id, "original-execution", &f.facts, now)
            .await
            .unwrap(),
        vec![attachment]
    );
    assert!(
        f.transport
            .execution_attachments(&f.second, &lease, &id, "original-execution", &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        f.transport
            .execution_attachments(&f.p, &lease, &id, "different-execution", &f.facts, now)
            .await
            .is_err()
    );
    {
        let mut db = f.transport.db.lock().await.unwrap();
        assert!(diesel::sql_query("UPDATE hagency_agent_v1.event_attachments SET descriptor='{}'::jsonb WHERE dispatch_id=$1").bind::<diesel::sql_types::Text,_>(&id).execute(&mut *db).await.is_err());
    }
    let mut left = f.facts.clone();
    left.requester_in_room = false;
    assert!(
        f.transport
            .execution_attachments(
                &f.p,
                &lease,
                &id,
                "original-execution",
                &left,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    assert!(
        f.transport
            .execution_attachments(
                &f.p,
                &lease,
                &id,
                "original-execution",
                &f.facts,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
}
