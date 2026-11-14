use super::*;

fn status(event_id: &str, room_id: &str, thread_root: &str, sent: Option<&str>) -> ProgressStatus {
    ProgressStatus {
        id: "progress_a".into(),
        binding_id: "binding_a".into(),
        binding_generation: 1,
        execution_id: "run_a".into(),
        started_at_ms: 0,
        recent: json!(["running `cargo test`"]),
        step_count: 1,
        final_body: None,
        revision: 2,
        sent_revision: 1,
        matrix_txn_id: "hg-progress-a".into(),
        status_event_id: sent.map(str::to_owned),
        state: "live".into(),
        worker_token: None,
        requester_mxid: "@bob:test".into(),
        room_id: room_id.into(),
        event_id: event_id.into(),
        thread_root: thread_root.into(),
        event_state: "running".into(),
        event_execution_id: Some("run_a".into()),
        outcome: None,
        reply_at_ms: None,
    }
}

#[test]
fn top_level_request_gets_top_level_status_replying_to_it() {
    // Project request: its stored root is its own event id.
    let (txn, content) = status_content(false, &status("$req", "!room", "$req", None));
    assert_eq!(txn, "hg-progress-a");
    assert_eq!(content["msgtype"], "m.notice");
    assert_eq!(
        content["m.relates_to"],
        json!({"m.in_reply_to":{"event_id":"$req"}}),
        "no m.thread"
    );
    assert!(
        content["body"]
            .as_str()
            .unwrap()
            .contains("running `cargo test`")
    );
}

#[test]
fn owner_direct_top_level_request_replies_to_request() {
    let (_, content) = status_content(true, &status("$req", "!dm", "!dm", None));
    assert_eq!(
        content["m.relates_to"],
        json!({"m.in_reply_to":{"event_id":"$req"}})
    );
}

#[test]
fn threaded_request_keeps_status_in_its_thread() {
    let (_, content) = status_content(false, &status("$req", "!room", "$root", None));
    assert_eq!(content["m.relates_to"]["rel_type"], "m.thread");
    assert_eq!(content["m.relates_to"]["event_id"], "$root");
    assert_eq!(content["msgtype"], "m.notice");
}

#[test]
fn edit_replaces_original_in_place_without_moving_it() {
    for root in ["$req", "$root"] {
        let (txn, content) = status_content(false, &status("$req", "!room", root, Some("$status")));
        assert_eq!(txn, "hg-progress-a-r2");
        assert_eq!(
            content["m.relates_to"],
            json!({"rel_type":"m.replace","event_id":"$status"})
        );
        assert!(content["m.new_content"].get("m.relates_to").is_none());
        assert!(content["body"].as_str().unwrap().starts_with("* "));
    }
}

#[test]
fn rich_reply_without_thread_relation_stays_top_level() {
    // Regression (2026-11-13 ledger: a new request carried an old thread root):
    // only an explicit `m.thread` relation selects a thread; a plain rich reply
    // to an earlier threaded event is a new top-level request.
    let raw = json!({"type":"m.room.message","event_id":"$new","room_id":"!r:x","sender":"@u:x",
        "content":{"msgtype":"m.text","body":"again","m.mentions":{"user_ids":["@bot:x"]},
        "m.relates_to":{"m.in_reply_to":{"event_id":"$old-agent-reply"}}}});
    let parsed = crate::workers::parse_event(&raw).unwrap();
    assert_eq!(parsed.thread_root, None);
    assert!(
        crate::room_context::thread_of("$new", "!r:x", "$new").is_none(),
        "own-root is not a thread"
    );
    assert_eq!(
        crate::room_context::thread_of("$new", "!r:x", "$old"),
        Some("$old")
    );
}
