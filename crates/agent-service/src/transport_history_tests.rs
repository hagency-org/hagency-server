use super::*;
trait HistoryLeaseReference {
    fn reference(self) -> LeaseRef;
}
impl HistoryLeaseReference for Lease {
    fn reference(self) -> LeaseRef {
        LeaseRef {
            agent_id: self.agent_id,
            epoch: self.epoch,
        }
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_started_history_pages_are_permanent_owner_scoped_and_body_free() {
    let mut f = fixture().await;
    let reference = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60000, false, crate::api::now_ms())
        .await
        .unwrap()
        .reference();
    let empty = f
        .transport
        .execution_history(&f.p, &f.agent, None, None, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(empty.snapshot.count, 0);
    for n in 0..129 {
        f.facts.observed_at_ms = crate::api::now_ms();
        let id = enqueue(&f, &format!("history-{n:03}")).await;
        running(&f, &reference, &id, &format!("execution-history-{n:03}")).await;
        f.transport
            .finish_without_reply(
                &f.p,
                &reference,
                &id,
                &format!("execution-history-{n:03}"),
                if n % 2 == 0 { "failed" } else { "rejected" },
                crate::api::now_ms(),
            )
            .await
            .unwrap();
    }
    let first = f
        .transport
        .execution_history(&f.second, &f.agent, None, None, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(first.executions.len(), 128);
    assert_eq!(first.snapshot.count, 129);
    assert!(
        first
            .executions
            .windows(2)
            .all(|w| w[0].dispatch_id < w[1].dispatch_id)
    );
    let cursor = first.next_cursor.as_deref().unwrap();
    let second = f
        .transport
        .execution_history(
            &f.second,
            &f.agent,
            Some(cursor),
            Some(&first.snapshot),
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(second.snapshot, first.snapshot);
    assert_eq!(second.executions.len(), 1);
    assert!(second.next_cursor.is_none());
    assert!(second.executions[0].dispatch_id.as_str() > cursor);
    let entry = &first.executions[0];
    let encoded = serde_json::to_value(entry).unwrap();
    assert!(encoded.get("body").is_none());
    assert!(encoded.get("amount").is_none());
    assert_eq!(
        entry.immutable_digest,
        hash(
            &serde_json::to_string(&(
                &entry.dispatch_id,
                &entry.binding_id,
                &entry.agent_id,
                &entry.event_id,
                &entry.room_id,
                &entry.requester_mxid,
                &entry.thread_root,
                "Run a task",
                entry.binding_generation
            ))
            .unwrap()
        )
    );
    assert!(
        f.transport
            .execution_history(
                &f.second,
                &f.agent,
                Some(cursor),
                None,
                crate::api::now_ms()
            )
            .await
            .is_err()
    );
    let other = fixture().await;
    assert!(
        other
            .transport
            .execution_history(&other.p, &f.agent, None, None, crate::api::now_ms())
            .await
            .is_err()
    );
    {
        let mut db = f.transport.db.lock().await;
        for statement in [
            "DELETE FROM hagency_agent_v1.owner_events WHERE id=$1",
            "UPDATE hagency_agent_v1.owner_events SET execution_id=NULL WHERE id=$1",
            "UPDATE hagency_agent_v1.owner_events SET dispatch_epoch=dispatch_epoch+1 WHERE id=$1",
            "UPDATE hagency_agent_v1.owner_events SET body='' WHERE id=$1",
        ] {
            assert!(
                sql_query(statement)
                    .bind::<Text, _>(&entry.dispatch_id)
                    .execute(&mut *db)
                    .await
                    .is_err()
            );
        }
    }
    // A new start changes the snapshot, while changing lease/state does not.
    f.facts.observed_at_ms = crate::api::now_ms();
    let id = enqueue(&f, "after-page-one").await;
    running(&f, &reference, &id, "after-page-execution").await;
    assert!(matches!(
        f.transport
            .execution_history(
                &f.second,
                &f.agent,
                Some(cursor),
                Some(&first.snapshot),
                crate::api::now_ms()
            )
            .await,
        Err(Error::Conflict("execution_history_changed"))
    ));
    let latest = f
        .transport
        .execution_history(&f.p, &f.agent, None, None, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(latest.snapshot.count, 130);
    assert!(matches!(
        f.transport
            .acquire_lease(
                &f.p,
                &f.agent,
                60000,
                true,
                &first.snapshot,
                crate::api::now_ms()
            )
            .await,
        Err(Error::Conflict("execution_history_changed"))
    ));
    f.transport
        .renew_lease(&f.p, &reference, 60000, crate::api::now_ms())
        .await
        .unwrap();
    let instance = f
        .domain
        .agent(&f.p, &f.agent, crate::api::now_ms())
        .await
        .unwrap();
    f.domain
        .set_execution_device(
            &f.second,
            &f.agent,
            crate::domain::SetExecutionDevice {
                expected_generation: instance.generation,
            },
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    f.transport
        .acquire_lease(
            &f.second,
            &f.agent,
            60000,
            true,
            &latest.snapshot,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(
        f.transport
            .execution_history(&f.second, &f.agent, None, None, crate::api::now_ms())
            .await
            .unwrap()
            .snapshot,
        latest.snapshot
    );
    f.domain
        .retire_agent(&f.p, &f.agent, crate::api::now_ms())
        .await
        .unwrap();
    assert_eq!(
        f.transport
            .execution_history(&f.second, &f.agent, None, None, crate::api::now_ms())
            .await
            .unwrap()
            .snapshot,
        latest.snapshot
    );
    f.auth
        .revoke_device(
            &f.credential,
            f.second.device_id.as_deref().unwrap(),
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    assert!(
        f.transport
            .execution_history(&f.second, &f.agent, None, None, crate::api::now_ms())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_history_acquire_and_old_execution_start_are_atomic() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let old = f
        .transport
        .acquire_for_test(&f.p, &f.agent, 60000, false, now)
        .await
        .unwrap()
        .reference();
    let id = enqueue(&f, "atomic-history-race").await;
    f.transport
        .claim_event(&f.p, &old, &id, &f.facts, now)
        .await
        .unwrap();
    f.transport.acknowledge(&f.p, &old, &id, now).await.unwrap();
    let witnessed = f
        .transport
        .execution_history(&f.second, &f.agent, None, None, now)
        .await
        .unwrap()
        .snapshot;
    let parallel = TransportStore::open(
        &std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap(),
        Limits::default(),
    )
    .await
    .unwrap();
    let (start, takeover) = tokio::join!(
        f.transport
            .start_execution(&f.p, &old, &id, "race-execution", &f.facts, now),
        parallel.acquire_lease(&f.second, &f.agent, 60000, true, &witnessed, now)
    );
    assert!(
        !(start.is_ok() && takeover.is_ok()),
        "a started execution cannot be omitted by a successful takeover witness"
    );
    match (start, takeover) {
        (Ok(_), Err(Error::Unauthorized("execution_device_required"))) => {
            f.transport
                .renew_lease(&f.p, &old, 60000, now)
                .await
                .unwrap();
            assert_eq!(
                f.transport
                    .execution_history(&f.p, &f.agent, None, None, now)
                    .await
                    .unwrap()
                    .snapshot
                    .count,
                1
            );
        }
        (Err(_), Ok(_)) => {
            assert_eq!(
                f.transport
                    .execution_history(&f.second, &f.agent, None, None, now)
                    .await
                    .unwrap()
                    .snapshot,
                witnessed
            );
        }
        outcome => panic!("unexpected serialized race: {outcome:?}"),
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_execution_history_real_routes_require_device_and_atomic_witness() {
    use crate::{api::App, gateway::Gateway, identity::Verifier};
    use salvo::{
        Service,
        http::StatusCode,
        test::{ResponseExt, TestClient},
    };
    use serde_json::{Value, json};
    use url::Url;
    let mut f = fixture().await;
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
    let gateway = Gateway::new(
        Arc::new(|_| {
            Box::pin(async { panic!("history/acquire cannot consult Matrix or provision") })
        }),
        "@_hagency_service:example.test".into(),
    );
    let app = App::new(
        f.auth.clone(),
        verifier,
        origin,
        issuer,
        "example.test".into(),
    )
    .with_domain(f.domain.clone(), gateway)
    .with_transport(f.transport.clone());
    let service = Service::new(app.router());
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
    let history_path = "/hagency/api/v1/execution/history";
    let acquire_path = "/hagency/api/v1/execution/leases/acquire";
    post(
        &service,
        history_path,
        &f.credential,
        json!({"agentId":f.agent,"cursor":null,"snapshot":null}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let h = post(
        &service,
        history_path,
        &f.device_token,
        json!({"agentId":f.agent,"cursor":null,"snapshot":null}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(h["history"]["snapshot"]["count"], 0);
    post(
        &service,
        acquire_path,
        &f.device_token,
        json!({"agentId":f.agent,"ttlMs":30000,"takeover":false}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    {
        let mut db = f.transport.db.lock().await;
        assert!(!sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.execution_leases WHERE agent_id=$1) AS matched").bind::<Text,_>(&f.agent).get_result::<Flag>(&mut *db).await.unwrap().matched);
    }
    let acquired=post(&service,acquire_path,&f.device_token,json!({"agentId":f.agent,"ttlMs":30000,"takeover":false,"historySnapshot":h["history"]["snapshot"]}),StatusCode::OK).await;
    let reference = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: acquired["lease"]["epoch"].as_i64().unwrap(),
    };
    f.facts.observed_at_ms = crate::api::now_ms();
    let id = enqueue(&f, "http-witness-start").await;
    running(&f, &reference, &id, "http-witness-execution").await;
    let denied=post(&service,acquire_path,&f.device_token,json!({"agentId":f.agent,"ttlMs":30000,"takeover":true,"historySnapshot":h["history"]["snapshot"]}),StatusCode::CONFLICT).await;
    assert_eq!(denied["code"], "execution_history_changed");
    f.transport
        .renew_lease(&f.p, &reference, 30000, crate::api::now_ms())
        .await
        .unwrap();
    let history = post(
        &service,
        history_path,
        &f.device_token,
        json!({"agentId":f.agent,"cursor":null,"snapshot":null}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(history["history"]["snapshot"]["count"], 1);
    assert_eq!(
        history["history"]["executions"][0]["executionId"],
        "http-witness-execution"
    );
    assert!(history["history"]["executions"][0].get("body").is_none());
    post(&service,acquire_path,&f.device_token,json!({"agentId":f.agent,"ttlMs":30000,"takeover":true,"historySnapshot":history["history"]["snapshot"]}),StatusCode::OK).await;
    post(
        &service,
        history_path,
        &f.device_token,
        json!({"agentId":f.agent,"cursor":null,"snapshot":null,"ownerUserId":f.p.user_id}),
        StatusCode::BAD_REQUEST,
    )
    .await;
}
