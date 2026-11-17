//! A hosted Agent's requester (its recorded Room human) talks to it like an
//! owner in a private pair Room, and can invite it into other Rooms.
use super::tests::{Fixture, fixture};
use super::*;
use diesel::sql_types::Text;

const HUMAN: &str = "@requester:example.test";

async fn record_human(f: &Fixture, human: &str) {
    sql_query("UPDATE hagency_agent_v1.bindings SET room_human_mxid=$2 WHERE id=$1")
        .bind::<Text, _>(&f.binding).bind::<Text, _>(human)
        .execute(&mut *f.transport.db.lock().await.unwrap()).await.unwrap();
}
fn plain(f: &Fixture, id: &str, sender: &str) -> RoutedEvent {
    RoutedEvent {
        event_id: format!("${id}"), room_id: f.facts.room_id.clone(), sender_mxid: sender.into(),
        body: "hello".into(), mentioned_mxids: BTreeSet::new(), thread_root: None, encrypted: false, is_edit: false,
    }
}
fn pair_facts(f: &Fixture, sender: &str) -> DeliveryFacts {
    let mut facts = f.facts.clone();
    facts.requester_mxid = sender.into();
    facts.requires_mention = false;
    facts
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_room_human_plain_message_wakes_agent_only_in_pair_room() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let ingest = |id: &'static str, sender: &'static str, facts: DeliveryFacts| {
        let event = plain(&f, id, sender);
        let t = &f.transport;
        let b = f.binding.clone();
        async move { t.ingest_routed(&b, event, &facts, now).await.unwrap() }
    };
    // No recorded Room human: a plain message from the requester is ignored.
    assert!(matches!(ingest("rh_before", HUMAN, pair_facts(&f, HUMAN)).await, RouteResult::Ignored));
    record_human(&f, HUMAN).await;
    assert!(matches!(ingest("rh_pair", HUMAN, pair_facts(&f, HUMAN)).await, RouteResult::Queued { .. }));
    // A second human in the Room (gateway saw no pair): mention required again.
    let mut crowded = pair_facts(&f, HUMAN);
    crowded.requires_mention = true;
    assert!(matches!(ingest("rh_crowded", HUMAN, crowded).await, RouteResult::Ignored));
    // Someone else in a pair Room is never the recorded human.
    let other = "@other:example.test";
    assert!(matches!(ingest("rh_other", other, pair_facts(&f, other)).await, RouteResult::Ignored));
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_puppet_invite_from_room_human_is_listed_for_owner_only() {
    let f = fixture().await;
    let now = crate::api::now_ms();
    let invite = |inviter: &str, room: &str| PuppetInviteEvent {
        room_id: room.into(), inviter_mxid: inviter.into(), puppet_mxid: f.facts.puppet_mxid.clone(),
    };
    let room = format!("!invite_{}:example.test", crate::secret_token());
    // Before a Room human is recorded, nobody's invite is kept.
    assert!(!f.transport.record_puppet_invite(&invite(HUMAN, &room), now).await.unwrap());
    record_human(&f, HUMAN).await;
    assert!(!f.transport.record_puppet_invite(&invite("@stranger:example.test", &room), now).await.unwrap());
    assert!(f.transport.record_puppet_invite(&invite(HUMAN, &room), now).await.unwrap());
    let listed = f.transport.puppet_invites(&f.p, &f.agent, now).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].room_id.as_str(), listed[0].inviter_mxid.as_str()), (room.as_str(), HUMAN));
    // Expired invites are not offered.
    let stale = format!("!stale_{}:example.test", crate::secret_token());
    let old = now - puppet_invites::PUPPET_INVITE_TTL_MS - 1;
    assert!(f.transport.record_puppet_invite(&invite(HUMAN, &stale), old).await.unwrap());
    // An invite into a Room the Agent is already bound to is not offered.
    let bound = f.facts.room_id.clone();
    assert!(f.transport.record_puppet_invite(&invite(HUMAN, &bound), now).await.unwrap());
    let rooms: Vec<String> = f.transport.puppet_invites(&f.p, &f.agent, now).await.unwrap().into_iter().map(|i| i.room_id).collect();
    assert_eq!(rooms, vec![room]);
    // Replacing the Room human withdraws the old human's invites.
    record_human(&f, "@successor:example.test").await;
    assert!(f.transport.puppet_invites(&f.p, &f.agent, now).await.unwrap().is_empty());
}

#[test]
fn parses_only_member_invites_of_another_user() {
    let raw = |kind: &str, membership: &str, sender: &str| serde_json::json!({
        "type": kind, "room_id": "!r:x", "sender": sender, "state_key": "@_hagency_a:x",
        "content": {"membership": membership}});
    assert_eq!(
        parse_puppet_invite(&raw("m.room.member", "invite", "@h:x")),
        Some(PuppetInviteEvent { room_id: "!r:x".into(), inviter_mxid: "@h:x".into(), puppet_mxid: "@_hagency_a:x".into() })
    );
    assert_eq!(parse_puppet_invite(&raw("m.room.member", "join", "@h:x")), None);
    assert_eq!(parse_puppet_invite(&raw("m.room.message", "invite", "@h:x")), None);
    assert_eq!(parse_puppet_invite(&raw("m.room.member", "invite", "@_hagency_a:x")), None);
    assert_eq!(parse_puppet_invite(&raw("m.room.member", "invite", "not-a-user")), None);
}

fn gateway(extra: &'static [&'static str]) -> crate::gateway::Gateway {
    let read: crate::gateway::ReadState = std::sync::Arc::new(move |room: String| Box::pin(async move {
        let member = |who: &str| serde_json::json!({"type":"m.room.member","state_key":who,"content":{"membership":"join"}});
        let mut state = vec![member("@owner:t"), member("@agent:t"), member("@service:t")];
        if room.starts_with("!room") {
            state.push(member("@human:t"));
            state.extend(extra.iter().map(|who| member(who)));
        } else {
            state.push(serde_json::json!({"type":"m.space.child","state_key":"!room:t","content":{"via":["t"]}}));
        }
        Ok(state)
    }));
    crate::gateway::Gateway::new(read, "@service:t".into())
}

#[tokio::test]
async fn gateway_sees_pair_room_for_a_lone_human_but_not_with_a_second_one() {
    for space in ["", "!space:t"] {
        let pair = gateway(&[]).delivery("!room:t", space, "@owner:t", "@human:t", "@agent:t").await.unwrap();
        assert!(!pair.requires_mention, "lone human, space {space:?}");
        let crowded = gateway(&["@guest:t"]).delivery("!room:t", space, "@owner:t", "@human:t", "@agent:t").await.unwrap();
        assert!(crowded.requires_mention, "second human, space {space:?}");
    }
    // The owner's own plain messages keep the original rule in a Project Room.
    let owner = gateway(&[]).delivery("!room:t", "!space:t", "@owner:t", "@owner:t", "@agent:t").await.unwrap();
    assert!(owner.requires_mention);
    // Shared (pool) delivery never relaxes the mention rule for the human.
    let pool = gateway(&[]).shared_delivery("!room:t", "", "@owner:t", "@human:t", "@agent:t").await.unwrap();
    assert!(pool.requires_mention);
}
