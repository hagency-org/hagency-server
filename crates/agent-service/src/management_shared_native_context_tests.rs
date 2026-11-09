use super::*;

#[test]
fn shared_context_v3_canonical_native_input_rejects_transcript_or_coverage_changes() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-native-context-v3.json"
    ))
    .unwrap();
    let work: SharedWorkItem = serde_json::from_value(fixture["work"].clone()).unwrap();
    let context: SharedThreadContext = serde_json::from_value(fixture["context"].clone()).unwrap();
    let rendered = render_context(&context, &work).unwrap();
    assert_eq!(rendered, fixture["renderedInput"]);
    assert_eq!(
        allocation_digest(&context).unwrap(),
        fixture["contextDigest"]
    );
    assert_eq!(crate::hash(&rendered), fixture["inputDigest"]);
    for (key, value) in [
        ("inputProfile", json!(INPUT_PROFILE_V2)),
        ("coverage", json!(COVERAGE_PRIOR_DELIVERED_PAIRS)),
        ("omittedEarlier", json!(false)),
        ("roomId", json!("!other:test")),
        ("version", json!(2)),
    ] {
        let mut modified = fixture["context"].clone();
        modified[key] = value;
        assert!(
            render_context(&serde_json::from_value(modified).unwrap(), &work).is_err(),
            "{key}"
        );
    }
    let old: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-thread-context-v1.json"
    ))
    .unwrap();
    let mut repeated = context;
    repeated.history = serde_json::from_value(old["context"]["history"].clone()).unwrap();
    assert!(render_context(&repeated, &work).is_err());
    assert!(
        SharedContextBudgetInput {
            version: 3,
            model_context_window: None
        }
        .validate()
        .is_ok()
    );
    assert!(
        SharedContextBudgetInput {
            version: 3,
            model_context_window: Some(128_000)
        }
        .validate()
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_context_v3_freezes_only_current_request_preserving_exact_start_and_legacy_replay() {
    let (mut f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, a) = qualify(&f).await;
    let sender = work::sender().await;
    complete(
        &f,
        &r,
        &m,
        &d,
        &a,
        "native_prior",
        "$native",
        201,
        "Old answer must stay in native history",
        1,
    )
    .await;
    send(&f, &m, &sender).await;
    f.policy = f.policy.with_shared_thread_context(true);
    let w = queue(
        &f,
        &r,
        &m,
        &a,
        "native_current",
        "$native",
        "Summarize our conversation",
    )
    .await;
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
        .await
        .unwrap();
    let native = SharedContextInput {
        authority: a.authority.clone(),
        work_digest: allocation_digest(&w).unwrap(),
        budget: Some(SharedContextBudgetInput {
            version: 3,
            model_context_window: None,
        }),
    };
    let c = f
        .domain
        .freeze_shared_context(&f.owner.p, &f.policy, &w.id, native.clone(), &m.gateway)
        .await
        .unwrap();
    assert_eq!(c.context.version, 3);
    assert!(
        c.context.history.is_empty() && c.context.budget.is_none() && c.context.omitted_earlier
    );
    let rendered = render_context(&c.context, &w).unwrap();
    assert!(rendered.contains("read_room_history") && rendered.contains(&w.body));
    assert!(!rendered.contains("Old answer must stay in native history"));
    assert_eq!(crate::hash(&rendered), c.input_digest);
    assert!(
        f.domain
            .authorize_shared_room_send(&f.owner.p, &f.policy, &w.id, &m.gateway)
            .await
            .is_err(),
        "a frozen description cannot authorize file publication before call admission"
    );
    for later in [
        native,
        context_input(&a, &w),
        v2_input(&a, &w, Some(128_000)),
    ] {
        let again = f
            .domain
            .freeze_shared_context(&f.owner.p, &f.policy, &w.id, later, &m.gateway)
            .await
            .unwrap();
        assert_eq!(
            allocation_encoding(&again).unwrap(),
            allocation_encoding(&c).unwrap()
        );
    }
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(&a, &w), &m.gateway)
            .await
            .is_err()
    );
    let mut changed = start_input(&a, &w, &c);
    changed.input_digest = "f".repeat(64);
    assert!(
        f.domain
            .start_shared_context_work(&f.owner.p, &f.policy, &w.id, changed, &m.gateway)
            .await
            .is_err()
    );
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
            .unwrap()
            .start
            .newly_started
    );
    let recovered = f
        .domain
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
        allocation_encoding(&recovered).unwrap(),
        allocation_encoding(&c).unwrap()
    );
    f.domain
        .authorize_shared_room_send(&f.owner.p, &f.policy, &w.id, &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .authorize_shared_room_send(&f.outsider.p, &f.policy, &w.id, &m.gateway)
            .await
            .is_err()
    );
    let (gateway, entered, release) = intake_tests::delayed_matrix(&m);
    let pending = f
        .domain
        .authorize_shared_room_send(&f.owner.p, &f.policy, &w.id, &gateway);
    let revoke = async {
        entered.notified().await;
        invalidate_authority(&f, &r.project_id).await;
        release.close();
    };
    let (result, ()) = tokio::join!(pending, revoke);
    assert!(
        result.is_err(),
        "authority is rechecked after the Matrix wait"
    );
}
