use super::*;
#[test]
fn live_and_done_bodies_read_as_plain_words() {
    assert!(live_body(&[]).starts_with("⏳ Working…"));
    let body = live_body(&[
        "reading a.rs".into(),
        "searching fn main".into(),
        "running `cargo test`".into(),
    ]);
    let mut lines = body.lines();
    assert_eq!(lines.next(), Some("⏳ Working… running `cargo test`"));
    assert!(
        body.contains("\n· reading a.rs\n· searching fn main"),
        "oldest first"
    );
    let long = live_body(&[
        "s1".into(),
        "s2".into(),
        "s3".into(),
        "s4".into(),
        "s5".into(),
    ]);
    assert_eq!(
        long.lines()
            .filter(|l| l.starts_with("· "))
            .collect::<Vec<_>>(),
        vec!["· s2", "· s3", "· s4"],
        "current plus the three before it"
    );
    assert!(done_body(41_200, 6).starts_with("✅ Done in 41 s · 6 steps"));
    assert_eq!(done_body(200, 1), "✅ Done in 1 s · 1 step");
    assert!(clean("a\u{7}b").unwrap() == "a b" && clean("  ").is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_status_is_posted_once_edited_summarized_and_fenced() {
    use super::super::tests::{enqueue, fixture, running};
    let f = fixture().await;
    let t = &f.transport;
    let now = crate::api::now_ms();
    let lease = t
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let id = enqueue(&f, "progress-status").await;
    // Not running yet: no status.
    assert!(
        t.record_progress(&f.p, &lease, &id, "progress-run", &[], 0, &f.facts, now)
            .await
            .is_err()
    );
    running(&f, &lease, &id, "progress-run").await;
    // Another device, a foreign execution or an old lease epoch is refused.
    assert!(
        t.record_progress(
            &f.second,
            &lease,
            &id,
            "progress-run",
            &[],
            0,
            &f.facts,
            now
        )
        .await
        .is_err()
    );
    assert!(
        t.record_progress(&f.p, &lease, &id, "foreign-run", &[], 0, &f.facts, now)
            .await
            .is_err()
    );
    let stale = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch + 1,
    };
    assert!(
        t.record_progress(&f.p, &stale, &id, "progress-run", &[], 0, &f.facts, now)
            .await
            .is_err()
    );
    assert!(
        t.record_progress(
            &f.p,
            &lease,
            &id,
            "progress-run",
            &vec!["x".to_owned(); 6],
            6,
            &f.facts,
            now
        )
        .await
        .is_err()
    );

    // 1. "Working…" is created once and posted top-level, replying to the request.
    assert!(
        t.record_progress(&f.p, &lease, &id, "progress-run", &[], 0, &f.facts, now)
            .await
            .unwrap()
    );
    assert!(
        !t.record_progress(&f.p, &lease, &id, "progress-run", &[], 0, &f.facts, now)
            .await
            .unwrap()
    );
    let due = t.progress_candidates("", now).await.unwrap();
    let candidate = due
        .iter()
        .find(|s| s.thread_root == "$progress-status")
        .expect("status due");
    let first = t
        .claim_progress(&candidate.id, &f.facts, now)
        .await
        .unwrap();
    let (txn, content) = status_content(false, &first);
    assert_eq!(txn, first.matrix_txn_id);
    assert_eq!(content["msgtype"], "m.notice");
    assert!(content["body"].as_str().unwrap().starts_with("⏳ Working…"));
    assert_eq!(
        content["m.relates_to"],
        serde_json::json!({"m.in_reply_to":{"event_id":"$progress-status"}})
    );
    assert!(
        t.confirm_progress(
            &first.id,
            "wrong-worker",
            first.revision,
            &Ok("$status".into()),
            now
        )
        .await
        .is_err()
    );
    t.confirm_progress(
        &first.id,
        first.worker_token.as_deref().unwrap(),
        first.revision,
        &Ok("$status".into()),
        now,
    )
    .await
    .unwrap();

    // 2. A step edits the same event, no sooner than 1.5 s after the last send.
    let steps = vec![
        "reading src/main.rs".to_owned(),
        "running `cargo test`".to_owned(),
    ];
    assert!(
        t.record_progress(&f.p, &lease, &id, "progress-run", &steps, 2, &f.facts, now)
            .await
            .unwrap()
    );
    assert!(
        t.claim_progress(&first.id, &f.facts, now + 100)
            .await
            .is_err(),
        "throttled"
    );
    // The send was stamped with the database clock, which has moved past `now`.
    let later = crate::api::now_ms() + MIN_EDIT_INTERVAL_MS + 100;
    assert!(
        t.progress_candidates("", later)
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == first.id)
    );
    let edit = t.claim_progress(&first.id, &f.facts, later).await.unwrap();
    let (edit_txn, content) = status_content(false, &edit);
    assert_eq!(
        edit_txn,
        format!("{}-r{}", first.matrix_txn_id, edit.revision)
    );
    assert_eq!(
        content["m.relates_to"],
        serde_json::json!({"rel_type":"m.replace","event_id":"$status"})
    );
    assert_eq!(content["m.new_content"]["msgtype"], "m.notice");
    assert!(content["m.new_content"].get("m.relates_to").is_none());
    let new_body = content["m.new_content"]["body"].as_str().unwrap();
    assert!(
        new_body.starts_with("⏳ Working… running `cargo test`")
            && new_body.contains("· reading src/main.rs")
    );
    assert_eq!(content["body"].as_str().unwrap(), format!("* {new_body}"));
    t.confirm_progress(
        &edit.id,
        edit.worker_token.as_deref().unwrap(),
        edit.revision,
        &Ok("$edit1".into()),
        later,
    )
    .await
    .unwrap();

    // 3. The reply completes the dispatch: later reports are refused and the
    // status is edited once more to the collapsed summary.
    t.submit_reply(
        &f.p,
        &lease,
        SubmitReply {
            dispatch_id: id.clone(),
            execution_id: "progress-run".into(),
            body: "Answer".into(),
        },
        &f.facts,
        now,
    )
    .await
    .unwrap();
    assert!(
        t.record_progress(
            &f.p,
            &lease,
            &id,
            "progress-run",
            &steps,
            3,
            &f.facts,
            later
        )
        .await
        .is_err()
    );
    let end = later + MIN_EDIT_INTERVAL_MS + 100;
    assert!(
        t.progress_candidates("", end)
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == first.id)
    );
    let summary = t.claim_progress(&first.id, &f.facts, end).await.unwrap();
    let (_, content) = status_content(false, &summary);
    assert_eq!(content["m.relates_to"]["rel_type"], "m.replace");
    assert_eq!(content["m.relates_to"]["event_id"], "$status");
    let done = content["m.new_content"]["body"].as_str().unwrap();
    assert!(
        done.starts_with("✅ Done in ") && done.contains(" s · 2 steps"),
        "{done}"
    );
    t.confirm_progress(
        &summary.id,
        summary.worker_token.as_deref().unwrap(),
        summary.revision,
        &Ok("$edit2".into()),
        end,
    )
    .await
    .unwrap();
    assert!(
        t.claim_progress(&first.id, &f.facts, end + 10_000)
            .await
            .is_err(),
        "done is terminal"
    );

    // 4. A binding generation change fences new reports and cancels live statuses.
    let other = enqueue(&f, "progress-stale-generation").await;
    running(&f, &lease, &other, "stale-run").await;
    assert!(
        t.record_progress(&f.p, &lease, &other, "stale-run", &[], 0, &f.facts, now)
            .await
            .unwrap()
    );
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut raw = AsyncPgConnection::establish(&url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.bindings SET generation=generation+1 WHERE id=$1")
        .bind::<Text, _>(&f.binding)
        .execute(&mut raw)
        .await
        .unwrap();
    assert!(
        t.record_progress(&f.p, &lease, &other, "stale-run", &steps, 2, &f.facts, now)
            .await
            .is_err()
    );
    let after = t.progress_candidates("", end).await.unwrap();
    assert!(
        !after
            .iter()
            .any(|s| s.thread_root == "$progress-stale-generation")
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_denied_first_status_post_keeps_still_working_notice() {
    use super::super::tests::{enqueue, fixture, running};
    use diesel::sql_types::BigInt;
    let f = fixture().await;
    let t = &f.transport;
    let now = crate::api::now_ms();
    let lease = t
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let id = enqueue(&f, "progress-denied").await;
    running(&f, &lease, &id, "denied-run").await;
    assert!(
        t.record_progress(&f.p, &lease, &id, "denied-run", &[], 0, &f.facts, now)
            .await
            .unwrap()
    );
    let due = t.progress_candidates("", now).await.unwrap();
    let candidate = due
        .iter()
        .find(|s| s.thread_root == "$progress-denied")
        .expect("status due");
    let claim = t
        .claim_progress(&candidate.id, &f.facts, now)
        .await
        .unwrap();
    let denied = Err(Error::Conflict("matrix_permission_missing"));
    t.confirm_progress(
        &claim.id,
        claim.worker_token.as_deref().unwrap(),
        claim.revision,
        &denied,
        now,
    )
    .await
    .unwrap();
    {
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let mut raw = AsyncPgConnection::establish(&url).await.unwrap();
        sql_query("UPDATE hagency_agent_v1.owner_events SET created_at_ms=$1 WHERE id=$2")
            .bind::<BigInt, _>(now - 130_000)
            .bind::<Text, _>(&id)
            .execute(&mut raw)
            .await
            .unwrap();
        sql_query("UPDATE hagency_agent_v1.request_notice_deployment SET since_ms=$1")
            .bind::<BigInt, _>(now - 131_000)
            .execute(&mut raw)
            .await
            .unwrap();
    }
    let notices = t.request_notice_candidates("", now).await.unwrap();
    let notice = notices
        .iter()
        .find(|n| n.owner_event_id == id)
        .expect("fallback notice owed");
    assert_eq!(
        notice.kind, "processing",
        "a never-posted, cancelled status must not suppress Still working"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_late_report_for_an_earlier_step_does_not_regress_the_status() {
    use super::super::tests::{enqueue, fixture, running};
    let f = fixture().await;
    let t = &f.transport;
    let now = crate::api::now_ms();
    let lease = t
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let id = enqueue(&f, "progress-reordered").await;
    running(&f, &lease, &id, "reordered-run").await;
    let five: Vec<String> = (1..=5).map(|n| format!("step {n}")).collect();
    assert!(
        t.record_progress(&f.p, &lease, &id, "reordered-run", &five, 5, &f.facts, now)
            .await
            .unwrap()
    );
    // Step 4's report arrives after step 5's: ignored, step 5 stays shown.
    assert!(
        !t.record_progress(
            &f.p,
            &lease,
            &id,
            "reordered-run",
            &five[..4],
            4,
            &f.facts,
            now
        )
        .await
        .unwrap()
    );
    let due = t.progress_candidates("", now).await.unwrap();
    let status = due
        .iter()
        .find(|s| s.thread_root == "$progress-reordered")
        .expect("status due");
    assert_eq!(status.step_count, 5);
    assert!(
        body_of(status).starts_with("⏳ Working… step 5"),
        "{}",
        body_of(status)
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_a_turn_ending_mid_send_is_finalized_without_waiting_for_the_lease() {
    use super::super::tests::{enqueue, fixture, running};
    let f = fixture().await;
    let t = &f.transport;
    let now = crate::api::now_ms();
    let lease = t
        .acquire_for_test(&f.p, &f.agent, 60_000, false, now)
        .await
        .unwrap();
    let lease = LeaseRef {
        agent_id: f.agent.clone(),
        epoch: lease.epoch,
    };
    let id = enqueue(&f, "progress-mid-send").await;
    running(&f, &lease, &id, "mid-send-run").await;
    assert!(
        t.record_progress(&f.p, &lease, &id, "mid-send-run", &[], 0, &f.facts, now)
            .await
            .unwrap()
    );
    let due = t.progress_candidates("", now).await.unwrap();
    let first = t
        .claim_progress(
            &due.iter()
                .find(|s| s.thread_root == "$progress-mid-send")
                .unwrap()
                .id,
            &f.facts,
            now,
        )
        .await
        .unwrap();
    t.confirm_progress(
        &first.id,
        first.worker_token.as_deref().unwrap(),
        first.revision,
        &Ok("$status".into()),
        now,
    )
    .await
    .unwrap();
    let steps = vec!["reading a.rs".to_owned()];
    assert!(
        t.record_progress(&f.p, &lease, &id, "mid-send-run", &steps, 1, &f.facts, now)
            .await
            .unwrap()
    );
    let later = crate::api::now_ms() + MIN_EDIT_INTERVAL_MS + 100;
    let edit = t.claim_progress(&first.id, &f.facts, later).await.unwrap();
    // The reply lands while that edit is still in flight (lease held).
    t.submit_reply(
        &f.p,
        &lease,
        SubmitReply {
            dispatch_id: id.clone(),
            execution_id: "mid-send-run".into(),
            body: "Answer".into(),
        },
        &f.facts,
        now,
    )
    .await
    .unwrap();
    t.progress_candidates("", later).await.unwrap();
    // Confirming the superseded live revision leaves the summary owed, not done.
    t.confirm_progress(
        &edit.id,
        edit.worker_token.as_deref().unwrap(),
        edit.revision,
        &Ok("$edit".into()),
        later,
    )
    .await
    .unwrap();
    let end = later + MIN_EDIT_INTERVAL_MS + 100;
    let summary = t.claim_progress(&first.id, &f.facts, end).await.unwrap();
    assert!(summary.revision > edit.revision);
    assert!(
        body_of(&summary).starts_with("✅ Done in "),
        "{}",
        body_of(&summary)
    );
}
