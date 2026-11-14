use crate::room_context::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const PUPPET: &str = "@_hagency_ann:example.org";
const ALICE: &str = "@alice:example.org";

fn text(id: &str, sender: &str, ts: i64, body: &str) -> Value {
    json!({"type":"m.room.message","event_id":id,"sender":sender,"origin_server_ts":ts,
        "content":{"msgtype":"m.text","body":body}})
}
fn edit(id: &str, sender: &str, ts: i64, target: &str, body: &str) -> Value {
    json!({"type":"m.room.message","event_id":id,"sender":sender,"origin_server_ts":ts,
        "content":{"msgtype":"m.text","body":format!("* {body}"),"m.new_content":{"msgtype":"m.text","body":body},
            "m.relates_to":{"rel_type":"m.replace","event_id":target}}})
}
fn run(events: &[Value], trigger: &str, from: i64, complete: bool) -> RoomContext {
    let names = BTreeMap::from([(ALICE.to_owned(), "Alice".to_owned())]);
    select(
        "room",
        &Timeline {
            events,
            trigger,
            visible_from_ms: from,
            names: &names,
            complete,
        },
    )
}
fn bodies(c: &RoomContext) -> Vec<&str> {
    c.messages.iter().map(|m| m.body.as_str()).collect()
}

#[test]
fn quotes_only_messages_before_the_trigger_oldest_first() {
    let events = [
        text("$later", ALICE, 50, "after the request"),
        text("$trigger", ALICE, 40, "@ann summarize this room so far"),
        text("$c", PUPPET, 30, "noted"),
        text("$b", ALICE, 20, "the launch codename is BLUE HERON"),
        text("$a", ALICE, 10, "kickoff"),
    ];
    let c = run(&events, "$trigger", 0, true);
    assert_eq!(
        bodies(&c),
        ["kickoff", "the launch codename is BLUE HERON", "noted"]
    );
    assert_eq!(c.messages[0].sender_display_name.as_deref(), Some("Alice"));
    assert_eq!(c.messages[2].sender_display_name, None);
    assert!(!c.omitted_older && c.unavailable.is_none());
    assert!(
        c.messages
            .iter()
            .all(|m| m.event_id != "$trigger" && m.event_id != "$later")
    );
}

#[test]
fn nothing_before_the_puppet_membership_is_visible() {
    let events = [
        text("$t", ALICE, 40, "summarize"),
        text("$after", ALICE, 30, "visible"),
        text("$before", ALICE, 10, "secret before join"),
    ];
    assert_eq!(bodies(&run(&events, "$t", 20, true)), ["visible"]);
}

#[test]
fn redactions_edits_duplicates_and_non_text_are_handled() {
    let events = [
        text("$t", ALICE, 90, "summarize"),
        edit("$e2", "@mallory:example.org", 80, "$a", "forged edit"),
        edit("$e1", ALICE, 70, "$a", "plan B is final"),
        json!({"type":"m.room.message","event_id":"$gone","sender":ALICE,"origin_server_ts":60,"content":{},
            "unsigned":{"redacted_because":{"type":"m.room.redaction"}}}),
        json!({"type":"m.reaction","event_id":"$r","sender":ALICE,"origin_server_ts":55,"content":{"m.relates_to":{}}}),
        json!({"type":"m.room.message","event_id":"$img","sender":ALICE,"origin_server_ts":52,"content":{"msgtype":"m.image","body":"cat.png"}}),
        text(
            "$b",
            ALICE,
            50,
            "> <@bob:example.org> earlier\n\nreply text",
        ),
        text(
            "$b",
            ALICE,
            50,
            "> <@bob:example.org> earlier\n\nreply text",
        ),
        text("$a", ALICE, 40, "plan A"),
    ];
    let c = run(&events, "$t", 0, true);
    assert_eq!(bodies(&c), ["plan B is final", "reply text"]);
    assert!(c.messages[0].edited && !c.messages[1].edited);
}

#[test]
fn budget_keeps_the_newest_and_reports_omission() {
    let mut events = vec![text("$t", ALICE, 10_000, "summarize")];
    for i in (0..80).rev() {
        events.push(text(&format!("$m{i}"), ALICE, i, &format!("message {i}")));
    }
    let c = run(&events, "$t", 0, true);
    assert_eq!(c.messages.len(), MAX_CONTEXT_MESSAGES);
    assert_eq!(c.messages.last().unwrap().body, "message 79");
    assert_eq!(c.messages[0].body, "message 30");
    assert!(c.omitted_older);

    let big = "x".repeat(MAX_MESSAGE_BYTES * 3);
    let mut events = vec![text("$t", ALICE, 10_000, "summarize")];
    for i in (0..20).rev() {
        events.push(text(&format!("$b{i}"), ALICE, i, &big));
    }
    let c = run(&events, "$t", 0, true);
    let total: usize = c
        .messages
        .iter()
        .map(|m| m.body.len() + m.sender.len())
        .sum();
    assert!(total <= MAX_CONTEXT_BYTES && c.omitted_older && !c.messages.is_empty());
    assert!(
        c.messages
            .iter()
            .all(|m| m.body.len() <= MAX_MESSAGE_BYTES && m.body.ends_with('…'))
    );
}

#[test]
fn incomplete_scan_or_missing_trigger_is_reported() {
    let events = [
        text("$t", ALICE, 20, "summarize"),
        text("$a", ALICE, 10, "a"),
    ];
    assert!(run(&events, "$t", 0, false).omitted_older);
    let missing = run(&events, "$absent", 0, true);
    assert!(missing.messages.is_empty() && missing.omitted_older);
}

#[test]
fn thread_versus_room_selection() {
    let room = "!r:example.org";
    assert_eq!(thread_of("$req", room, "$root"), Some("$root"));
    assert_eq!(
        thread_of("$req", room, "$req"),
        None,
        "a top-level Project message roots itself"
    );
    assert_eq!(
        thread_of("$req", room, room),
        None,
        "owner DMs use the Room ID"
    );
}

#[test]
fn membership_reads_names_and_join_time() {
    let state = [
        json!({"type":"m.room.member","state_key":PUPPET,"origin_server_ts":77,"content":{"membership":"join","displayname":"ann4agent"}}),
        json!({"type":"m.room.member","state_key":ALICE,"origin_server_ts":5,"content":{"membership":"join","displayname":"Alice"}}),
        json!({"type":"m.room.name","state_key":"","content":{"name":"haws"}}),
    ];
    let (names, joined) = membership(&state, PUPPET);
    assert_eq!(joined, Some(77));
    assert_eq!(names.get(ALICE).map(String::as_str), Some("Alice"));
    let invited = [
        json!({"type":"m.room.member","state_key":PUPPET,"origin_server_ts":9,"content":{"membership":"invite"}}),
    ];
    assert_eq!(membership(&invited, PUPPET).1, None);
}

#[test]
fn encrypted_rooms_send_no_text_and_say_why() {
    let c = RoomContext::unavailable("room", UNAVAILABLE_ENCRYPTED);
    assert_eq!(
        serde_json::to_value(&c).unwrap(),
        json!({"scope":"room","messages":[],"omittedOlder":false,"unavailable":"encrypted"})
    );
}

#[test]
fn wire_shape_is_camel_case_and_omits_empty_optionals() {
    let events = [
        text("$t", ALICE, 20, "summarize"),
        text("$a", PUPPET, 10, "hi"),
    ];
    let value = serde_json::to_value(run(&events, "$t", 0, true)).unwrap();
    assert_eq!(
        value,
        json!({"scope":"room","omittedOlder":false,"messages":[
            {"eventId":"$a","sender":PUPPET,"body":"hi","originServerTs":10}]})
    );
}

fn facts(encrypted: bool) -> crate::transport::DeliveryFacts {
    crate::transport::DeliveryFacts {
        owner_mxid: ALICE.into(),
        requester_mxid: ALICE.into(),
        puppet_mxid: PUPPET.into(),
        room_id: "!r:example.org".into(),
        space_id: String::new(),
        observed_at_ms: 1,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        requester_in_room: true,
        puppet_in_room: true,
        puppet_can_send_message: true,
        encrypted,
        owner_direct_valid: true,
        requires_mention: true,
    }
}

#[tokio::test]
async fn dispatch_context_flags_encrypted_and_unreadable_rooms() {
    let read: crate::gateway::ReadState =
        std::sync::Arc::new(|_| Box::pin(async { Ok(Vec::new()) }));
    let gateway = crate::gateway::Gateway::new(read, "@_hagency_service:example.org".into());
    let c = for_dispatch(None, &gateway, &facts(true), "$req", "$root").await;
    assert_eq!(
        (c.scope, c.unavailable, c.messages.len()),
        ("thread", Some(UNAVAILABLE_ENCRYPTED), 0)
    );
    let c = for_dispatch(None, &gateway, &facts(false), "$req", "$req").await;
    assert_eq!(
        (c.scope, c.unavailable),
        ("room", Some(UNAVAILABLE_UNREADABLE))
    );
}
