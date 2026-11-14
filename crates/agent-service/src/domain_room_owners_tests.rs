//! ADR 0001: several owners on one server register the same room, each in
//! their own project, and bind their own Agents there. Room admins keep
//! control of every owner's Agents in the room (pause, policy).
use super::*;

struct Owner {
    p: Principal,
    project: String,
    space: String,
}

async fn owner(
    auth: &Store,
    store: &DomainStore,
    suffix: &str,
    name: &str,
    room: &str,
    room_admin: bool,
) -> Owner {
    let (_, p) = user(auth, suffix, name).await;
    let space = format!("!{name}_space_{suffix}:example.test");
    let project = store
        .register_project(&p, &space, &admin(&p, &space, true, None), *NOW)
        .await
        .unwrap();
    let mut in_room = admin(&p, room, false, Some(&space));
    in_room.can_manage_policy = room_admin;
    store
        .register_room(
            &p,
            &project.id,
            room,
            &admin(&p, &space, true, None),
            &in_room,
            *NOW,
        )
        .await
        .unwrap();
    Owner {
        p,
        project: project.id,
        space,
    }
}

fn bind(o: &Owner, room: &str, key: &str) -> CreateBoundAgent {
    CreateBoundAgent {
        project_id: o.project.clone(),
        room_id: room.into(),
        display_name: format!("{key} agent"),
        idempotency_key: key.into(),
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_two_owners_bind_agents_into_one_room_and_room_admins_control_both() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let room = format!("!shared_room_{suffix}:example.test");
    // Ann administers the room; Ben and Webster are plain members.
    let ann = owner(&auth, &store, &suffix, "ann", &room, true).await;
    let ben = owner(&auth, &store, &suffix, "ben", &room, false).await;
    let web = owner(&auth, &store, &suffix, "web", &room, false).await;

    let ben_agent = store
        .create_bound_agent(
            &ben.p,
            bind(&ben, &room, "ben-key"),
            &facts(&ben.p, &ben.space, &room, None),
            *NOW,
        )
        .await
        .unwrap();
    let web_agent = store
        .create_bound_agent(
            &web.p,
            bind(&web, &room, "web-key"),
            &facts(&web.p, &web.space, &room, None),
            *NOW,
        )
        .await
        .unwrap();
    assert_eq!(
        ben_agent.binding.project_id.as_deref(),
        Some(ben.project.as_str())
    );
    assert_eq!(
        web_agent.binding.project_id.as_deref(),
        Some(web.project.as_str())
    );

    // Another owner's project is still off limits: Webster is not in Ben's Space.
    let mut outsider = facts(&web.p, &ben.space, &room, None);
    outsider.owner_in_space = false;
    assert!(matches!(
        store
            .create_bound_agent(&web.p, bind(&ben, &room, "web-in-ben"), &outsider, *NOW)
            .await,
        Err(Error::Unauthorized("room_membership_required"))
    ));

    // A room admin's pause suspends every owner's Agents in the room.
    let ann_room = admin(&ann.p, &room, false, Some(&ann.space));
    assert_eq!(
        store
            .suspend_room_bindings(&ann.p, &ann.project, &room, &ann_room, *NOW)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .clear_room_pause(&ann.p, &ann.project, &room, &ann_room, *NOW)
            .await
            .unwrap(),
        2
    );

    // A room admin's policy covers every owner's registration of the room.
    let revision = store
        .service_state(&ann.p, &ann.project, Some(&room), *NOW)
        .await
        .unwrap()["revision"]
        .as_i64()
        .unwrap();
    store
        .set_room_policy(
            &ann.p,
            &room,
            &ann.project,
            revision,
            RoomCreationPolicy::Disabled,
            &ann_room,
            *NOW,
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .create_bound_agent(
                &web.p,
                bind(&web, &room, "web-after-disable"),
                &facts(&web.p, &web.space, &room, None),
                *NOW
            )
            .await,
        Err(Error::Unauthorized("agent_creation_denied"))
    ));
    // A later owner's registration inherits the room's policy.
    let late = owner(&auth, &store, &suffix, "late", &room, false).await;
    assert!(
        store
            .create_bound_agent(
                &late.p,
                bind(&late, &room, "late-key"),
                &facts(&late.p, &late.space, &room, None),
                *NOW
            )
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_registering_a_room_needs_a_joined_member_not_room_power() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let room = format!("!member_room_{suffix}:example.test");
    let (_, p) = user(&auth, &suffix, "nonmember").await;
    let space = format!("!nonmember_space_{suffix}:example.test");
    let project = store
        .register_project(&p, &space, &admin(&p, &space, true, None), *NOW)
        .await
        .unwrap();
    let mut outside = admin(&p, &room, false, Some(&space));
    outside.joined = false;
    outside.can_manage_policy = false;
    assert!(matches!(
        store
            .register_room(
                &p,
                &project.id,
                &room,
                &admin(&p, &space, true, None),
                &outside,
                *NOW
            )
            .await,
        Err(Error::Unauthorized("room_membership_required"))
    ));
    outside.joined = true;
    store
        .register_room(
            &p,
            &project.id,
            &room,
            &admin(&p, &space, true, None),
            &outside,
            *NOW,
        )
        .await
        .unwrap();
    // Registering again in the same project is idempotent.
    store
        .register_room(
            &p,
            &project.id,
            &room,
            &admin(&p, &space, true, None),
            &outside,
            *NOW,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_an_owner_leaving_retires_only_their_agents_and_rejoining_does_not_revive_them() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let room = format!("!follow_room_{suffix}:example.test");
    let ben = owner(&auth, &store, &suffix, "ben", &room, false).await;
    let web = owner(&auth, &store, &suffix, "web", &room, false).await;
    let ben_agent = store
        .create_bound_agent(
            &ben.p,
            bind(&ben, &room, "ben-follow"),
            &facts(&ben.p, &ben.space, &room, None),
            *NOW,
        )
        .await
        .unwrap();
    let web_agent = store
        .create_bound_agent(
            &web.p,
            bind(&web, &room, "web-follow"),
            &facts(&web.p, &web.space, &room, None),
            *NOW,
        )
        .await
        .unwrap();
    let observe = |o: &Owner, a: &AgentBinding, owner_in_room: bool| {
        let mut f = facts(&o.p, &o.space, &room, Some(&a.agent.puppet_mxid));
        f.owner_in_room = owner_in_room;
        f
    };
    // Ben is kicked: his Agent's binding goes to leaving; Webster's stays.
    assert!(
        store
            .retire_membership_loss_trusted(
                &ben_agent.binding.id,
                ben_agent.binding.generation,
                &observe(&ben, &ben_agent, false),
                *NOW
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .retire_membership_loss_trusted(
                &web_agent.binding.id,
                web_agent.binding.generation,
                &observe(&web, &web_agent, true),
                *NOW
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .departure_desired_trusted(&ben_agent.binding.id, ben_agent.binding.generation + 1)
            .await
            .unwrap()
    );
    assert!(
        !store
            .departure_desired_trusted(&web_agent.binding.id, web_agent.binding.generation)
            .await
            .unwrap()
    );
    // Ben rejoins: nothing brings the Agent back by itself; the old observation
    // with Ben present again does not reactivate the leaving binding.
    assert!(
        store
            .retire_membership_loss_trusted(
                &ben_agent.binding.id,
                ben_agent.binding.generation + 1,
                &observe(&ben, &ben_agent, true),
                *NOW
            )
            .await
            .is_ok()
    );
    assert!(
        store
            .departure_desired_trusted(&ben_agent.binding.id, ben_agent.binding.generation + 1)
            .await
            .unwrap()
    );
}
