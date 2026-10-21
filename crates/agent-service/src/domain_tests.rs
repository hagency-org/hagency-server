use super::*;
use crate::{identity::Identity, secret_token, store::Store};
// Tests needing a discussion scope explicitly issue two independent commands.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateBoundAgent {
    pub project_id: String,
    pub room_id: String,
    pub display_name: String,
    pub idempotency_key: String,
}
impl DomainStore {
    pub(crate) async fn create_bound_agent(
        &self,
        p: &Principal,
        input: CreateBoundAgent,
        f: &RoomFacts,
        now: i64,
    ) -> Result<AgentBinding> {
        let agent = self
            .create_agent(
                p,
                CreateAgent {
                    visibility: crate::domain::AgentVisibility::Public,
                    display_name: input.display_name,
                    idempotency_key: input.idempotency_key.clone(),
                },
                now,
            )
            .await?;
        self.bind_room(
            p,
            &agent.id,
            BindRoom {
                project_id: input.project_id,
                room_id: input.room_id,
                idempotency_key: input.idempotency_key,
            },
            f,
            now,
        )
        .await
    }
}
static NOW: std::sync::LazyLock<i64> = std::sync::LazyLock::new(crate::api::now_ms);
fn admin(p: &Principal, room: &str, space: bool, parent: Option<&str>) -> AdminFacts {
    AdminFacts {
        actor_mxid: p.mxid.clone(),
        room_id: room.into(),
        observed_at_ms: *NOW,
        joined: true,
        can_manage_policy: true,
        is_space: space,
        linked_space_id: parent.map(str::to_owned),
    }
}
fn facts(p: &Principal, space: &str, room: &str, puppet: Option<&str>) -> RoomFacts {
    RoomFacts {
        owner_direct_valid: false,
        owner_mxid: p.mxid.clone(),
        room_id: room.into(),
        space_id: space.into(),
        observed_at_ms: *NOW,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        service_can_invite: true,
        puppet_mxid: puppet.map(str::to_owned),
        puppet_in_room: puppet.is_some(),
        encrypted: false,
    }
}
async fn user(store: &Store, suffix: &str, name: &str) -> (String, Principal) {
    let grant = store
        .sign_in(
            Identity {
                issuer: "https://example.test/auth/".into(),
                subject: format!("{name}-{suffix}"),
                mxid: format!("@{name}_{suffix}:example.test"),
                client_id: "client-test".into(),
                valid_until_ms: *NOW + 1_000_000,
            },
            *NOW,
        )
        .await
        .unwrap();
    let p = store.authenticate(&grant.token, *NOW, false).await.unwrap();
    let device = store
        .register_device(
            &grant.token,
            crate::store::RegisterDevice {
                installation_id: format!("create-{}", p.user_id),
                name: "Creation device".into(),
            },
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let p = store
        .authenticate(&device.token, crate::api::now_ms(), true)
        .await
        .unwrap();
    (grant.token, p)
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_domain_ownership_policy_isolation_and_lifecycle() {
    let url =
        std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test database required");
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (credential, p) = user(&auth, &suffix, "alice").await;
    let (_, other) = user(&auth, &suffix, "bob").await;
    let space1 = format!("!space1_{suffix}:example.test");
    let space2 = format!("!space2_{suffix}:example.test");
    let room1 = format!("!room1_{suffix}:example.test");
    let room2 = format!("!room2_{suffix}:example.test");
    let admin1 = admin(&p, &space1, true, None);
    let admin2 = admin(&p, &space2, true, None);
    let project1 = store
        .register_project(&p, &space1, &admin1, *NOW)
        .await
        .unwrap();
    let project2 = store
        .register_project(&p, &space2, &admin2, *NOW)
        .await
        .unwrap();
    assert_eq!(
        store
            .register_project(&p, &space1, &admin1, *NOW)
            .await
            .unwrap()
            .id,
        project1.id
    );
    store
        .register_room(
            &p,
            &project1.id,
            &room1,
            &admin1,
            &admin(&p, &room1, false, Some(&space1)),
            *NOW,
        )
        .await
        .unwrap();
    store
        .register_room(
            &p,
            &project2.id,
            &room2,
            &admin2,
            &admin(&p, &room2, false, Some(&space2)),
            *NOW,
        )
        .await
        .unwrap();
    let request = || CreateBoundAgent {
        project_id: project1.id.clone(),
        room_id: room1.clone(),
        display_name: "Codex".into(),
        idempotency_key: "create-test".into(),
    };
    let f1 = facts(&p, &space1, &room1, None);
    let created = store
        .create_bound_agent(&p, request(), &f1, *NOW)
        .await
        .unwrap();
    assert_eq!(
        store
            .create_bound_agent(&p, request(), &f1, *NOW)
            .await
            .unwrap()
            .agent
            .id,
        created.agent.id
    );
    assert_eq!(
        store
            .command_status(&p, "agent.create", "create-test", *NOW)
            .await
            .unwrap()
            .agent
            .id,
        created.agent.id
    );
    assert!(
        store
            .command_status(&other, "agent.create", "create-test", *NOW)
            .await
            .is_err()
    );
    assert!(
        store
            .command_status(&p, "agent.transfer", "create-test", *NOW)
            .await
            .is_err()
    );
    let mut changed = request();
    changed.display_name = "Other".into();
    assert!(
        store
            .create_bound_agent(&p, changed, &f1, *NOW)
            .await
            .is_err()
    );
    assert!(store.agent(&other, &created.agent.id, *NOW).await.is_err());
    let joined1 = facts(&p, &space1, &room1, Some(&created.agent.puppet_mxid));
    store
        .activate_binding(
            &p,
            &created.binding.id,
            created.binding.generation,
            &joined1,
            *NOW,
        )
        .await
        .unwrap();
    let joined2 = facts(&p, &space2, &room2, Some(&created.agent.puppet_mxid));
    let second = store
        .bind_room(
            &p,
            &created.agent.id,
            BindRoom {
                project_id: project2.id.clone(),
                room_id: room2.clone(),
                idempotency_key: "bind-test".into(),
            },
            &joined2,
            *NOW,
        )
        .await
        .unwrap();
    store
        .activate_binding(
            &p,
            &second.binding.id,
            second.binding.generation,
            &joined2,
            *NOW,
        )
        .await
        .unwrap();
    let denied = CreationPolicy {
        default_allow: true,
        allow: [p.mxid.clone()].into(),
        deny: [p.mxid.clone()].into(),
    };
    store
        .set_project_policy(&p, &project1.id, 1, denied, &admin1, *NOW)
        .await
        .unwrap();
    assert!(
        store
            .set_project_policy(
                &p,
                &project1.id,
                1,
                CreationPolicy::default(),
                &admin1,
                *NOW
            )
            .await
            .is_err()
    );
    // Creation prohibition is not runtime revocation.
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined1, *NOW)
            .await
            .is_ok()
    );
    let mut fresh_request = request();
    fresh_request.idempotency_key = "new-denied".into();
    assert!(
        store
            .create_bound_agent(&p, fresh_request, &f1, *NOW)
            .await
            .is_err()
    );
    store
        .set_room_policy(
            &p,
            &room1,
            &project1.id,
            1,
            RoomCreationPolicy::Disabled,
            &admin(&p, &room1, false, Some(&space1)),
            *NOW,
        )
        .await
        .unwrap();
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined1, *NOW)
            .await
            .is_ok()
    );
    store
        .suspend_project_bindings(&p, &project1.id, &admin1, *NOW)
        .await
        .unwrap();
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined1, *NOW)
            .await
            .is_err()
    );
    assert!(
        store
            .resume_binding(&p, &created.binding.id, &joined1, *NOW)
            .await
            .is_err()
    );
    assert!(
        store
            .runnable(&p, &second.binding.id, &joined2, *NOW)
            .await
            .is_ok()
    );
    assert!(
        store
            .activate_binding(
                &p,
                &created.binding.id,
                created.binding.generation,
                &joined1,
                *NOW
            )
            .await
            .is_err()
    );
    // Independent pause sources cannot be cleared by the other administrator scope.
    store
        .suspend_room_bindings(
            &p,
            &project1.id,
            &room1,
            &admin(&p, &room1, false, Some(&space1)),
            *NOW,
        )
        .await
        .unwrap();
    store
        .clear_project_pause(&p, &project1.id, &admin1, *NOW)
        .await
        .unwrap();
    assert!(
        store
            .resume_binding(&p, &created.binding.id, &joined1, *NOW)
            .await
            .is_err()
    );
    store
        .clear_room_pause(
            &p,
            &project1.id,
            &room1,
            &admin(&p, &room1, false, Some(&space1)),
            *NOW,
        )
        .await
        .unwrap();
    store
        .resume_binding(&p, &created.binding.id, &joined1, *NOW)
        .await
        .unwrap();
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined1, *NOW)
            .await
            .is_ok()
    );
    let paused = store
        .suspend_agent(&p, &created.agent.id, *NOW)
        .await
        .unwrap();
    assert_eq!(paused.state, "suspended");
    assert!(
        store
            .runnable(&p, &second.binding.id, &joined2, *NOW)
            .await
            .is_err()
    );
    store
        .resume_agent(&p, &created.agent.id, *NOW)
        .await
        .unwrap();
    assert!(
        store
            .runnable(&p, &second.binding.id, &joined2, *NOW)
            .await
            .is_ok()
    );
    let reopened = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert_eq!(
        reopened
            .create_bound_agent(&p, request(), &f1, *NOW)
            .await
            .unwrap()
            .agent
            .id,
        created.agent.id
    );
    assert!(
        DomainStore::open(&url, "example.test", "_hagency_other_")
            .await
            .is_err()
    );
    let mut db = store.db.lock().await.unwrap();
    assert!(
        sql_query("UPDATE hagency_agent_v1.agents SET owner_user_id=$1 WHERE id=$2")
            .bind::<Text, _>(&other.user_id)
            .bind::<Text, _>(&created.agent.id)
            .execute(&mut *db)
            .await
            .is_err()
    );
    assert!(
        sql_query(
            "UPDATE hagency_agent_v1.agents SET puppet_mxid='@stolen:example.test' WHERE id=$1"
        )
        .bind::<Text, _>(&created.agent.id)
        .execute(&mut *db)
        .await
        .is_err()
    );
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.agents WHERE id=$1")
            .bind::<Text, _>(&created.agent.id)
            .execute(&mut *db)
            .await
            .is_err()
    );
    drop(db);
    let retired = store
        .retire_agent(&p, &created.agent.id, *NOW)
        .await
        .unwrap();
    assert_eq!(retired.owner_user_id, p.user_id);
    assert_eq!(
        store
            .retire_agent(&p, &created.agent.id, *NOW)
            .await
            .unwrap()
            .generation,
        retired.generation
    );
    assert!(
        store
            .runnable(&p, &second.binding.id, &joined2, *NOW)
            .await
            .is_err()
    );
    assert!(
        store
            .retire_agent(&other, &created.agent.id, *NOW)
            .await
            .is_err()
    );
    assert!(
        store
            .confirm_retired(&p, &created.agent.id, retired.generation, *NOW)
            .await
            .is_err()
    );
    for binding in store.bindings(&p, &created.agent.id, *NOW).await.unwrap() {
        let mut f = if binding.room_id == room1 {
            joined1.clone()
        } else {
            joined2.clone()
        };
        f.puppet_in_room = false;
        store
            .confirm_left(&p, &binding.id, binding.generation, &f, *NOW)
            .await
            .unwrap();
    }
    let final_agent = store
        .confirm_retired(&p, &created.agent.id, retired.generation, *NOW)
        .await
        .unwrap();
    assert_eq!(final_agent.state, "retired");
    assert!(
        store
            .bind_room(
                &p,
                &created.agent.id,
                BindRoom {
                    project_id: project2.id.clone(),
                    room_id: room2,
                    idempotency_key: "after-retire".into()
                },
                &joined2,
                *NOW
            )
            .await
            .is_err()
    );
    auth.sign_out(&credential).await.unwrap();
    assert!(store.agent(&p, &created.agent.id, *NOW).await.is_err());
}
#[test]
fn request_contract_rejects_owner_and_resource_fields() {
    assert!(serde_json::from_value::<CreateAgent>(serde_json::json!({"projectId":"p","roomId":"!r:s","displayName":"a","idempotencyKey":"key","ownerUserId":"other"})).is_err());
    assert!(serde_json::from_value::<BindRoom>(serde_json::json!({"projectId":"p","roomId":"!r:s","idempotencyKey":"key","allocationId":"old"})).is_err());
}
#[test]
fn policy_deny_priority_and_trusted_observation_requirements() {
    let p = Principal {
        user_id: "u".into(),
        subject: "subject".into(),
        mxid: "@a:s".into(),
        session_id: "session".into(),
        client_id: "client".into(),
        valid_until_ms: *NOW + 100,
        device_id: None,
        device_generation: None,
    };
    let policy = CreationPolicy {
        default_allow: false,
        allow: [p.mxid.clone()].into(),
        deny: BTreeSet::new(),
    };
    let rp = RoomCreationPolicy::default();
    let mut f = facts(&p, "!space:s", "!room:s", None);
    assert!(creating(&p, &f, "!room:s", "!space:s", &policy, &rp, None, *NOW).is_ok());
    let mut deny = policy.clone();
    deny.deny.insert(p.mxid.clone());
    assert!(creating(&p, &f, "!room:s", "!space:s", &deny, &rp, None, *NOW).is_err());
    f.owner_in_space = false;
    assert!(creating(&p, &f, "!room:s", "!space:s", &policy, &rp, None, *NOW).is_err());
    f.owner_in_space = true;
    f.owner_in_room = false;
    assert!(creating(&p, &f, "!room:s", "!space:s", &policy, &rp, None, *NOW).is_err());
    f.owner_in_room = true;
    f.observed_at_ms = *NOW - 30_001;
    assert!(creating(&p, &f, "!room:s", "!space:s", &policy, &rp, None, *NOW).is_err());
    f.observed_at_ms = *NOW;
    f.service_can_invite = false;
    f.puppet_in_room = true;
    f.puppet_mxid = Some("@wrong:s".into());
    assert!(
        creating(
            &p,
            &f,
            "!room:s",
            "!space:s",
            &policy,
            &rp,
            Some("@right:s"),
            *NOW
        )
        .is_err()
    );
    f.service_can_invite = true;
    f.encrypted = true;
    assert!(creating(&p, &f, "!room:s", "!space:s", &policy, &rp, None, *NOW).is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_device_principal_snapshot_is_rechecked_after_rotate_and_revoke() {
    use crate::store::RegisterDevice;
    let url =
        std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test database required");
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (credential, p) = user(&auth, &suffix, "snapshot").await;
    let space = format!("!snapshot_space_{suffix}:example.test");
    let room = format!("!snapshot_room_{suffix}:example.test");
    let sf = admin(&p, &space, true, None);
    let project = store.register_project(&p, &space, &sf, *NOW).await.unwrap();
    store
        .register_room(
            &p,
            &project.id,
            &room,
            &sf,
            &admin(&p, &room, false, Some(&space)),
            *NOW,
        )
        .await
        .unwrap();
    let created = store
        .create_bound_agent(
            &p,
            CreateBoundAgent {
                project_id: project.id,
                room_id: room.clone(),
                display_name: "Snapshot agent".into(),
                idempotency_key: "snapshot-create".into(),
            },
            &facts(&p, &space, &room, None),
            *NOW,
        )
        .await
        .unwrap();
    let joined = facts(&p, &space, &room, Some(&created.agent.puppet_mxid));
    store
        .activate_binding(
            &p,
            &created.binding.id,
            created.binding.generation,
            &joined,
            *NOW,
        )
        .await
        .unwrap();
    let register = || RegisterDevice {
        installation_id: "snapshot-install".into(),
        name: "Snapshot device".into(),
    };
    let first = auth
        .register_device(&credential, register(), *NOW)
        .await
        .unwrap();
    let old_p = auth.authenticate(&first.token, *NOW, true).await.unwrap();
    assert!(store.agents(&old_p, *NOW).await.is_ok());
    assert!(
        store
            .runnable(&old_p, &created.binding.id, &joined, *NOW)
            .await
            .is_ok()
    );
    let rotated = auth
        .register_device(&credential, register(), *NOW)
        .await
        .unwrap();
    assert!(store.agents(&old_p, *NOW).await.is_err());
    assert!(
        store
            .runnable(&old_p, &created.binding.id, &joined, *NOW)
            .await
            .is_err()
    );
    let mut new_p = auth.authenticate(&rotated.token, *NOW, true).await.unwrap();
    assert!(
        store
            .runnable(&new_p, &created.binding.id, &joined, *NOW)
            .await
            .is_ok()
    );
    new_p.device_generation = None;
    assert!(store.agents(&new_p, *NOW).await.is_err());
    new_p.device_generation = Some(rotated.generation);
    auth.revoke_device(&credential, &rotated.device_id, *NOW)
        .await
        .unwrap();
    assert!(store.agents(&new_p, *NOW).await.is_err());
    assert!(
        store
            .runnable(&new_p, &created.binding.id, &joined, *NOW)
            .await
            .is_err()
    );
    assert!(store.agents(&p, *NOW).await.is_ok());
}

#[test]
fn caller_time_cannot_make_expired_matrix_observation_fresh() {
    let observed = crate::api::now_ms() - MAX_FACT_AGE_MS - 1;
    assert!(fresh(observed, observed).is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_stale_caller_clock_and_lock_wait_cannot_extend_session() {
    let url =
        std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test database required");
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (_, p) = user(&auth, &suffix, "clock").await;
    let stale_now = crate::api::now_ms() - 10_000;
    let mut lock = AsyncPgConnection::establish(&url).await.unwrap();
    lock.batch_execute("BEGIN; SELECT pg_advisory_xact_lock(5210750088328904)")
        .await
        .unwrap();
    sql_query("UPDATE hagency_agent_v1.sessions SET valid_until_ms=(extract(epoch from clock_timestamp())*1000)::bigint + 50 WHERE id=$1")
        .bind::<Text, _>(&p.session_id).execute(&mut lock).await.unwrap();
    let caller = store.clone();
    let queued = tokio::spawn(async move { caller.agents(&p, stale_now).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    lock.batch_execute("COMMIT").await.unwrap();
    assert!(queued.await.unwrap().is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_trusted_cleanup_converges_after_owner_logout_and_disable_without_reassigning() {
    let url =
        std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").expect("dedicated test database required");
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (credential, p) = user(&auth, &suffix, "cleanup").await;
    let space = format!("!cleanup_space_{suffix}:example.test");
    let room = format!("!cleanup_room_{suffix}:example.test");
    let sf = admin(&p, &space, true, None);
    let project = store.register_project(&p, &space, &sf, *NOW).await.unwrap();
    store
        .register_room(
            &p,
            &project.id,
            &room,
            &sf,
            &admin(&p, &room, false, Some(&space)),
            *NOW,
        )
        .await
        .unwrap();
    let created = store
        .create_bound_agent(
            &p,
            CreateBoundAgent {
                project_id: project.id,
                room_id: room.clone(),
                display_name: "Cleanup identity".into(),
                idempotency_key: "cleanup-create".into(),
            },
            &facts(&p, &space, &room, None),
            *NOW,
        )
        .await
        .unwrap();
    let joined = facts(&p, &space, &room, Some(&created.agent.puppet_mxid));
    store
        .activate_binding(
            &p,
            &created.binding.id,
            created.binding.generation,
            &joined,
            *NOW,
        )
        .await
        .unwrap();
    let mut departed = joined.clone();
    departed.puppet_in_room = false;
    departed.owner_in_room = false;
    departed.owner_in_space = false;
    assert!(
        store
            .confirm_left_trusted(
                &created.binding.id,
                created.binding.generation,
                &departed,
                *NOW
            )
            .await
            .is_err()
    );
    let retired = store
        .retire_agent(&p, &created.agent.id, *NOW)
        .await
        .unwrap();
    let binding = store.binding(&p, &created.binding.id, *NOW).await.unwrap();
    auth.sign_out(&credential).await.unwrap();
    let mut db = store.db.lock().await.unwrap();
    sql_query("UPDATE hagency_agent_v1.users SET active=false WHERE id=$1")
        .bind::<Text, _>(&p.user_id)
        .execute(&mut *db)
        .await
        .unwrap();
    drop(db);
    assert!(
        store
            .confirm_left(&p, &binding.id, binding.generation, &departed, *NOW)
            .await
            .is_err()
    );
    assert!(
        store
            .confirm_retired_trusted(&retired.id, retired.generation, *NOW)
            .await
            .is_err()
    );
    let scopes = store.cleanup_scopes(1000).await.unwrap();
    assert!(
        scopes
            .iter()
            .any(|s| s.binding_id.as_deref() == Some(&binding.id) && s.owner_user_id == p.user_id)
    );
    assert!(
        store
            .confirm_left_trusted(&binding.id, binding.generation - 1, &departed, *NOW)
            .await
            .is_err()
    );
    let mut spoofed = departed.clone();
    spoofed.owner_mxid = "@wrong:example.test".into();
    assert!(
        store
            .confirm_left_trusted(&binding.id, binding.generation, &spoofed, *NOW)
            .await
            .is_err()
    );
    let mut stale = departed.clone();
    stale.observed_at_ms = crate::api::now_ms() - 30_001;
    assert!(
        store
            .confirm_left_trusted(
                &binding.id,
                binding.generation,
                &stale,
                stale.observed_at_ms
            )
            .await
            .is_err()
    );
    assert!(
        store
            .confirm_left_trusted(&binding.id, binding.generation, &joined, *NOW)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .confirm_left_trusted(&binding.id, binding.generation, &departed, *NOW)
            .await
            .unwrap()
            .state,
        "left"
    );
    assert_eq!(
        store
            .confirm_left_trusted(&binding.id, binding.generation, &departed, *NOW)
            .await
            .unwrap()
            .state,
        "left"
    );
    assert!(
        store
            .cleanup_scopes(1000)
            .await
            .unwrap()
            .iter()
            .any(|s| s.agent_id == retired.id && s.binding_id.is_none())
    );
    let final_agent = store
        .confirm_retired_trusted(&retired.id, retired.generation, *NOW)
        .await
        .unwrap();
    assert_eq!(final_agent.state, "retired");
    assert_eq!(final_agent.owner_user_id, p.user_id);
    assert_eq!(final_agent.puppet_mxid, created.agent.puppet_mxid);
    assert!(
        !store
            .cleanup_scopes(1000)
            .await
            .unwrap()
            .iter()
            .any(|s| s.agent_id == retired.id)
    );
    let mut db = store.db.lock().await.unwrap();
    let audited=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.domain_audit WHERE actor_user_id=$1 AND object_id=$2 AND operation='worker.agent.retired') AS matched").bind::<Text,_>(&p.user_id).bind::<Text,_>(&retired.id).get_result::<Flag>(&mut *db).await.unwrap();
    assert!(audited.matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_committed_provisioning_survives_logout_and_terminal_sweep_retains_late_join_scope()
 {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (credential, p) = user(&auth, &suffix, "reconcile").await;
    let space = format!("!reconcile_space_{suffix}:example.test");
    let room = format!("!reconcile_room_{suffix}:example.test");
    let a = admin(&p, &space, true, None);
    let project = domain.register_project(&p, &space, &a, *NOW).await.unwrap();
    domain
        .register_room(
            &p,
            &project.id,
            &room,
            &a,
            &admin(&p, &room, false, Some(&space)),
            *NOW,
        )
        .await
        .unwrap();
    let created = domain
        .create_bound_agent(
            &p,
            CreateBoundAgent {
                project_id: project.id,
                room_id: room.clone(),
                display_name: "Codex".into(),
                idempotency_key: "reconcile".into(),
            },
            &facts(&p, &space, &room, None),
            *NOW,
        )
        .await
        .unwrap();
    let joined = facts(&p, &space, &room, Some(&created.agent.puppet_mxid));
    let mut not_joined = joined.clone();
    not_joined.puppet_in_room = false;
    assert!(
        domain
            .verify_provisioning_trusted(
                &created.binding.id,
                created.binding.generation,
                &not_joined,
                false,
                *NOW
            )
            .await
            .is_ok()
    );
    let mut forged = joined.clone();
    forged.owner_mxid = "@forged:example.test".into();
    assert!(
        domain
            .verify_provisioning_trusted(
                &created.binding.id,
                created.binding.generation,
                &forged,
                true,
                *NOW
            )
            .await
            .is_err()
    );
    assert!(
        domain
            .verify_provisioning_trusted(
                &created.binding.id,
                created.binding.generation - 1,
                &joined,
                true,
                *NOW
            )
            .await
            .is_err()
    );
    auth.sign_out(&credential).await.unwrap();
    assert!(
        domain
            .activate_binding(
                &p,
                &created.binding.id,
                created.binding.generation,
                &joined,
                *NOW
            )
            .await
            .is_err()
    );
    domain
        .verify_provisioning_trusted(
            &created.binding.id,
            created.binding.generation,
            &joined,
            true,
            *NOW,
        )
        .await
        .unwrap();
    // A new login of the same immutable identity may request departure.
    let (_, fresh) = user(&auth, &suffix, "reconcile").await;
    let leaving = domain
        .leave_binding(&fresh, &created.binding.id, *NOW)
        .await
        .unwrap();
    let mut departed = joined.clone();
    departed.puppet_in_room = false;
    domain
        .confirm_left_trusted(&leaving.id, leaving.generation, &departed, *NOW)
        .await
        .unwrap();
    let scopes = domain.reconciliation_scopes("", 1000).await.unwrap();
    assert!(
        scopes
            .iter()
            .any(|s| s.binding_id.as_deref() == Some(&leaving.id)
                && s.binding_state.as_deref() == Some("left"))
    );
    assert!(
        domain
            .departure_desired_trusted(&leaving.id, leaving.generation)
            .await
            .unwrap()
    );
    assert!(
        !domain
            .departure_desired_trusted(&leaving.id, leaving.generation - 1)
            .await
            .unwrap()
    );
    assert!(
        domain
            .verify_provisioning_trusted(
                &leaving.id,
                created.binding.generation,
                &joined,
                true,
                *NOW
            )
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_scope_pause_blocks_empty_scope_and_preserves_creation_runtime_boundary() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let store = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (_, p) = user(&auth, &suffix, "pause_owner").await;
    let space = format!("!pause_space_{suffix}:example.test");
    let room = format!("!pause_room_{suffix}:example.test");
    let room2 = format!("!pause_room2_{suffix}:example.test");
    let a = admin(&p, &space, true, None);
    let ra = admin(&p, &room, false, Some(&space));
    let ra2 = admin(&p, &room2, false, Some(&space));
    let project = store.register_project(&p, &space, &a, *NOW).await.unwrap();
    store
        .register_room(&p, &project.id, &room, &a, &ra, *NOW)
        .await
        .unwrap();
    store
        .register_room(&p, &project.id, &room2, &a, &ra2, *NOW)
        .await
        .unwrap();
    let create = |key: &str| CreateBoundAgent {
        project_id: project.id.clone(),
        room_id: room.clone(),
        display_name: "Scope pause probe".into(),
        idempotency_key: key.into(),
    };
    let f = facts(&p, &space, &room, None);
    assert_eq!(
        store
            .suspend_project_bindings(&p, &project.id, &a, *NOW)
            .await
            .unwrap(),
        0
    );
    assert!(matches!(
        store
            .create_bound_agent(&p, create("blocked-empty-project"), &f, *NOW)
            .await,
        Err(Error::Unauthorized("administrator_pause_active"))
    ));
    assert!(
        store
            .command_status(&p, "agent.bind", "blocked-empty-project", *NOW)
            .await
            .is_err()
    );
    store
        .suspend_room_bindings(&p, &project.id, &room, &ra, *NOW)
        .await
        .unwrap();
    store
        .clear_room_pause(&p, &project.id, &room, &ra, *NOW)
        .await
        .unwrap();
    let merged = store
        .service_state(&p, &project.id, Some(&room), *NOW)
        .await
        .unwrap();
    assert_eq!(merged["servicePaused"], true);
    assert_eq!(merged["projectPaused"], true);
    assert_eq!(merged["roomPaused"], false);
    assert!(
        store
            .create_bound_agent(&p, create("blocked-remaining-project"), &f, *NOW)
            .await
            .is_err()
    );
    store
        .clear_project_pause(&p, &project.id, &a, *NOW)
        .await
        .unwrap();
    let created = store
        .create_bound_agent(&p, create("pause-active"), &f, *NOW)
        .await
        .unwrap();
    let joined = facts(&p, &space, &room, Some(&created.agent.puppet_mxid));
    store
        .activate_binding(
            &p,
            &created.binding.id,
            created.binding.generation,
            &joined,
            *NOW,
        )
        .await
        .unwrap();
    let disabled = store
        .set_room_policy(
            &p,
            &room,
            &project.id,
            1,
            RoomCreationPolicy::Disabled,
            &ra,
            *NOW,
        )
        .await
        .unwrap();
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined, *NOW)
            .await
            .is_ok()
    );
    assert!(matches!(
        store
            .create_bound_agent(&p, create("blocked-disabled"), &f, *NOW)
            .await,
        Err(Error::Unauthorized("agent_creation_denied"))
    ));
    store
        .set_room_policy(
            &p,
            &room,
            &project.id,
            disabled.revision,
            RoomCreationPolicy::default(),
            &ra,
            *NOW,
        )
        .await
        .unwrap();
    // A room with no existing bindings cannot evade an administrator pause by
    // binding an existing permanent-owner Agent instead of creating a new one.
    store
        .suspend_room_bindings(&p, &project.id, &room2, &ra2, *NOW)
        .await
        .unwrap();
    let bind = || BindRoom {
        project_id: project.id.clone(),
        room_id: room2.clone(),
        idempotency_key: "blocked-new-binding".into(),
    };
    let f2 = facts(&p, &space, &room2, Some(&created.agent.puppet_mxid));
    assert!(matches!(
        store
            .bind_room(&p, &created.agent.id, bind(), &f2, *NOW)
            .await,
        Err(Error::Unauthorized("administrator_pause_active"))
    ));
    assert!(
        store
            .command_status(&p, "agent.bind", "blocked-new-binding", *NOW)
            .await
            .is_err()
    );
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined, *NOW)
            .await
            .is_ok()
    );
    store
        .clear_room_pause(&p, &project.id, &room2, &ra2, *NOW)
        .await
        .unwrap();
    let second = store
        .bind_room(&p, &created.agent.id, bind(), &f2, *NOW)
        .await
        .unwrap();
    assert_eq!(
        store
            .command_status(&p, "agent.bind", "blocked-new-binding", *NOW)
            .await
            .unwrap()
            .binding
            .unwrap()
            .id,
        second.binding.id
    );
    store
        .suspend_project_bindings(&p, &project.id, &a, *NOW)
        .await
        .unwrap();
    store
        .clear_project_pause(&p, &project.id, &a, *NOW)
        .await
        .unwrap();
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined, *NOW)
            .await
            .is_err()
    );
    store
        .resume_binding(&p, &created.binding.id, &joined, *NOW)
        .await
        .unwrap();
    assert!(
        store
            .runnable(&p, &created.binding.id, &joined, *NOW)
            .await
            .is_ok()
    );
}

async fn rebind_departed(
    domain: &DomainStore,
    p: &Principal,
    binding: &Binding,
    good: &RoomFacts,
    key: &str,
) -> Binding {
    let current = domain.binding(p, &binding.id, *NOW).await.unwrap();
    assert_eq!(current.state, "leaving");
    assert!(
        domain
            .departure_desired_trusted(&binding.id, current.generation)
            .await
            .unwrap()
    );
    let mut departed = good.clone();
    departed.puppet_in_room = false;
    domain
        .confirm_left_trusted(&binding.id, current.generation, &departed, *NOW)
        .await
        .unwrap();
    let joined = domain
        .bind_room(
            p,
            &binding.agent_id,
            BindRoom {
                project_id: binding.project_id.clone().unwrap(),
                room_id: binding.room_id.clone(),
                idempotency_key: key.into(),
            },
            good,
            *NOW,
        )
        .await
        .unwrap();
    domain
        .activate_binding(p, &binding.id, joined.binding.generation, good, *NOW)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_membership_loss_is_durable_and_old_dispatch_cannot_revive() {
    use crate::{
        store::RegisterDevice,
        transport::{DeliveryFacts, LeaseRef, Limits, RouteResult, RoutedEvent, TransportStore},
    };
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let transport = TransportStore::open(&url, Limits::default()).await.unwrap();
    let suffix = secret_token();
    let (credential, p) = user(&auth, &suffix, "member_loss").await;
    let mut bindings = Vec::new();
    let mut created_agent: Option<String> = None;
    for index in 0..2 {
        let space = format!("!membership_space{index}_{suffix}:example.test");
        let room = format!("!membership_room{index}_{suffix}:example.test");
        let a = admin(&p, &space, true, None);
        let project = domain.register_project(&p, &space, &a, *NOW).await.unwrap();
        domain
            .register_room(
                &p,
                &project.id,
                &room,
                &a,
                &admin(&p, &room, false, Some(&space)),
                *NOW,
            )
            .await
            .unwrap();
        let creation = match &created_agent {
            None => domain
                .create_bound_agent(
                    &p,
                    CreateBoundAgent {
                        project_id: project.id.clone(),
                        room_id: room.clone(),
                        display_name: "membership".into(),
                        idempotency_key: "membership-create".into(),
                    },
                    &facts(&p, &space, &room, None),
                    *NOW,
                )
                .await
                .unwrap(),
            Some(agent_id) => {
                let puppet = domain.agent(&p, agent_id, *NOW).await.unwrap().puppet_mxid;
                domain
                    .bind_room(
                        &p,
                        agent_id,
                        BindRoom {
                            project_id: project.id.clone(),
                            room_id: room.clone(),
                            idempotency_key: "membership-bind".into(),
                        },
                        &facts(&p, &space, &room, Some(&puppet)),
                        *NOW,
                    )
                    .await
                    .unwrap()
            }
        };
        created_agent = Some(creation.agent.id.clone());
        let f = facts(&p, &space, &room, Some(&creation.agent.puppet_mxid));
        let b = domain
            .activate_binding(
                &p,
                &creation.binding.id,
                creation.binding.generation,
                &f,
                *NOW,
            )
            .await
            .unwrap();
        bindings.push((b, f));
    }
    let agent_id = created_agent.unwrap();
    let device = auth
        .register_device(
            &credential,
            RegisterDevice {
                installation_id: format!("membership-{suffix}"),
                name: "test".into(),
            },
            *NOW,
        )
        .await
        .unwrap();
    let dp = auth.authenticate(&device.token, *NOW, true).await.unwrap();
    domain
        .set_execution_device(
            &dp,
            &agent_id,
            SetExecutionDevice {
                expected_generation: domain.agent(&dp, &agent_id, *NOW).await.unwrap().generation,
            },
            *NOW,
        )
        .await
        .unwrap();
    let lease = transport
        .acquire_for_test(&dp, &agent_id, 60_000, false, *NOW)
        .await
        .unwrap();
    let reference = LeaseRef {
        agent_id,
        epoch: lease.epoch,
    };
    let (b, good) = &bindings[0];
    let delivery = DeliveryFacts {
        owner_direct_valid: false,
        owner_mxid: p.mxid.clone(),
        requester_mxid: p.mxid.clone(),
        puppet_mxid: good.puppet_mxid.clone().unwrap(),
        room_id: good.room_id.clone(),
        space_id: good.space_id.clone(),
        observed_at_ms: *NOW,
        owner_in_space: true,
        owner_in_room: true,
        room_in_space: true,
        requester_in_room: true,
        puppet_in_room: true,
        puppet_can_send_message: true,
        encrypted: false,
    };
    let queued_event = RoutedEvent {
        event_id: format!("$membership_queued_{suffix}"),
        room_id: good.room_id.clone(),
        sender_mxid: p.mxid.clone(),
        body: "queued request".into(),
        mentioned_mxids: [delivery.puppet_mxid.clone()].into(),
        thread_root: None,
        encrypted: false,
        is_edit: false,
    };
    let RouteResult::Queued {
        dispatch_id: queued_id,
    } = transport
        .ingest_routed(&b.id, queued_event, &delivery, *NOW)
        .await
        .unwrap()
    else {
        panic!("not queued");
    };
    let event = RoutedEvent {
        event_id: format!("$membership_{suffix}"),
        room_id: good.room_id.clone(),
        sender_mxid: p.mxid.clone(),
        body: "original request".into(),
        mentioned_mxids: [delivery.puppet_mxid.clone()].into(),
        thread_root: None,
        encrypted: false,
        is_edit: false,
    };
    let RouteResult::Queued { dispatch_id } = transport
        .ingest_routed(&b.id, event, &delivery, *NOW)
        .await
        .unwrap()
    else {
        panic!("not queued")
    };
    transport
        .claim_event(&dp, &reference, &dispatch_id, &delivery, *NOW)
        .await
        .unwrap();
    transport
        .acknowledge(&dp, &reference, &dispatch_id, *NOW)
        .await
        .unwrap();
    transport
        .start_execution(
            &dp,
            &reference,
            &dispatch_id,
            "membership-old-execution",
            &delivery,
            *NOW,
        )
        .await
        .unwrap();
    assert!(
        !domain
            .retire_membership_loss_trusted(&b.id, b.generation, good, *NOW)
            .await
            .unwrap()
    );
    let mut lost = good.clone();
    lost.owner_in_room = false;
    let mut spoofed = lost.clone();
    spoofed.owner_mxid = "@other:example.test".into();
    assert!(
        domain
            .retire_membership_loss_trusted(&b.id, b.generation, &spoofed, *NOW)
            .await
            .is_err()
    );
    // The denied authenticated domain operation commits the loss separately;
    // its error must not roll the departure back.
    assert!(domain.runnable(&p, &b.id, &lost, *NOW).await.is_err());
    let paused = domain.binding(&p, &b.id, *NOW).await.unwrap();
    assert_eq!(paused.state, "leaving");
    assert_eq!(paused.generation, b.generation + 1);
    assert!(domain.runnable(&p, &b.id, good, *NOW).await.is_err());
    assert!(
        domain
            .runnable(&p, &bindings[1].0.id, &bindings[1].1, *NOW)
            .await
            .is_ok()
    );
    // Rejoining requires a confirmed departure and explicit owner rebind.
    // Old observations and executions cannot affect the new generation.
    assert!(
        !domain
            .retire_membership_loss_trusted(&b.id, paused.generation, good, *NOW)
            .await
            .unwrap()
    );
    assert!(domain.resume_binding(&p, &b.id, good, *NOW).await.is_err());
    let resumed = rebind_departed(&domain, &p, b, good, "rebind-owner-return").await;
    assert_eq!(resumed.generation, paused.generation + 1);
    assert!(
        domain
            .retire_membership_loss_trusted(&b.id, b.generation, &lost, *NOW)
            .await
            .is_err()
    );
    assert!(
        transport
            .start_execution(
                &dp,
                &reference,
                &dispatch_id,
                "membership-old-execution",
                &delivery,
                *NOW
            )
            .await
            .is_err()
    );
    transport.invalidate_stale_dispatches(*NOW).await.unwrap();
    #[derive(diesel::QueryableByName)]
    struct EventState {
        #[diesel(sql_type=Text)]
        state: String,
    }
    let state = sql_query("SELECT state FROM hagency_agent_v1.owner_events WHERE id=$1")
        .bind::<Text, _>(&dispatch_id)
        .get_result::<EventState>(&mut *domain.db.lock().await.unwrap())
        .await
        .unwrap();
    assert_eq!(state.state, "unknown");
    let queued = sql_query("SELECT state FROM hagency_agent_v1.owner_events WHERE id=$1")
        .bind::<Text, _>(&queued_id)
        .get_result::<EventState>(&mut *domain.db.lock().await.unwrap())
        .await
        .unwrap();
    assert_eq!(queued.state, "cancelled");
    let new_event = RoutedEvent {
        event_id: format!("$membership_new_{suffix}"),
        room_id: good.room_id.clone(),
        sender_mxid: p.mxid.clone(),
        body: "new request".into(),
        mentioned_mxids: [delivery.puppet_mxid.clone()].into(),
        thread_root: None,
        encrypted: false,
        is_edit: false,
    };
    assert!(matches!(
        transport
            .ingest_routed(&b.id, new_event, &delivery, *NOW)
            .await
            .unwrap(),
        RouteResult::Queued { .. }
    ));
    // Puppet removal also requires an explicit membership-restored owner resume.
    let mut puppet_lost = good.clone();
    puppet_lost.puppet_in_room = false;
    assert!(
        domain
            .retire_membership_loss_trusted(&b.id, resumed.generation, &puppet_lost, *NOW)
            .await
            .unwrap()
    );
    assert!(
        domain
            .resume_binding(&p, &b.id, &puppet_lost, *NOW)
            .await
            .is_err()
    );
    for unlink in [false, true] {
        let restored = rebind_departed(
            &domain,
            &p,
            b,
            good,
            if unlink {
                "rebind-after-space-loss"
            } else {
                "rebind-after-kick"
            },
        )
        .await;
        let mut removed = good.clone();
        if unlink {
            removed.room_in_space = false;
        } else {
            removed.owner_in_space = false;
        }
        assert!(
            domain
                .retire_membership_loss_trusted(&b.id, restored.generation, &removed, *NOW)
                .await
                .unwrap()
        );
        assert!(
            domain
                .resume_binding(&p, &b.id, &removed, *NOW)
                .await
                .is_err()
        );
        assert_eq!(
            domain
                .binding(&p, &bindings[1].0.id, *NOW)
                .await
                .unwrap()
                .generation,
            bindings[1].0.generation
        );
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_retirement_cursor_passes_twenty_unavailable_departures() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    let name = format!("hagency_retirement_{}", &hash(&secret_token())[..16]);
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut isolated = url::Url::parse(&source).unwrap();
    isolated.set_path(&format!("/{name}"));
    let result=tokio::spawn(async move {
        let auth=Store::open(isolated.as_str(),"example.test","https://example.test/auth/").await.unwrap();
        let domain=DomainStore::open(isolated.as_str(),"example.test","_hagency_test_").await.unwrap();
        let (_,p)=user(&auth,&secret_token(),"retire").await;
        let project=domain.register_project(&p,"!space:example.test",&admin(&p,"!space:example.test",true,None),*NOW).await.unwrap();
        domain.register_room(&p,&project.id,"!room:example.test",&admin(&p,"!space:example.test",true,None),&admin(&p,"!room:example.test",false,Some("!space:example.test")),*NOW).await.unwrap();
        let mut db=AsyncPgConnection::establish(isolated.as_str()).await.unwrap();
        for n in 0..20 {
            let id=format!("agt_a_{n:03}");
            sql_query("INSERT INTO hagency_agent_v1.agents(id,owner_user_id,puppet_mxid,display_name,state) VALUES($1,$2,$3,'Blocked departure','retiring')")
                .bind::<Text,_>(&id).bind::<Text,_>(&p.user_id).bind::<Text,_>(format!("@_hagency_test_a_{n}:example.test")).execute(&mut db).await.unwrap();
            sql_query("INSERT INTO hagency_agent_v1.bindings(id,agent_id,project_id,room_id,state) VALUES($1,$2,$3,'!room:example.test','leaving')")
                .bind::<Text,_>(format!("bnd_a_{n:03}")).bind::<Text,_>(id).bind::<Text,_>(&project.id).execute(&mut db).await.unwrap();
        }
        sql_query("INSERT INTO hagency_agent_v1.agents(id,owner_user_id,puppet_mxid,display_name,state) VALUES('agt_z_ready',$1,'@_hagency_test_ready:example.test','Ready for retirement','retiring')")
            .bind::<Text,_>(&p.user_id).execute(&mut db).await.unwrap();
        let first=domain.cleanup_scopes_after("","",20).await.unwrap();
        assert_eq!(first.len(),20);assert!(first.iter().all(|s|s.binding_id.is_some()));
        let last=first.last().unwrap();
        let next=domain.cleanup_scopes_after(&last.agent_id,last.binding_id.as_deref().unwrap(),20).await.unwrap();
        assert_eq!(next.len(),1);assert_eq!(next[0].agent_id,"agt_z_ready");assert!(next[0].binding_id.is_none());
        domain.confirm_retired_trusted("agt_z_ready",next[0].agent_generation,*NOW).await.unwrap();
        assert_eq!(domain.agent(&p,"agt_z_ready",*NOW).await.unwrap().state,"retired");
        assert!(domain.cleanup_scopes_after("agt_z_ready","",20).await.unwrap().is_empty());
        assert_eq!(domain.cleanup_scopes_after("","",20).await.unwrap().len(),20);
    }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_global_identity_create_needs_no_project_or_room_and_binding_is_separate() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (_, p) = user(&auth, &secret_token(), "global_owner").await;
    let make = || CreateAgent {
        visibility: crate::domain::AgentVisibility::Public,
        display_name: "Global Codex".into(),
        idempotency_key: "global-identity".into(),
    };
    let agent = domain.create_agent(&p, make(), *NOW).await.unwrap();
    crate::assert_entity_id(&agent.id, "agt_");
    assert_eq!(
        agent.puppet_mxid,
        format!("@_hagency_test_{}:example.test", agent.id)
    );
    assert_eq!(agent.state, "creating");
    assert_eq!(agent.owner_user_id, p.user_id);
    assert!(
        domain
            .bindings(&p, &agent.id, *NOW)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        domain.create_agent(&p, make(), *NOW).await.unwrap().id,
        agent.id
    );
    let command = domain
        .command_status(&p, "agent.create", "global-identity", *NOW)
        .await
        .unwrap();
    assert!(command.binding.is_none());
    assert!(serde_json::from_value::<CreateAgent>(serde_json::json!({"displayName":"Global","idempotencyKey":"new","projectId":"prj_old","roomId":"!old:test"})).is_err(),"old combined Create contract is rejected");
    domain
        .confirm_identity_provisioned(&agent.id, agent.generation)
        .await
        .unwrap();
    assert_eq!(
        domain.agent(&p, &agent.id, *NOW).await.unwrap().state,
        "active"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_owner_loss_retires_joining_and_paused_bindings_before_provisioning() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (_, p) = user(&auth, &suffix, "depart").await;
    let (_, other) = user(&auth, &suffix, "stranger").await;
    let space = format!("!depart_space_{suffix}:example.test");
    let a = admin(&p, &space, true, None);
    let project = domain.register_project(&p, &space, &a, *NOW).await.unwrap();
    for mode in ["joining", "suspended", "final_join", "final_kick"] {
        let room = format!("!depart_{mode}_{suffix}:example.test");
        domain
            .register_room(
                &p,
                &project.id,
                &room,
                &a,
                &admin(&p, &room, false, Some(&space)),
                *NOW,
            )
            .await
            .unwrap();
        let agent = domain
            .create_agent(
                &p,
                CreateAgent {
                    visibility: crate::domain::AgentVisibility::Public,
                    display_name: mode.into(),
                    idempotency_key: format!("create-{mode}"),
                },
                *NOW,
            )
            .await
            .unwrap();
        let mut good = facts(&p, &space, &room, Some(&agent.puppet_mxid));
        good.puppet_in_room = false;
        let request = || BindRoom {
            project_id: project.id.clone(),
            room_id: room.clone(),
            idempotency_key: format!("bind-{mode}"),
        };
        // A third party's invitation/admin status cannot substitute for ownership.
        assert!(
            domain
                .bind_room(
                    &other,
                    &agent.id,
                    request(),
                    &facts(&other, &space, &room, None),
                    *NOW
                )
                .await
                .is_err()
        );
        let mut absent = good.clone();
        absent.owner_in_room = false;
        assert!(
            domain
                .bind_room(&p, &agent.id, request(), &absent, *NOW)
                .await
                .is_err()
        );
        let created = domain
            .bind_room(&p, &agent.id, request(), &good, *NOW)
            .await
            .unwrap();
        let mut binding = created.binding;
        // A not-yet-joined puppet must not cancel a legitimate joining intent.
        assert!(
            !domain
                .retire_membership_loss_trusted(&binding.id, binding.generation, &good, *NOW)
                .await
                .unwrap()
        );
        good.puppet_in_room = true;
        if mode == "suspended" {
            domain
                .activate_binding(&p, &binding.id, binding.generation, &good, *NOW)
                .await
                .unwrap();
            binding = domain.suspend_binding(&p, &binding.id, *NOW).await.unwrap();
        }
        let mut lost = good.clone();
        if mode == "final_kick" {
            lost.puppet_in_room = false;
        } else {
            lost.owner_in_room = false;
        }
        let mut stale = lost.clone();
        stale.observed_at_ms = *NOW - 60_000;
        assert!(
            domain
                .retire_membership_loss_trusted(&binding.id, binding.generation, &stale, *NOW)
                .await
                .is_err()
        );
        if matches!(mode, "final_join" | "final_kick") {
            assert!(
                domain
                    .verify_provisioning_trusted(&binding.id, binding.generation, &lost, true, *NOW)
                    .await
                    .is_err()
            );
        } else {
            assert!(
                domain
                    .retire_membership_loss_trusted(&binding.id, binding.generation, &lost, *NOW)
                    .await
                    .unwrap()
            );
        }
        let leaving = domain.binding(&p, &binding.id, *NOW).await.unwrap();
        assert_eq!(leaving.state, "leaving");
        assert_eq!(leaving.generation, binding.generation + 1);
        assert!(
            domain
                .verify_provisioning_trusted(&binding.id, leaving.generation, &good, true, *NOW)
                .await
                .is_err()
        );
        assert!(
            domain
                .resume_binding(&p, &binding.id, &good, *NOW)
                .await
                .is_err()
        );
        assert!(
            !domain
                .retire_membership_loss_trusted(&binding.id, leaving.generation, &lost, *NOW)
                .await
                .unwrap()
        );
        assert!(
            domain
                .cleanup_scopes(1000)
                .await
                .unwrap()
                .iter()
                .any(|s| s.binding_id.as_deref() == Some(&binding.id))
        );
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_membership_controller_leaves_paused_and_joining_rooms_and_retries() {
    let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut control = AsyncPgConnection::establish(&source).await.unwrap();
    let name = format!("hagency_membership_{}", &hash(&secret_token())[..16]);
    control
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let mut url = url::Url::parse(&source).unwrap();
    url.set_path(&format!("/{name}"));
    let result =
        tokio::spawn(async move { exercise_membership_controller(url.as_str()).await }).await;
    control
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn membership_controller_rechecks_replaced_connection_without_stacking_locks() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let first = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let second = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    assert!(first.try_membership_controller().await.unwrap());
    assert!(first.try_membership_controller().await.unwrap());
    assert!(!second.try_membership_controller().await.unwrap());
    // Repeated checks must not require multiple unlocks.
    first
        .db
        .lock()
        .await
        .unwrap()
        .batch_execute("SELECT pg_advisory_unlock(5210750088328905)")
        .await
        .unwrap();
    assert!(second.try_membership_controller().await.unwrap());
    second
        .db
        .lock()
        .await
        .unwrap()
        .batch_execute("SELECT pg_advisory_unlock(5210750088328905)")
        .await
        .unwrap();
    assert!(first.try_membership_controller().await.unwrap());

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let mut operation = Box::pin(async {
        let mut guard = first.db.lock().await.unwrap();
        (*guard)
            .transaction::<_, diesel::result::Error, _>(async |conn: &mut AsyncPgConnection| {
                conn.batch_execute("SELECT 1").await?;
                ready_tx.send(()).unwrap();
                std::future::pending::<()>().await;
                Ok(())
            })
            .await
            .unwrap();
    });
    tokio::select! { _ = &mut operation => panic!("must remain pending"), _ = ready_rx => () }
    drop(operation);
    // Terminating the cancelled leader's connection must allow another replica
    // to take over. The previous leader's new connection must yield to it.
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        while !second.try_membership_controller().await.unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!first.try_membership_controller().await.unwrap());
    second
        .db
        .lock()
        .await
        .unwrap()
        .batch_execute("SELECT pg_advisory_unlock(5210750088328905)")
        .await
        .unwrap();
    assert!(first.try_membership_controller().await.unwrap());
}

async fn exercise_membership_controller(url: &str) {
    use crate::{gateway::Gateway, matrix_client::MatrixClient};
    use std::{
        collections::HashMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let auth = Store::open(url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (_, p) = user(&auth, &secret_token(), "controller").await;
    let space = "!controller_space:example.test";
    let a = admin(&p, space, true, None);
    let project = domain.register_project(&p, space, &a, *NOW).await.unwrap();
    let rooms = Arc::new(Mutex::new(HashMap::<String, (bool, bool, String)>::new()));
    let attempts = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    let joins = Arc::new(AtomicUsize::new(0));
    let mut bindings = Vec::new();
    for mode in [
        "joining",
        "active",
        "suspended",
        "kicked",
        "valid",
        "unavailable",
    ] {
        let room = format!("!controller_{mode}:example.test");
        domain
            .register_room(
                &p,
                &project.id,
                &room,
                &a,
                &admin(&p, &room, false, Some(space)),
                *NOW,
            )
            .await
            .unwrap();
        let agent = domain
            .create_agent(
                &p,
                CreateAgent {
                    visibility: crate::domain::AgentVisibility::Public,
                    display_name: mode.into(),
                    idempotency_key: format!("controller-create-{mode}"),
                },
                *NOW,
            )
            .await
            .unwrap();
        domain
            .confirm_identity_provisioned(&agent.id, agent.generation)
            .await
            .unwrap();
        let good = facts(&p, space, &room, Some(&agent.puppet_mxid));
        let creation = domain
            .bind_room(
                &p,
                &agent.id,
                BindRoom {
                    project_id: project.id.clone(),
                    room_id: room.clone(),
                    idempotency_key: format!("controller-bind-{mode}"),
                },
                &good,
                *NOW,
            )
            .await
            .unwrap();
        let mut binding = creation.binding;
        if mode != "joining" {
            binding = domain
                .activate_binding(&p, &binding.id, binding.generation, &good, *NOW)
                .await
                .unwrap();
        }
        if mode == "suspended" {
            binding = domain.suspend_binding(&p, &binding.id, *NOW).await.unwrap();
        }
        rooms.lock().unwrap().insert(
            room,
            (
                matches!(mode, "valid" | "kicked"),
                mode != "kicked",
                agent.puppet_mxid,
            ),
        );
        bindings.push((mode, binding));
    }
    let state = rooms.clone();
    let owner = p.mxid.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let state = state.clone();
            let owner = owner.clone();
            Box::pin(async move {
                if room.contains("unavailable") {
                    return Err(Error::Unavailable("fixture_state_unavailable"));
                }
                let state = state.lock().unwrap();
                let mut events = vec![
                    serde_json::json!({"type":"m.room.member","state_key":owner,"content":{"membership":"join"}}),
                ];
                if room == space {
                    for room in state.keys() {
                        events.push(serde_json::json!({"type":"m.space.child","state_key":room,"content":{"via":["example.test"]}}));
                    }
                } else {
                    let (present, joined, puppet) = state.get(&room).unwrap();
                    events[0]["content"]["membership"] =
                        serde_json::json!(if *present { "join" } else { "leave" });
                    events.push(serde_json::json!({"type":"m.room.member","state_key":puppet,"content":{"membership":if *joined {"join"} else {"leave"}}}));
                }
                Ok(events)
            })
        }),
        "@_hagency_service:example.test".into(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = rooms.clone();
    let calls = attempts.clone();
    let joins_observed = joins.clone();
    let http = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut raw = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let read = socket.read(&mut buf).await.unwrap();
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&buf[..read]);
                if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(raw).unwrap();
            let target = request.split_whitespace().nth(1).unwrap();
            let url = url::Url::parse(&format!("http://localhost{target}")).unwrap();
            let segments: Vec<_> = url
                .path_segments()
                .unwrap()
                .map(|s| {
                    percent_encoding::percent_decode_str(s)
                        .decode_utf8()
                        .unwrap()
                        .into_owned()
                })
                .collect();
            let mut status = "200 OK";
            if segments.last().map(String::as_str) == Some("leave") {
                let room = &segments[4];
                let mut calls = calls.lock().unwrap();
                let count = calls.entry(room.clone()).or_default();
                *count += 1;
                // A failed departure must remain owed across subsequent rounds.
                if *count == 1 {
                    status = "503 Service Unavailable";
                } else {
                    state.lock().unwrap().get_mut(room).unwrap().1 = false;
                }
            } else {
                joins_observed.fetch_add(1, Ordering::SeqCst);
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let matrix = MatrixClient::new(
        format!("http://{address}").parse().unwrap(),
        "fixture-AS-token-no-real-secret".into(),
        "example.test".into(),
    )
    .unwrap();
    let worker = crate::workers::start_cleanup(domain.clone(), gateway, matrix);
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let mut complete = true;
            for (mode, binding) in &bindings {
                let b = domain
                    .binding(&p, &binding.id, crate::api::now_ms())
                    .await
                    .unwrap();
                if matches!(*mode, "valid" | "unavailable") {
                    assert_eq!(b.state, "active");
                    assert_eq!(b.generation, binding.generation);
                } else {
                    complete &= b.state == "left";
                    if b.state == "left" {
                        assert_eq!(b.generation, binding.generation + 1);
                    }
                }
            }
            if complete {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await;
    worker.abort();
    let _ = worker.await;
    http.abort();
    let _ = http.await;
    result.expect("membership controller did not converge");
    assert_eq!(
        AtomicUsize::load(&joins, Ordering::SeqCst),
        0,
        "revoked intents cannot issue joins"
    );
    let calls = attempts.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(
        calls.values().all(|n| *n >= 2),
        "failed departure must retry"
    );
}

#[test]
fn agent_audience_defaults_private_and_rejects_unknown_values() {
    let request: CreateAgent =
        serde_json::from_value(serde_json::json!({"displayName":"Mine","idempotencyKey":"legacy"}))
            .unwrap();
    assert_eq!(request.visibility, AgentVisibility::Private);
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        serde_json::json!({"displayName":"Mine","idempotencyKey":"legacy"}),
        "legacy request digests stay stable"
    );
    for value in ["PUBLIC", "team", "", "owner", "Private"] {
        assert!(
            serde_json::from_value::<CreateAgent>(
                serde_json::json!({"displayName":"Mine","idempotencyKey":"key","visibility":value})
            )
            .is_err()
        );
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_agent_audience_enforces_discovery_room_binding_and_owner_dm() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let auth = Store::open(&url, "example.test", "https://example.test/auth/")
        .await
        .unwrap();
    let domain = DomainStore::open(&url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let suffix = secret_token();
    let (_, owner) = user(&auth, &suffix, "audience_owner").await;
    let (_, other) = user(&auth, &suffix, "audience_other").await;
    let private = domain
        .create_agent(
            &owner,
            serde_json::from_value(
                serde_json::json!({"displayName":"Private","idempotencyKey":"private"}),
            )
            .unwrap(),
            *NOW,
        )
        .await
        .unwrap();
    assert_eq!(private.visibility, AgentVisibility::Private);
    domain
        .confirm_identity_provisioned(&private.id, private.generation)
        .await
        .unwrap();
    assert!(
        domain
            .agents(&owner, *NOW)
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == private.id)
    );
    assert!(
        !domain
            .discover_agents(&other, *NOW)
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == private.id)
    );
    assert!(domain.agent(&other, &private.id, *NOW).await.is_err());
    assert!(
        domain
            .set_visibility(&other, &private.id, AgentVisibility::Public, *NOW)
            .await
            .is_err()
    );
    let space = format!("!audience_space_{suffix}:example.test");
    let room = format!("!audience_room_{suffix}:example.test");
    let project = domain
        .register_project(&owner, &space, &admin(&owner, &space, true, None), *NOW)
        .await
        .unwrap();
    domain
        .register_room(
            &owner,
            &project.id,
            &room,
            &admin(&owner, &space, true, None),
            &admin(&owner, &room, false, Some(&space)),
            *NOW,
        )
        .await
        .unwrap();
    let request = || BindRoom {
        project_id: project.id.clone(),
        room_id: room.clone(),
        idempotency_key: "shared".into(),
    };
    let good = facts(&owner, &space, &room, Some(&private.puppet_mxid));
    assert!(matches!(
        domain
            .bind_room(&owner, &private.id, request(), &good, *NOW)
            .await,
        Err(Error::Unauthorized("private_agent_owner_direct_only"))
    ));
    let direct = format!("!audience_direct_{suffix}:example.test");
    let mut direct_facts = facts(&owner, "", &direct, Some(&private.puppet_mxid));
    direct_facts.owner_direct_valid = true;
    let dm = domain
        .adopt_owner_direct(&owner, &private.id, &direct, &direct_facts, *NOW)
        .await
        .unwrap();
    assert_eq!(dm.binding.scope_kind, "owner_direct");
    domain
        .set_visibility(&owner, &private.id, AgentVisibility::Public, *NOW)
        .await
        .unwrap();
    let catalog = domain.discover_agents(&other, *NOW).await.unwrap();
    let row = catalog.iter().find(|a| a.id == private.id).unwrap();
    assert!(!row.is_mine);
    let safe = serde_json::to_value(row).unwrap();
    for field in ["ownerUserId", "ownerDirectRoomId", "executionDeviceId"] {
        assert!(safe.get(field).is_none());
    }
    assert!(
        domain
            .discover_agents(&owner, *NOW)
            .await
            .unwrap()
            .iter()
            .find(|a| a.id == private.id)
            .unwrap()
            .is_mine
    );
    let bound = domain
        .bind_room(&owner, &private.id, request(), &good, *NOW)
        .await
        .unwrap();
    domain
        .activate_binding(
            &owner,
            &bound.binding.id,
            bound.binding.generation,
            &good,
            *NOW,
        )
        .await
        .unwrap();
    assert!(
        domain
            .room_roster(&other, &project.id, &room, *NOW)
            .await
            .unwrap()
            .iter()
            .any(|a| a.agent_id == private.id && !a.is_mine)
    );
    assert!(matches!(
        domain
            .set_visibility(&owner, &private.id, AgentVisibility::Private, *NOW)
            .await,
        Err(Error::Conflict("leave_shared_rooms_before_private"))
    ));
    let leaving = domain
        .leave_binding(&owner, &bound.binding.id, *NOW)
        .await
        .unwrap();
    let mut departed = good.clone();
    departed.puppet_in_room = false;
    domain
        .confirm_left_trusted(&leaving.id, leaving.generation, &departed, *NOW)
        .await
        .unwrap();
    domain
        .set_visibility(&owner, &private.id, AgentVisibility::Private, *NOW)
        .await
        .unwrap();
    assert!(
        !domain
            .discover_agents(&other, *NOW)
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == private.id)
    );
    assert!(domain.binding(&owner, &dm.binding.id, *NOW).await.is_ok());
    assert!(
        domain
            .bind_room(&owner, &private.id, request(), &good, *NOW)
            .await
            .is_err(),
        "replayed shared command cannot reauthorize private service"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn postgres_agent_audience_migration_preserves_private_defaults_and_saved_choices() {
    let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let mut db = AsyncPgConnection::establish(&url).await.unwrap();
    let schema = format!("audience_migration_{}", &secret_token()[..16]);
    db.batch_execute(&format!("CREATE SCHEMA {schema}; CREATE TABLE {schema}.agents(id text PRIMARY KEY); CREATE TABLE {schema}.bindings(agent_id text,scope_kind text,state text); INSERT INTO {schema}.agents VALUES ('shared'),('direct'),('left'); INSERT INTO {schema}.bindings VALUES ('shared','project','suspended'),('direct','owner_direct','active'),('left','project','left');")).await.unwrap();
    let migration = include_str!("visibility_schema.sql").replace("hagency_agent_v1", &schema);
    db.batch_execute(&migration).await.unwrap();
    db.batch_execute(&format!("INSERT INTO {schema}.agents(id) VALUES ('new'); UPDATE {schema}.agents SET visibility='public' WHERE id='direct';")).await.unwrap();
    db.batch_execute(&migration).await.unwrap();
    let valid=sql_query(format!("SELECT (SELECT visibility FROM {schema}.agents WHERE id='shared')='public' AND (SELECT visibility FROM {schema}.agents WHERE id='left')='private' AND (SELECT visibility FROM {schema}.agents WHERE id='new')='private' AND (SELECT visibility FROM {schema}.agents WHERE id='direct')='public' AS matched")).get_result::<Flag>(&mut db).await.unwrap();
    assert!(
        valid.matched,
        "migration changed a saved choice or published an unshared Agent"
    );
    assert!(
        db.batch_execute(&format!(
            "UPDATE {schema}.agents SET visibility='invalid' WHERE id='new'"
        ))
        .await
        .is_err()
    );
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .unwrap();
}
