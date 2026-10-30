use super::*;
#[path="management_shared_budget_tests.rs"]
mod budget_tests;
#[path="management_shared_context_tests.rs"]
mod context_tests;
#[path = "management_shared_work_tests.rs"]
mod work;
#[path = "management_shared_attempt_tests.rs"]
mod attempts;
use crate::management_shared_execution_types::*;
#[path = "management_shared_activity_tests.rs"]
mod activity_tests;
#[path="management_shared_ticket_tests.rs"]
mod ticket_tests;
#[path = "management_shared_intake_tests.rs"]
mod intake_tests;
#[path="management_shared_demand_tests.rs"]
mod demand_tests;

async fn prepared_pool(durable: bool) -> (Fixture, crate::management_pool_types::PoolAgentRequest, PendingManagedAssignment, MatrixFixture) {
    use crate::management_pool_types::DecidePoolAgent;
    use crate::management_tests::allocations::pool_agents;
    let mut f=pool_agents::isolated_fixture().await;
    f.policy=f.policy.with_agent_admission(true).with_agent_executor(true)
        .with_agent_matrix_provisioning(true).with_shared_execution(true).with_shared_successors(true)
        .with_shared_durable_intake(durable).with_shared_demands(durable);
    let pool=pool_agents::pool(&f,&f.outsider.p).await;
    let request=f.domain.request_pool_agent(&f.outsider.p,&f.policy,&pool.contribution.id,pool_agents::input(&pool,"pool_runtime")).await.unwrap();
    f.domain.decide_pool_agent(&f.owner.p,&f.policy,&request.id,DecidePoolAgent {command_id:"pool_runtime_approve".into(),expected_revision:1,decision:Decision::Approve}).await.unwrap();
    let delivery=poll(&f).await;
    assert!(delivery.execution_allowed);
    let assignment=f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,id(&delivery),reservation(&delivery)).await.unwrap().assignment.unwrap();
    let check=f.domain.poll_managed_agent_matrix(&f.owner.p,&f.policy,Default::default()).await.unwrap().unwrap();
    assert!(check.execution_allowed);
    assert!(check.command.space_id.is_empty());
    assert_eq!(check.command.reserve_command.room_id,request.room_id);
    let observation=ManagedAgentMatrixObservation {version:1,command_id:check.command.command_id.clone(),command_digest:check.command_digest.clone(),assignment_id:assignment.id.clone(),agent_reservation_id:assignment.agent_reservation_id.clone(),reserve_command_digest:check.command.reserve_command_digest.clone(),reserve_receipt_digest:assignment.reserve_receipt_digest.clone(),observed_ledger_revision:100,eligible:true,reason:None,valid_until_ms:check.command.expires_at_ms};
    f.domain.record_managed_agent_matrix(&f.owner.p,&f.policy,&check.command.command_id,AgentMatrixResultInput {command_digest:check.command_digest,token:check.token,observation}).await.unwrap();
    let events=vec![
        json!({"type":"m.room.create","state_key":"","content":{}}),
        json!({"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}}),
        json!({"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}}),
        json!({"type":"m.room.guest_access","state_key":"","content":{"guest_access":"forbidden"}}),
        json!({"type":"m.room.member","state_key":f.outsider.p.mxid,"content":{"membership":"join"}}),
        json!({"type":"m.room.member","state_key":SERVICE,"content":{"membership":"invite"}}),
    ];
    let matrix=matrix_fixture_from_states(HashMap::from([(request.room_id.clone(),events)])).await;
    f.domain.reconcile_managed_agent_matrix(&assignment.id,&matrix.gateway,&matrix.matrix,Some(&f.policy)).await.unwrap();
    let membership=f.domain.managed_agent_matrix(&f.outsider.p,&f.policy,&request.id).await.unwrap().unwrap();
    assert_eq!(membership.state,"matrix_verified");
    let calls = matrix.calls.lock().unwrap().clone();
    // A newly created pool room invites the service; it has not joined yet.
    // The service must join before it can invite and join the Agent puppet.
    assert_eq!(calls.iter().filter(|p| p.contains("/join/")).count(), 2);
    assert!(calls.as_slice().first().unwrap().contains("/join/"));
    (f, request, assignment, matrix)
}

/// A carried pool Agent's activity lease follows its pool's capacity chain:
/// with chain lifetimes on it outlasts the month its reserve command names.
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn carried_pool_agent_activity_lease_follows_the_capacity_chain() {
    use crate::management_pool_types::DecidePoolAgent;
    use crate::management_tests::allocations::pool_agents::{self, succession};
    let mut f=succession::succession_fixture().await;
    f.policy=f.policy.with_agent_matrix_provisioning(true).with_shared_execution(true).with_shared_successors(true);
    let pool=admitted(&f).await;
    // The root's month ends in 20 s; its renewal follows without a gap.
    let (root,renewal)=succession::seed_short_chain(&f,&pool,&f.outsider.p,20_000).await;
    let request=f.domain.request_pool_agent(&f.outsider.p,&f.policy,&pool.contribution.id,pool_agents::input(&pool,"chain_runtime")).await.unwrap();
    f.domain.decide_pool_agent(&f.owner.p,&f.policy,&request.id,DecidePoolAgent {command_id:"chain_runtime_approve".into(),expected_revision:1,decision:Decision::Approve}).await.unwrap();
    let delivery=poll(&f).await;
    let assignment=f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,id(&delivery),reservation(&delivery)).await.unwrap().assignment.unwrap();
    let check=f.domain.poll_managed_agent_matrix(&f.owner.p,&f.policy,Default::default()).await.unwrap().unwrap();
    // Matrix checks are capped by the chain lifetime, not the reserve expiry.
    assert!(check.command.expires_at_ms<=renewal.period_end_ms);
    let observation=ManagedAgentMatrixObservation {version:1,command_id:check.command.command_id.clone(),command_digest:check.command_digest.clone(),assignment_id:assignment.id.clone(),agent_reservation_id:assignment.agent_reservation_id.clone(),reserve_command_digest:check.command.reserve_command_digest.clone(),reserve_receipt_digest:assignment.reserve_receipt_digest.clone(),observed_ledger_revision:100,eligible:true,reason:None,valid_until_ms:check.command.expires_at_ms};
    f.domain.record_managed_agent_matrix(&f.owner.p,&f.policy,&check.command.command_id,AgentMatrixResultInput {command_digest:check.command_digest,token:check.token,observation}).await.unwrap();
    let events=vec![
        json!({"type":"m.room.create","state_key":"","content":{}}),
        json!({"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}}),
        json!({"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}}),
        json!({"type":"m.room.guest_access","state_key":"","content":{"guest_access":"forbidden"}}),
        json!({"type":"m.room.member","state_key":f.outsider.p.mxid,"content":{"membership":"join"}}),
        json!({"type":"m.room.member","state_key":SERVICE,"content":{"membership":"invite"}}),
    ];
    let matrix=matrix_fixture_from_states(HashMap::from([(request.room_id.clone(),events)])).await;
    f.domain.reconcile_managed_agent_matrix(&assignment.id,&matrix.gateway,&matrix.matrix,Some(&f.policy)).await.unwrap();
    let runtime=shared_poll(&f).await;
    assert!(runtime.execution_allowed);
    assert_eq!(runtime.command.reserve_command.expires_at_ms,root.period_end_ms);
    let activity=save(&f,&runtime,"qualified",101).await.unwrap().activity.unwrap();
    // The lease is the usual 30 s session window, past the root's month end:
    // the renewal already covers that instant.
    assert!(activity.expires_at_ms>root.period_end_ms,"{} <= {}",activity.expires_at_ms,root.period_end_ms);
    assert!(activity.expires_at_ms<=renewal.period_end_ms);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_agent_reaches_private_room_and_runtime_without_any_project() {
    let (f, request, _assignment, matrix) = Box::pin(prepared_pool(false)).await;
    let runtime=shared_poll(&f).await;
    assert!(runtime.execution_allowed);
    assert_eq!(runtime.command.reserve_command.room_id,request.room_id);
    assert!(runtime.command.reserve_command.pool_grant.is_some());
    assert!(runtime.command.reserve_command.project_id.is_empty());
    let activity=save(&f,&runtime,"qualified",101).await.unwrap().activity.unwrap();
    let event=crate::transport::RoutedEvent {event_id:"$pool_text".into(),room_id:request.room_id.clone(),sender_mxid:f.outsider.p.mxid.clone(),body:"Hello pool Agent".into(),mentioned_mxids:Default::default(),thread_root:None,encrypted:false,is_edit:false};
    f.domain.route_shared_event(&f.policy,&event,now_ms(),&matrix.gateway).await.unwrap();
    let work=f.domain.poll_shared_work(&f.owner.p,&f.policy,activity.authority.clone(),&matrix.gateway).await.unwrap().unwrap();
    let input=SharedWorkInput {authority:activity.authority.clone(),work_id:work.id.clone(),work_digest:allocation_digest(&work).unwrap()};
    f.domain.acknowledge_shared_work(&f.owner.p,&f.policy,input.clone(),&matrix.gateway).await.unwrap();
    assert!(f.domain.start_shared_work(&f.owner.p,&f.policy,input,&matrix.gateway).await.unwrap().newly_started);
    let ticket=work::ticket(&f,&runtime,&work).await;
    f.domain.record_shared_work_result(&f.owner.p,&f.policy,&work.id,work::outcome(&runtime,&work,&ticket,102,"completed")).await.unwrap();
    assert!(f.domain.shared_execution_status(&f.outsider.p,&f.policy,&request.id).await.unwrap().unwrap().work_available);
    let stopped=attempts::stop(&f,&runtime,103).await;
    let next=f.domain.start_shared_attempt(&f.owner.p,&f.policy,attempts::start(&runtime,&stopped),&matrix.gateway).await.unwrap();
    assert_eq!(next.command.process_attempt,2);
    f.domain.stop_pool_agent(&f.owner.p,&request.id,RevokeInput {command_id:"stop_pool_runtime".into(),expected_revision:2}).await.unwrap();
    assert!(!f.domain.shared_execution_status(&f.outsider.p,&f.policy,&request.id).await.unwrap().unwrap().work_available);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_agent_topup_is_approved_only_by_resource_owner_and_preserves_original_reservation() {
    use crate::management_agent_budget_types::*;
    let (mut f, request, _, _) = Box::pin(prepared_pool(true)).await;
    f.policy=f.policy.with_agent_budget_deltas(true);
    let b=f.domain.agent_budget(&f.outsider.p,&f.policy,&request.id).await.unwrap();
    assert!(b.can_request);
    assert!(f.domain.agent_budget(&f.admin.p,&f.policy,&request.id).await.is_err());
    let top=f.domain.request_agent_topup(&f.outsider.p,&f.policy,&request.id,RequestAgentTopUp { approve_immediately: false, owner_funded: false,
        command_id:"pool_topup_request".into(),expected_request_revision:b.request_revision,
        expected_assignment_generation:b.assignment_generation,expected_budget_revision:b.budget_revision,
        expected_budget_digest:b.budget_digest,add_tokens:100,purpose:"Continue the same pool Agent".into(),
    }).await.unwrap().top_up;
    assert!(!top.can_decide);
    let owner_view=f.domain.agent_topup(&f.owner.p,&f.policy,&top.id).await.unwrap();
    assert!(owner_view.can_decide);
    let decide=DecideAgentTopUp {command_id:"pool_topup_decision".into(),expected_revision:top.revision,expected_authority_revision:top.authority_revision,decision:Decision::Approve};
    assert!(f.domain.decide_agent_topup(&f.outsider.p,&f.policy,&top.id,decide.clone()).await.is_err());
    assert!(f.domain.decide_agent_topup(&f.admin.p,&f.policy,&top.id,decide.clone()).await.is_err());
    f.domain.decide_agent_topup(&f.owner.p,&f.policy,&top.id,decide).await.unwrap();
    let d=f.domain.poll_agent_budget(&f.owner.p,&f.policy,ExecutorPollInput::default()).await.unwrap().unwrap();
    assert!(d.application_allowed);
    let c=&d.command;
    assert!(c.reserve_command.pool_grant.is_some());
    assert_eq!(c.approving_administrator_mxid,f.owner.p.mxid);
    assert_eq!(c.reserve_command.requested_tokens,100);
    let mut forged=c.clone();forged.approving_administrator_mxid=f.outsider.p.mxid.clone();
    assert!(forged.validate("test.local").is_err());
    let receipt=AgentBudgetReceipt {version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),top_up_id:c.top_up_id.clone(),agent_reservation_id:c.reserve_receipt.agent_reservation_id.clone().unwrap(),assignment_id:c.assignment_id.clone(),original_reserve_receipt_digest:c.reserve_receipt_digest.clone(),state:"applied".into(),expected_budget_revision:c.expected_budget_revision,previous_budget_digest:c.expected_budget_digest.clone(),budget_revision:2,budget_digest:next_budget_digest(c,200).unwrap(),previous_ceiling_tokens:100,applied_delta_tokens:100,effective_ceiling_tokens:200,ledger_revision:200,period_key:c.reserve_command.period_key.clone(),reason:None};
    f.domain.record_agent_budget(&f.owner.p,&c.command_id,AgentBudgetResult {command_digest:d.command_digest,token:d.token,receipt}).await.unwrap();
    let b=f.domain.agent_budget(&f.outsider.p,&f.policy,&request.id).await.unwrap();
    assert_eq!((b.original_ceiling_tokens,b.effective_ceiling_tokens,b.budget_revision),(100,200,2));
    assert_eq!(f.domain.pool_agent_request(&f.outsider.p,&request.id).await.unwrap().requested_tokens,100);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_owner_can_fund_beyond_published_grant_but_manager_cannot() {
    use crate::management_agent_budget_types::*;
    let (mut f, request, _, _) = Box::pin(prepared_pool(true)).await;
    f.policy=f.policy.with_agent_budget_deltas(true);
    let b=f.domain.agent_budget(&f.owner.p,&f.policy,&request.id).await.unwrap();
    assert!(!b.can_request && b.can_owner_fund);
    assert!(f.domain.agent_budget(&f.admin.p,&f.policy,&request.id).await.is_err());
    let input=RequestAgentTopUp { approve_immediately: false, owner_funded: true,
        command_id:"pool_topup_request".into(),expected_request_revision:b.request_revision,
        expected_assignment_generation:b.assignment_generation,expected_budget_revision:b.budget_revision,
        expected_budget_digest:b.budget_digest,add_tokens:2000000,purpose:"Continue the same pool Agent".into(),
    };
    assert!(f.domain.request_agent_topup(&f.outsider.p,&f.policy,&request.id,input.clone()).await.is_err());
    assert!(f.domain.request_agent_topup(&f.admin.p,&f.policy,&request.id,input.clone()).await.is_err());
    let mut delegated=input.clone();delegated.owner_funded=false;
    assert!(f.domain.request_agent_topup(&f.outsider.p,&f.policy,&request.id,delegated).await.is_err());
    let top=f.domain.request_agent_topup(&f.owner.p,&f.policy,&request.id,input.clone()).await.unwrap().top_up;
    assert_eq!(f.domain.request_agent_topup(&f.owner.p,&f.policy,&request.id,input).await.unwrap().top_up.id,top.id);
    assert!(top.can_decide && top.can_cancel && top.owner_funded);
    let owner_view=f.domain.agent_topup(&f.owner.p,&f.policy,&top.id).await.unwrap();
    assert!(owner_view.can_decide);
    let decide=DecideAgentTopUp {command_id:"pool_topup_decision".into(),expected_revision:top.revision,expected_authority_revision:top.authority_revision,decision:Decision::Approve};
    assert!(f.domain.decide_agent_topup(&f.outsider.p,&f.policy,&top.id,decide.clone()).await.is_err());
    assert!(f.domain.decide_agent_topup(&f.admin.p,&f.policy,&top.id,decide.clone()).await.is_err());
    f.domain.decide_agent_topup(&f.owner.p,&f.policy,&top.id,decide).await.unwrap();
    let d=f.domain.poll_agent_budget(&f.owner.p,&f.policy,ExecutorPollInput::default()).await.unwrap().unwrap();
    assert!(d.application_allowed);
    let c=&d.command;
    assert!(c.owner_funded && c.version==2);
    assert!(c.add_tokens>c.reserve_command.pool_grant.as_ref().unwrap().allocated_tokens);
    assert_eq!(c.approving_administrator_mxid,f.owner.p.mxid);
    assert_eq!(c.reserve_command.requested_tokens,100);
    let mut forged=c.clone();forged.approving_administrator_mxid=f.outsider.p.mxid.clone();
    assert!(forged.validate("test.local").is_err());
    let receipt=AgentBudgetReceipt {version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),top_up_id:c.top_up_id.clone(),agent_reservation_id:c.reserve_receipt.agent_reservation_id.clone().unwrap(),assignment_id:c.assignment_id.clone(),original_reserve_receipt_digest:c.reserve_receipt_digest.clone(),state:"applied".into(),expected_budget_revision:c.expected_budget_revision,previous_budget_digest:c.expected_budget_digest.clone(),budget_revision:2,budget_digest:next_budget_digest(c,200).unwrap(),previous_ceiling_tokens:100,applied_delta_tokens:2000000,effective_ceiling_tokens:2000100,ledger_revision:200,period_key:c.reserve_command.period_key.clone(),reason:None};
    f.domain.record_agent_budget(&f.owner.p,&c.command_id,AgentBudgetResult {command_digest:d.command_digest,token:d.token,receipt}).await.unwrap();
    let b=f.domain.agent_budget(&f.outsider.p,&f.policy,&request.id).await.unwrap();
    assert_eq!((b.original_ceiling_tokens,b.effective_ceiling_tokens,b.budget_revision),(100,2000100,2));
    assert_eq!(f.domain.pool_agent_request(&f.outsider.p,&request.id).await.unwrap().requested_tokens,100);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_chat_status_reports_interruption_and_budget_block_without_cross_user_leakage() {
    let (f, request, assignment, matrix) = Box::pin(prepared_pool(true)).await;
    let runtime = shared_poll(&f).await;
    let activity = save(&f, &runtime, "qualified", 101).await.unwrap().activity.unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let mut event = crate::transport::RoutedEvent {
        event_id: "$research_status".into(), room_id: request.room_id.clone(),
        sender_mxid: f.outsider.p.mxid.clone(), body: "Research this".into(),
        mentioned_mxids: Default::default(), thread_root: Some("$research_thread".into()),
        encrypted: false, is_edit: false,
    };
    f.domain.route_shared_event(&f.policy, &event, now_ms(), &matrix.gateway).await.unwrap();
    let query = json!({"roomId":request.room_id});
    let value = f.domain.shared_approvals(&f.outsider.p, serde_json::from_value(query.clone()).unwrap()).await.unwrap();
    assert_eq!(value["executions"][0]["state"], "pending");
    assert_eq!(value["executions"][0]["threadRoot"], "$research_thread");
    assert!(value["executions"][0].get("body").is_none());
    let work = f.domain.poll_shared_work(&f.owner.p, &f.policy, activity.authority.clone(), &matrix.gateway).await.unwrap().unwrap();
    let input = SharedWorkInput {authority: activity.authority.clone(), work_id: work.id.clone(), work_digest: allocation_digest(&work).unwrap()};
    f.domain.acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &matrix.gateway).await.unwrap();
    f.domain.start_shared_work(&f.owner.p, &f.policy, input, &matrix.gateway).await.unwrap();
    let value = f.domain.shared_approvals(&f.outsider.p, serde_json::from_value(query.clone()).unwrap()).await.unwrap();
    assert_eq!(value["executions"][0]["state"], "started");
    attempts::stop(&f, &runtime, 103).await;
    let value = f.domain.shared_approvals(&f.outsider.p, serde_json::from_value(query.clone()).unwrap()).await.unwrap();
    assert_eq!(value["executions"][0]["state"], "unknown");
    event.event_id = "$budget_block_status".into();
    f.domain.route_shared_event(&f.policy, &event, now_ms(), &matrix.gateway).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_shared_intake SET state='blocked',reason='budget_blocked' WHERE assignment_id=$1 AND event_id=$2")
        .bind::<Text,_>(&assignment.id).bind::<Text,_>(&event.event_id).execute(&mut db).await.unwrap();
    for p in [&f.owner.p,&f.outsider.p] {
        let value = f.domain.shared_approvals(p, serde_json::from_value(query.clone()).unwrap()).await.unwrap();
        assert_eq!(value["executions"][0]["eventId"], event.event_id);
        assert_eq!(value["executions"][0]["reason"], "budget_blocked");
    }
    let hidden = f.domain.shared_approvals(&f.admin.p, serde_json::from_value(query).unwrap()).await.unwrap();
    assert_eq!(hidden["executions"], json!([]));
    for room in [json!("!elsewhere:example.test"),Value::Null] {
        let hidden = f.domain.shared_approvals(&f.outsider.p, serde_json::from_value(json!({"roomId":room})).unwrap()).await.unwrap();
        assert_eq!(hidden["executions"], json!([]));
    }
    assert!(sql_query("SELECT count(*)=1 AS matched FROM hagency_agent_v1.management_shared_work WHERE id=$1 AND state='unknown'")
        .bind::<Text,_>(&work.id).get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_agent_expired_owner_session_waits_without_revoking_reserved_agent() {
    #[derive(QueryableByName)]
    struct SavedState { #[diesel(sql_type=Text)] payload: String }
    let (f, request, assignment, matrix) = Box::pin(prepared_pool(false)).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    // Arrange natural expiry without the separate explicit-shortening/revocation trigger.
    db.batch_execute("ALTER TABLE hagency_agent_v1.sessions DISABLE TRIGGER USER").await.unwrap();
    sql_query("UPDATE hagency_agent_v1.sessions SET valid_until_ms=(extract(epoch from clock_timestamp())*1000)::bigint-1 WHERE id=$1")
        .bind::<Text,_>(&f.owner.p.session_id).execute(&mut db).await.unwrap();
    db.batch_execute("ALTER TABLE hagency_agent_v1.sessions ENABLE TRIGGER USER").await.unwrap();
    f.domain.reconcile_managed_agent_matrix(&assignment.id,&matrix.gateway,&matrix.matrix,Some(&f.policy)).await.unwrap();
    let row: SavedState=sql_query("SELECT i.state AS payload FROM hagency_agent_v1.management_agent_intents i WHERE request_id=$1")
        .bind::<Text,_>(&request.id).get_result(&mut db).await.unwrap();
    assert_ne!(row.payload,"fenced");
    let fences:Count=sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.fence'")
        .bind::<Text,_>(&request.id).get_result(&mut db).await.unwrap();
    assert_eq!(fences.count,0);
    assert!(f.domain.poll_shared_runtime(&f.owner.p,&f.policy,ExecutorPollInput::default()).await.is_err());
    sql_query("UPDATE hagency_agent_v1.sessions SET valid_until_ms=$2 WHERE id=$1")
        .bind::<Text,_>(&f.owner.p.session_id).bind::<BigInt,_>(crate::api::now_ms()+1_000_000).execute(&mut db).await.unwrap();
    f.domain.reconcile_managed_agent_matrix(&assignment.id,&matrix.gateway,&matrix.matrix,Some(&f.policy)).await.unwrap();
    let resumed=shared_poll(&f).await;
    assert_eq!(resumed.command.assignment_id,assignment.id);
    assert!(resumed.execution_allowed);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_private_chat_without_mentions_retains_promotes_and_resumes_only_applicant_messages() {
    use crate::management_shared_demand_types::SharedDemandStartInput;
    use crate::management_shared_intake_types::SharedIntakeQuery;
    let (f, request, assignment, matrix) = Box::pin(prepared_pool(true)).await;
    let runtime = shared_poll(&f).await;
    let activity = save(&f, &runtime, "qualified", 101).await.unwrap().activity.unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    async fn received(db: &mut AsyncPgConnection) -> i64 {
        sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS count")
            .get_result::<Count>(db).await.unwrap().count
    }
    let mut event = crate::transport::RoutedEvent {
        event_id: "$plain_pool_chat".into(), room_id: request.room_id.clone(),
        sender_mxid: f.outsider.p.mxid.clone(), body: "你好".into(),
        mentioned_mxids: Default::default(), thread_root: None, encrypted: false, is_edit: false,
    };
    let puppet = f.domain.managed_agent_matrix(&f.outsider.p, &f.policy, &request.id)
        .await.unwrap().unwrap().puppet_mxid;
    for (i, sender) in [f.owner.p.mxid.as_str(), f.admin.p.mxid.as_str(), puppet.as_str(), SERVICE].iter().enumerate() {
        let mut other = event.clone();
        other.event_id = format!("$ignored_pool_sender_{i}");
        other.sender_mxid = sender.to_string();
        f.domain.route_shared_event(&f.policy, &other, received(&mut db).await, &matrix.gateway).await.unwrap();
    }
    assert!(f.domain.shared_intake_history(&f.outsider.p, &assignment.id, SharedIntakeQuery {after: None})
        .await.unwrap().items.is_empty());

    // The applicant may invite others; a joined member is addressed only by mention.
    member(&mut matrix.states.lock().unwrap(), &request.room_id, "@member:example.test", "join");
    let mut plain = event.clone(); plain.event_id = "$pool_member_plain".into();
    plain.sender_mxid = "@member:example.test".into();
    f.domain.route_shared_event(&f.policy, &plain, received(&mut db).await, &matrix.gateway).await.unwrap();
    assert!(f.domain.shared_intake_history(&f.outsider.p, &assignment.id, SharedIntakeQuery {after: None})
        .await.unwrap().items.is_empty());
    member(&mut matrix.states.lock().unwrap(), &request.room_id, "@member:example.test", "leave");

    let time = received(&mut db).await;
    for _ in 0..2 {
        f.domain.route_shared_event(&f.policy, &event, time, &matrix.gateway).await.unwrap();
    }
    let history = f.domain.shared_intake_history(&f.outsider.p, &assignment.id, SharedIntakeQuery {after: None}).await.unwrap();
    assert_eq!(history.items.iter().filter(|r| r.event_id == event.event_id).count(), 1);
    assert!(history.items.iter().any(|r| r.event_id == event.event_id && r.state == "pending"));
    #[derive(QueryableByName)] struct Original { #[diesel(sql_type=Text)] payload: String }
    let original = sql_query("SELECT event_payload AS payload FROM hagency_agent_v1.management_shared_intake WHERE assignment_id=$1 AND event_id=$2")
        .bind::<Text, _>(&assignment.id).bind::<Text, _>(&event.event_id).get_result::<Original>(&mut db).await.unwrap();
    let original: Value = serde_json::from_str(&original.payload).unwrap();
    assert_eq!(original["mentioned_mxids"], json!([])); // Do not invent a mention in the event journal.

    let work = f.domain.poll_shared_work(&f.owner.p, &f.policy, activity.authority.clone(), &matrix.gateway).await.unwrap().unwrap();
    assert_eq!(work.event_id, event.event_id);
    assert_eq!(work.body, "你好");
    assert_eq!(f.domain.poll_shared_work(&f.owner.p, &f.policy, activity.authority.clone(), &matrix.gateway).await.unwrap().unwrap().id, work.id);
    let input = SharedWorkInput {authority: activity.authority.clone(), work_id: work.id.clone(), work_digest: allocation_digest(&work).unwrap()};
    f.domain.acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &matrix.gateway).await.unwrap();
    assert!(f.domain.start_shared_work(&f.owner.p, &f.policy, input, &matrix.gateway).await.unwrap().newly_started);
    let ticket = work::ticket(&f, &runtime, &work).await;
    f.domain.record_shared_work_result(&f.owner.p, &f.policy, &work.id, work::outcome(&runtime, &work, &ticket, 102, "completed")).await.unwrap();
    let stopped = attempts::stop(&f, &runtime, 103).await;

    event.thread_root = Some(event.event_id.clone());
    event.event_id = "$plain_pool_followup".into(); event.body = "继续".into();
    f.domain.route_shared_event(&f.policy, &event, received(&mut db).await, &matrix.gateway).await.unwrap();
    let demand = f.domain.poll_shared_demand(&f.owner.p, &f.policy, Default::default()).await.unwrap().demand.unwrap();
    let next = f.domain.start_shared_demand(&f.owner.p, &f.policy, &demand.offer.demand_id,
        SharedDemandStartInput {demand_digest: demand.demand_digest, expected_control_revision: demand.offer.control_revision, start: attempts::start(&runtime, &stopped)}, &matrix.gateway).await.unwrap();
    let activity = f.domain.record_shared_attempt_process(&f.owner.p, &f.policy, &next.command.command_id,
        attempts::result(&next, "qualified", next.command.budget_observation.observed_ledger_revision + 1)).await.unwrap().activity.unwrap();
    let followup = f.domain.poll_shared_work(&f.owner.p, &f.policy, activity.authority, &matrix.gateway).await.unwrap().unwrap();
    assert_eq!(followup.event_id, event.event_id);
    assert_eq!(followup.body, "继续");
    assert_eq!(followup.thread_root, "$plain_pool_chat");

    f.domain.change_shared_run_control(&f.outsider.p, &f.policy, &assignment.id,
        intake_tests::change("pause_pool_chat", 1, "paused"), &matrix.gateway).await.unwrap();
    event.event_id = "$plain_pool_paused".into();
    f.domain.route_shared_event(&f.policy, &event, received(&mut db).await, &matrix.gateway).await.unwrap();
    let history = f.domain.shared_intake_history(&f.outsider.p, &assignment.id, SharedIntakeQuery {after: None}).await.unwrap();
    assert!(history.items.iter().any(|r| r.event_id == event.event_id && r.reason.as_deref() == Some("paused_at_receipt")));
    let resumed = f.domain.change_shared_run_control(&f.outsider.p, &f.policy, &assignment.id,
        intake_tests::change("resume_pool_chat", 2, "running"), &matrix.gateway).await.unwrap();
    assert_eq!(resumed.control.origin, "manager");
    let paused = f.domain.change_shared_run_control(&f.owner.p, &f.policy, &assignment.id,
        intake_tests::change("owner_pauses_pool_chat", 3, "paused"), &matrix.gateway).await.unwrap();
    assert_eq!(paused.control.origin, "executor_stop");
    assert!(f.domain.change_shared_run_control(&f.owner.p, &f.policy, &assignment.id,
        intake_tests::change("owner_cannot_resume_pool_chat", 4, "running"), &matrix.gateway).await.is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_schema_upgrade_preserves_existing_capacity_and_rolls_back_atomically() {
    let source=std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
    let name=format!("pool_upgrade_{}",&crate::hash(&secret_token())[..16]);
    let mut control=AsyncPgConnection::establish(&source).await.unwrap();
    control.batch_execute(&format!("CREATE DATABASE {name}")).await.unwrap();
    let mut url=url::Url::parse(&source).unwrap();url.set_path(&format!("/{name}"));
    let f=fixture_at(url.to_string()).await;
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    // Keep an existing grant and approved plan while testing the nullable schema.
    let mut policy=f.policy.clone();policy=policy.with_project_reservations(true);
    let f=Fixture {policy,..f};
    let contribution=admitted(&f).await;
    let plan=approved_plan(&f,&contribution,&f.outsider.p).await;
    let input=grant_input(&f,&contribution,&f.outsider.p).await;
    let expected=serde_json::to_string(&input.receipt).unwrap();
    let grant=f.domain.publish_management_grant(&f.owner.p,&f.policy,&contribution.contribution.id,input).await.unwrap();
    db.batch_execute(include_str!("management_pool_agents_remove_test.sql")).await.unwrap();
    db.batch_execute("ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=23;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=23);CREATE INDEX pool_agent_room_unique ON hagency_agent_v1.management_requests(id)").await.unwrap();
    assert!(DomainStore::open(&f.url,"example.test","_hagency_test_").await.is_err());
    #[derive(QueryableByName)] struct Check {#[diesel(sql_type=Bool)] matched:bool}
    assert!(sql_query("SELECT version=23 AND NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='management_agent_requests' AND column_name='pool_grant_id') AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP INDEX hagency_agent_v1.pool_agent_room_unique").await.unwrap();
    let upgraded=DomainStore::open(&f.url,"example.test","_hagency_test_").await.unwrap();
    #[derive(QueryableByName)] struct Raw {#[diesel(sql_type=Text)] value:String}
    let raw=sql_query("SELECT receipt AS value FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&grant.id).get_result::<Raw>(&mut db).await.unwrap();
    assert_eq!(raw.value,expected);
    assert_eq!(upgraded.management_project(&f.outsider.p,&f.policy,&plan.id).await.unwrap().id,plan.id);
    drop(upgraded);drop(db);drop(f);
    control.batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)")).await.unwrap();
}

pub(super) async fn remove_migration(db: &mut AsyncPgConnection) {
    work::remove_migration(db).await;

    db.batch_execute("DROP TRIGGER fence_shared_runtime ON hagency_agent_v1.management_shared_assignments;DROP TRIGGER exclude_shared_runtime_v1 ON hagency_agent_v1.management_agent_runtime_commands;DROP TABLE hagency_agent_v1.management_shared_activity_renewals,hagency_agent_v1.management_shared_process_receipts,hagency_agent_v1.management_shared_runtimes;DROP FUNCTION hagency_agent_v1.fence_shared_runtime(),hagency_agent_v1.exclude_shared_runtime_v1(),hagency_agent_v1.retain_shared_runtime(),hagency_agent_v1.validate_shared_runtime();ALTER TABLE hagency_agent_v1.management_deployment DROP CONSTRAINT management_deployment_version_check;UPDATE hagency_agent_v1.management_deployment SET version=11;ALTER TABLE hagency_agent_v1.management_deployment ADD CONSTRAINT management_deployment_version_check CHECK(version=11)").await.unwrap();
}
async fn prepared() -> (
    Fixture,
    ManagedAgentRequest,
    Actor,
    PendingManagedAssignment,
    MatrixFixture,
) {
    let (mut f, r, admin, a, m) = Box::pin(ready()).await;
    f.policy = f.policy.with_shared_execution(true);
    (f, r, admin, a, m)
}
async fn shared_poll(f: &Fixture) -> SharedRuntimeDelivery {
    f.domain
        .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap()
}
fn evidence(d: &SharedRuntimeDelivery, state: &str, revision: i64) -> SharedProcessInput {
    SharedProcessInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: SharedProcessReceipt {
            version: 1,
            observation_id: format!("{}_observation_{revision}", d.command.command_id),
            command_id: d.command.command_id.clone(),
            command_digest: d.command_digest.clone(),
            assignment_id: d.command.assignment_id.clone(),
            agent_reservation_id: d.command.agent_reservation_id.clone(),
            ledger_revision: revision,
            process_digest: Some("a".repeat(64)),
            state: state.into(),
            policy_digest: (state == "qualified").then(|| "b".repeat(64)),
        },
    }
}
async fn save(
    f: &Fixture,
    d: &SharedRuntimeDelivery,
    state: &str,
    revision: i64,
) -> crate::Result<SharedRuntimeAcknowledgement> {
    f.domain
        .record_shared_process(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            evidence(d, state, revision),
        )
        .await
}
async fn history(f: &Fixture, d: &SharedRuntimeDelivery) -> SharedRuntimeDelivery {
    f.domain
        .recover_shared_runtime(
            &f.owner.p,
            &f.policy,
            &d.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: d.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap()
}
async fn qualify(f: &Fixture) -> (SharedRuntimeDelivery, SharedActivityLease) {
    let d = shared_poll(f).await;
    let activity = save(f, &d, "qualified", 200)
        .await
        .unwrap()
        .activity
        .unwrap();
    (d, activity)
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_requires_closed_policy_current_codex_and_real_matrix() {
    let (mut f, r, _admin, a) = Box::pin(prepared_matrix()).await;
    f.policy = f
        .policy
        .with_shared_execution(true)
        .with_agent_runtime_provisioning(true);
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.domain
            .poll_shared_runtime(
                &f.owner.p,
                &f.policy.clone().with_shared_execution(false),
                ExecutorPollInput::default()
            )
            .await
            .is_err()
    );
    let m = matrix_fixture(&f, &r).await;
    permit(&f).await;
    f.domain
        .reconcile_managed_agent_matrix(&a.id, &m.gateway, &m.matrix, Some(&f.policy))
        .await
        .unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query("UPDATE hagency_agent_v1.management_contributions SET runtime='claude' WHERE id=(SELECT contribution_id FROM hagency_agent_v1.management_projects WHERE id=$1)").bind::<Text,_>(&r.project_id).execute(&mut db).await.unwrap();
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    let (f, r, admin, _a, m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    assert!(d.execution_allowed);
    assert!(!d.cleanup_allowed);
    assert_eq!(d.command.policy, SHARED_TEXT_POLICY);
    assert_eq!(d.command_digest, allocation_digest(&d.command).unwrap());
    assert!(
        f.domain
            .poll_managed_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    for actor in [&f.outsider.p, &f.coordinator.p, &f.admin.p, &admin.p] {
        assert!(
            f.domain
                .poll_shared_runtime(actor, &f.policy, ExecutorPollInput::default())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            f.domain
                .record_shared_process(
                    actor,
                    &f.policy,
                    &d.command.command_id,
                    evidence(&d, "qualified", 200)
                )
                .await
                .is_err()
        );
    }
    let service = svc(&f, m.gateway.clone());
    let mut raw = serde_json::to_value(evidence(&d, "qualified", 200)).unwrap();
    raw["canExecute"] = json!(true);
    post(
        &service,
        &format!("shared-execution/runtime/{}/result", d.command.command_id),
        &f.owner.token,
        raw,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let mut bad = evidence(&d, "qualified", 200);
    bad.receipt.assignment_id = "other_assignment".into();
    assert!(
        f.domain
            .record_shared_process(&f.owner.p, &f.policy, &d.command.command_id, bad)
            .await
            .is_err()
    );
    let ack = save(&f, &d, "qualified", 200).await.unwrap();
    assert!(ack.activity.is_some());
    let status = f
        .domain
        .shared_execution_status(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.state, "qualified");
    assert!(status.work_available);
    assert!(!status.capacity_released);
    assert!(status.recent_work.is_empty());
    assert!(
        f.domain
            .shared_execution_status(&admin.p, &f.policy, &r.id)
            .await
            .unwrap()
            .is_some()
    );
    for actor in [&f.owner.p, &f.admin.p, &f.coordinator.p] {
        assert!(
            f.domain
                .shared_execution_status(actor, &f.policy, &r.id)
                .await
                .is_err()
        );
    }
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT a.state='suspended' AND a.execution_device_id IS NULL AND a.owner_user_id=$2 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings b WHERE b.agent_id=a.id) AS matched FROM hagency_agent_v1.agents a WHERE a.id=$1").bind::<Text,_>(&r.proposed_agent_id).bind::<Text,_>(&f.outsider.p.user_id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_v1_already_issued_is_never_replaced() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let v1 = poll_runtime(&f).await;
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(poll_runtime(&f).await.command_digest, v1.command_digest);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_historical_receipt_cannot_create_activity_or_respawn() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let old = history(&f, &d).await;
    assert!(!old.execution_allowed);
    assert!(!old.cleanup_allowed);
    let ack = save(&f, &old, "qualified", 200).await.unwrap();
    assert!(ack.activity.is_none());
    assert_eq!(ack.state, "issued");
    let cleanup = shared_poll(&f).await;
    assert!(!cleanup.execution_allowed);
    assert!(!cleanup.cleanup_allowed);
    assert_eq!(cleanup.command_digest, d.command_digest);
    assert!(
        save(&f, &cleanup, "qualified", 200)
            .await
            .unwrap()
            .activity
            .is_none()
    );
    assert_eq!(
        save(&f, &cleanup, "stopped", 201).await.unwrap().state,
        "stopped"
    );
    assert!(save(&f, &cleanup, "qualified", 202).await.is_err());
    let old_execute=save(&f,&d,"qualified",200).await.unwrap();
    assert_eq!(old_execute.state,"stopped");
    assert!(old_execute.activity.is_none());
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    let recovered = history(&f, &d).await;
    assert_eq!(recovered.recorded.unwrap().state, "stopped");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_receipt_and_renewal_replay_never_extend_or_reuse_old_authority() {
    let (f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, first) = qualify(&f).await;
    let again = save(&f, &d, "qualified", 200)
        .await
        .unwrap()
        .activity
        .unwrap();
    assert_eq!(again.expires_at_ms, first.expires_at_ms);
    assert_eq!(again.authority.epoch, 1);
    let input = SharedActivityRenewInput {
        authority: again.authority.clone(),
        renewal_id: "renew_first".into(),
    };
    let renewed = f
        .domain
        .renew_shared_activity(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    assert_eq!(renewed.authority.epoch, 2);
    let replay = f
        .domain
        .renew_shared_activity(&f.owner.p, &f.policy, input.clone(), &m.gateway)
        .await
        .unwrap();
    assert_eq!(replay.expires_at_ms, renewed.expires_at_ms);
    assert_eq!(replay.authority.epoch, 2);
    let mut changed = input.clone();
    changed.authority = renewed.authority.clone();
    assert!(
        f.domain
            .renew_shared_activity(&f.owner.p, &f.policy, changed, &m.gateway)
            .await
            .is_err()
    );
    let next = SharedActivityRenewInput {
        authority: renewed.authority.clone(),
        renewal_id: "renew_second".into(),
    };
    let next = f
        .domain
        .renew_shared_activity(&f.owner.p, &f.policy, next, &m.gateway)
        .await
        .unwrap();
    assert_eq!(next.authority.epoch, 3);
    assert!(
        f.domain
            .renew_shared_activity(&f.owner.p, &f.policy, input.clone(), &m.gateway)
            .await
            .is_err()
    );
    let reused = SharedActivityRenewInput {
        authority: next.authority.clone(),
        renewal_id: input.renewal_id,
    };
    assert!(
        f.domain
            .renew_shared_activity(&f.owner.p, &f.policy, reused, &m.gateway)
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    sql_query(
        "UPDATE hagency_agent_v1.management_shared_runtimes SET activity_until_ms=1 WHERE id=$1",
    )
    .bind::<Text, _>(&d.command.command_id)
    .execute(&mut db)
    .await
    .unwrap();
    let expired = SharedActivityRenewInput {
        authority: next.authority,
        renewal_id: "renew_expired".into(),
    };
    assert!(
        f.domain
            .renew_shared_activity(&f.owner.p, &f.policy, expired, &m.gateway)
            .await
            .is_err()
    );
    assert!(
        save(&f, &d, "qualified", 200)
            .await
            .unwrap()
            .activity
            .is_none()
    );
    let cleanup = shared_poll(&f).await;
    assert!(cleanup.cleanup_allowed);
    assert!(!cleanup.execution_allowed);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_rechecks_matrix_and_revocation_after_network_await() {
    let (f, r, admin, _a, m) = Box::pin(prepared()).await;
    let (_d, activity) = qualify(&f).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Semaphore::new(0));
    let data = m.states.clone();
    let entered = started.clone();
    let barrier = resume.clone();
    let gateway = Gateway::new(
        Arc::new(move |room| {
            let data = data.clone();
            let entered = entered.clone();
            let barrier = barrier.clone();
            Box::pin(async move {
                entered.notify_one();
                let _ = barrier.acquire().await;
                data.lock()
                    .unwrap()
                    .get(&room)
                    .cloned()
                    .ok_or(crate::Error::NotFound("missing"))
            })
        }),
        SERVICE.into(),
    );
    let input = SharedActivityRenewInput {
        authority: activity.authority,
        renewal_id: "renew_racing_revoke".into(),
    };
    let waiting = f
        .domain
        .renew_shared_activity(&f.owner.p, &f.policy, input, &gateway);
    let revoke_after = async {
        started.notified().await;
        revoke(&f, &r, &admin).await;
        resume.close();
    };
    let (result, _) = tokio::join!(waiting, revoke_after);
    assert!(result.is_err());
    let cleanup = shared_poll(&f).await;
    assert!(cleanup.cleanup_allowed);
    assert!(!cleanup.execution_allowed);
    let old = history(&f, &cleanup).await;
    assert!(!old.execution_allowed);
    assert!(!old.cleanup_allowed);
    let late = save(&f, &old, "qualified", 200).await.unwrap();
    assert!(late.activity.is_none());
    assert_eq!(late.state, "cleanup_pending");
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_generation_rotation_is_cleanup_only() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let new = f
        .auth
        .register_device(
            &f.owner.session,
            RegisterDevice {
                installation_id: "macbook".into(),
                name: "Reauthorized".into(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    let p = f
        .auth
        .authenticate(&new.token, now_ms(), true)
        .await
        .unwrap();
    assert!(save(&f, &d, "qualified", 200).await.is_err());
    let old = f
        .domain
        .recover_shared_runtime(
            &p,
            &f.policy,
            &d.command.command_id,
            ManagedRuntimeRecoverInput {
                command_digest: d.command_digest.clone(),
                observation_id: None,
            },
        )
        .await
        .unwrap();
    assert!(!old.execution_allowed);
    assert!(!old.cleanup_allowed);
    let ack = f
        .domain
        .record_shared_process(
            &p,
            &f.policy,
            &d.command.command_id,
            evidence(&old, "qualified", 200),
        )
        .await
        .unwrap();
    assert!(ack.activity.is_none());
    let cleanup = f
        .domain
        .poll_shared_runtime(&p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    assert!(cleanup.cleanup_allowed);
    assert!(!cleanup.execution_allowed);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_atomic_receipts_revoke_and_cross_protocol_collision() {
    let (f, r, admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_shared_reject() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'fixture';END;$$;CREATE TRIGGER fixture_shared_reject BEFORE INSERT ON hagency_agent_v1.management_shared_process_receipts FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_shared_reject()").await.unwrap();
    assert!(save(&f, &d, "qualified", 200).await.is_err());
    assert!(sql_query("SELECT activity_epoch=0 AND state='issued' AS matched FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1").bind::<Text,_>(&d.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fixture_shared_reject ON hagency_agent_v1.management_shared_process_receipts;DROP FUNCTION hagency_agent_v1.fixture_shared_reject()").await.unwrap();
    save(&f, &d, "qualified", 200).await.unwrap();
    let mut changed = evidence(&d, "qualified", 201);
    changed.receipt.process_digest = Some("c".repeat(64));
    assert!(
        f.domain
            .record_shared_process(&f.owner.p, &f.policy, &d.command.command_id, changed)
            .await
            .is_err()
    );
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_shared_reject() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN IF NEW.permanent_fence THEN RAISE EXCEPTION 'fixture';END IF;RETURN NEW;END;$$;CREATE TRIGGER fixture_shared_reject BEFORE UPDATE ON hagency_agent_v1.management_shared_runtimes FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_shared_reject()").await.unwrap();
    assert!(
        f.domain
            .revoke_managed_agent_request(
                &admin.p,
                &f.policy,
                &r.id,
                RevokeInput {
                    command_id: "failed_revoke".into(),
                    expected_revision: r.revision
                }
            )
            .await
            .is_err()
    );
    assert!(sql_query("SELECT NOT permanent_fence AND activity_epoch=1 AS matched FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1").bind::<Text,_>(&d.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fixture_shared_reject ON hagency_agent_v1.management_shared_runtimes;DROP FUNCTION hagency_agent_v1.fixture_shared_reject()").await.unwrap();
    revoke(&f, &r, &admin).await;
    let fence = poll(&f).await;
    let mut clash = fence_result(&fence, true);
    if let AgentExecutorReceipt::Fence(ref mut receipt) = clash.receipt {
        receipt.ledger_revision = 200;
        receipt.fence_revision = 200;
    }
    assert!(submit(&f, &fence, clash).await.is_err());
    let old = history(&f, &d).await;
    assert_eq!(
        save(&f, &old, "unknown", 201).await.unwrap().state,
        "cleanup_pending"
    );
    assert_eq!(
        save(&f, &old, "stopped", 202).await.unwrap().state,
        "stopped"
    );
    assert!(save(&f, &old, "qualified", 203).await.is_err());
    assert!(
        sql_query("DELETE FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1")
            .bind::<Text, _>(&d.command.command_id)
            .execute(&mut db)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_v11_migration_is_atomic_preserves_original_v1() {
    let (f, _r, _admin, a, _m) = Box::pin(prepared()).await;
    let original = poll_runtime(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    remove_migration(&mut db).await;
    db.batch_execute(
        "CREATE TABLE hagency_agent_v1.management_shared_process_receipts(dummy text)",
    )
    .await
    .unwrap();
    assert!(
        DomainStore::open(&f.url, "example.test", "_hagency_test_")
            .await
            .is_err()
    );
    assert!(sql_query("SELECT version=11 AND to_regclass('hagency_agent_v1.management_shared_runtimes') IS NULL AS matched FROM hagency_agent_v1.management_deployment").get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TABLE hagency_agent_v1.management_shared_process_receipts")
        .await
        .unwrap();
    DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let recovered = poll_runtime(&f).await;
    assert_eq!(
        allocation_digest(&original.command).unwrap(),
        allocation_digest(&recovered.command).unwrap()
    );
    assert_eq!(recovered.command.original().assignment_id, a.id);
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        sql_query("SELECT version=29 AS matched FROM hagency_agent_v1.management_deployment")
            .get_result::<Check>(&mut db)
            .await
            .unwrap()
            .matched
    );
}

#[test]
fn shared_execution_canonical_fixture_is_closed_and_keeps_original_reserve_bytes() {
    let f: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-execution-v1.json"
    ))
    .unwrap();
    let c: SharedRuntimeCommand = serde_json::from_value(f["command"].clone()).unwrap();
    c.validate("example.test").unwrap();
    assert_eq!(allocation_digest(&c).unwrap(), f["commandDigest"]);
    let r: SharedProcessReceipt = serde_json::from_value(f["receipt"].clone()).unwrap();
    r.validate_for(&c).unwrap();
    assert_eq!(allocation_digest(&r).unwrap(), f["receiptDigest"]);
    let v1: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-agent-runtime-v1.json"
    ))
    .unwrap();
    assert_eq!(
        f["command"]["reserveCommand"],
        v1["provisionCommand"]["reserveCommand"]
    );
    let mut unknown = f["command"].clone();
    unknown["allowTools"] = json!(true);
    assert!(serde_json::from_value::<SharedRuntimeCommand>(unknown).is_err());
    let mut bad = c;
    bad.policy = "codex_no_tools_v1".into();
    assert!(bad.validate("example.test").is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_concurrent_poll_finite_issuance_and_changed_membership() {
    let (f, r, _admin, _a, m) = Box::pin(prepared()).await;
    let other = DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default()),
        other.poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
    );
    let left = left.unwrap().unwrap();
    let right = right.unwrap().unwrap();
    assert_eq!(left.command_digest, right.command_digest);
    assert_eq!(left.command.expires_at_ms, right.command.expires_at_ms);
    // Poll order is deliberately nondeterministic. Every original finite ticket
    // remains scoped and valid; later discovery cannot revoke an in-flight upload.
    let fresh = shared_poll(&f).await;
    assert_eq!(fresh.command_digest, left.command_digest);
    assert!(save(&f, &left, "qualified", 200).await.unwrap().activity.is_some());
    assert!(save(&f, &right, "qualified", 200).await.unwrap().activity.is_some());
    let activity = save(&f, &fresh, "qualified", 200)
        .await
        .unwrap()
        .activity
        .unwrap();
    let setup = f
        .domain
        .management_project_setup(&f.outsider.p, &f.policy, &r.project_id)
        .await
        .unwrap()
        .setup
        .unwrap();
    member(
        &mut m.states.lock().unwrap(),
        setup.room_id.as_ref().unwrap(),
        &f.outsider.p.mxid,
        "leave",
    );
    let input = SharedActivityRenewInput {
        authority: activity.authority,
        renewal_id: "renew_removed_member".into(),
    };
    assert!(
        f.domain
            .renew_shared_activity(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .is_err()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT activity_epoch=1 AS matched FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1").bind::<Text,_>(&right.command.command_id).get_result::<Check>(&mut db).await.unwrap().matched);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_renewal_receipt_rollback_retains_original_lease() {
    let (f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    let (d, activity) = qualify(&f).await;
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("CREATE FUNCTION hagency_agent_v1.fixture_renew_reject() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'fixture';END;$$;CREATE TRIGGER fixture_renew_reject BEFORE INSERT ON hagency_agent_v1.management_shared_activity_renewals FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.fixture_renew_reject()").await.unwrap();
    let input = SharedActivityRenewInput {
        authority: activity.authority,
        renewal_id: "rollback_renewal".into(),
    };
    assert!(
        f.domain
            .renew_shared_activity(&f.owner.p, &f.policy, input.clone(), &m.gateway)
            .await
            .is_err()
    );
    assert!(sql_query("SELECT activity_epoch=1 AND activity_until_ms=$2 AND last_renewal_id IS NULL AS matched FROM hagency_agent_v1.management_shared_runtimes WHERE id=$1").bind::<Text,_>(&d.command.command_id).bind::<BigInt,_>(activity.expires_at_ms).get_result::<Check>(&mut db).await.unwrap().matched);
    db.batch_execute("DROP TRIGGER fixture_renew_reject ON hagency_agent_v1.management_shared_activity_renewals;DROP FUNCTION hagency_agent_v1.fixture_renew_reject()").await.unwrap();
    assert_eq!(
        f.domain
            .renew_shared_activity(&f.owner.p, &f.policy, input, &m.gateway)
            .await
            .unwrap()
            .authority
            .epoch,
        2
    );
    assert!(
        sql_query(
            "DELETE FROM hagency_agent_v1.management_shared_activity_renewals WHERE runtime_id=$1"
        )
        .bind::<Text, _>(&d.command.command_id)
        .execute(&mut db)
        .await
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_http_qualify_renew_and_recover_distinguish_execution() {
    let (f, _r, _admin, _a, m) = Box::pin(prepared()).await;
    let s = svc(&f, m.gateway.clone());
    let reply = post(
        &s,
        "shared-execution/runtime/poll",
        &f.owner.token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let d: SharedRuntimeDelivery = serde_json::from_value(reply["delivery"].clone()).unwrap();
    assert!(d.execution_allowed);
    let ack = post(
        &s,
        &format!("shared-execution/runtime/{}/result", d.command.command_id),
        &f.owner.token,
        serde_json::to_value(evidence(&d, "qualified", 200)).unwrap(),
        StatusCode::OK,
    )
    .await;
    let mut ack = ack;
    ack.as_object_mut().unwrap().remove("observedAtMs");
    let ack: SharedRuntimeAcknowledgement = serde_json::from_value(ack).unwrap();
    let lease = ack.activity.unwrap();
    let input = SharedActivityRenewInput {
        authority: lease.authority,
        renewal_id: "http_renewal".into(),
    };
    let renewed = post(
        &s,
        "shared-execution/activity/renew",
        &f.owner.token,
        serde_json::to_value(input.clone()).unwrap(),
        StatusCode::OK,
    )
    .await;
    let lease: SharedActivityLease = serde_json::from_value(renewed["activity"].clone()).unwrap();
    assert_eq!(lease.authority.epoch, 2);
    let replay = post(
        &s,
        "shared-execution/activity/renew",
        &f.owner.token,
        serde_json::to_value(input).unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        replay["activity"]["expiresAtMs"],
        renewed["activity"]["expiresAtMs"]
    );
    let recovered = post(
        &s,
        &format!("shared-execution/runtime/{}/recover", d.command.command_id),
        &f.owner.token,
        json!({"commandDigest":d.command_digest,"observationId":null}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(recovered["delivery"]["executionAllowed"], false);
    assert_eq!(recovered["delivery"]["cleanupAllowed"], false);
    assert_eq!(recovered["delivery"]["recorded"]["state"], "qualified");
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_initial_deadline_is_permanent_and_recovery_retains_late_evidence() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    tokio::time::sleep(std::time::Duration::from_millis(
        (d.command.expires_at_ms - now_ms() + 30).max(1) as u64,
    ))
    .await;
    assert!(save(&f, &d, "qualified", 200).await.is_err());
    let cleanup = shared_poll(&f).await;
    assert_eq!(cleanup.command_digest, d.command_digest);
    assert!(cleanup.cleanup_allowed);
    assert!(!cleanup.execution_allowed);
    let late = save(&f, &cleanup, "qualified", 200).await.unwrap();
    assert!(late.activity.is_none());
    assert_eq!(late.state, "cleanup_pending");
    let old = history(&f, &d).await;
    assert_eq!(old.recorded.as_ref().unwrap().ledger_revision, 200);
    assert!(!old.execution_allowed);
    assert!(!old.cleanup_allowed);
    assert_eq!(
        save(&f, &old, "stopped", 201).await.unwrap().state,
        "stopped"
    );
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_sql_guards_prevent_both_directions_of_legacy_rebinding() {
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = shared_poll(&f).await;
    let mut legacy = serde_json::to_value(&d.command).unwrap();
    legacy["commandId"] = json!("forbidden_metadata_provision");
    legacy["capability"] = json!(crate::management_agent_runtime_types::AGENT_RUNTIME_CAPABILITY);
    legacy["operation"] = json!("agent.runtime.provision");
    legacy["policy"] = json!("codex_no_tools_v1");
    let c: ManagedRuntimeProvisionCommand = serde_json::from_value(legacy).unwrap();
    c.validate("example.test").unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("INSERT INTO hagency_agent_v1.management_agent_runtime_commands(id,assignment_id,operation,created_at_ms,payload,digest,expires_at_ms) VALUES($1,$2,'agent.runtime.provision',$3,$4,$5,$6)").bind::<Text,_>(&c.command_id).bind::<Text,_>(&c.assignment_id).bind::<BigInt,_>(c.issued_at_ms).bind::<Text,_>(serde_json::to_string(&c).unwrap()).bind::<Text,_>(allocation_digest(&c).unwrap()).bind::<BigInt,_>(c.expires_at_ms).execute(&mut db).await.is_err());
    let (f, _r, _admin, _a, _m) = Box::pin(prepared()).await;
    let d = poll_runtime(&f).await;
    let mut shared = serde_json::to_value(d.command.original()).unwrap();
    shared["commandId"] = json!("forbidden_shared_provision");
    shared["capability"] = json!(SHARED_EXECUTION_CAPABILITY);
    shared["operation"] = json!("agent.execution.provision");
    shared["policy"] = json!(SHARED_TEXT_POLICY);
    let c: SharedRuntimeCommand = serde_json::from_value(shared).unwrap();
    c.validate("example.test").unwrap();
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("INSERT INTO hagency_agent_v1.management_shared_runtimes(id,assignment_id,created_at_ms,payload,digest,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&c.command_id).bind::<Text,_>(&c.assignment_id).bind::<BigInt,_>(c.issued_at_ms).bind::<Text,_>(serde_json::to_string(&c).unwrap()).bind::<Text,_>(allocation_digest(&c).unwrap()).bind::<BigInt,_>(c.expires_at_ms).execute(&mut db).await.is_err());
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn shared_runtime_discovery_rotates_three_agents_without_polling_healthy_processes() {
    let (f, first, admin, first_assignment, m) = Box::pin(prepared()).await;
    let (first_runtime, _activity) = qualify(&f).await;
    let (_second, second_assignment) =
        Box::pin(work::another_assignment(&f, &first, &admin, &m, 2)).await;
    // An existing qualified process must not shadow a different approved agent.
    let second_runtime = shared_poll(&f).await;
    assert_eq!(second_runtime.command.assignment_id, second_assignment.id);
    assert_ne!(second_runtime.command_digest, first_runtime.command_digest);
    save(&f, &second_runtime, "qualified", 301).await.unwrap();
    assert!(
        f.domain
            .poll_shared_runtime(&f.owner.p, &f.policy, ExecutorPollInput::default())
            .await
            .unwrap()
            .is_none()
    );

    // Unknown original-process cleanup remains actionable, not an excuse to
    // respawn it. Discovery of another agent must still make bounded progress.
    let original = history(&f, &first_runtime).await;
    save(&f, &original, "unknown", 302).await.unwrap();
    let cleanup = shared_poll(&f).await;
    assert_eq!(cleanup.command.assignment_id, first_assignment.id);
    assert!(cleanup.cleanup_allowed && !cleanup.execution_allowed);
    let (_third, third_assignment) =
        Box::pin(work::another_assignment(&f, &first, &admin, &m, 3)).await;
    let third_runtime = f
        .domain
        .poll_shared_runtime(
            &f.owner.p,
            &f.policy,
            ExecutorPollInput {
                after: Some(cleanup.command.command_id.clone()),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third_runtime.command.assignment_id, third_assignment.id);
    assert!(third_runtime.execution_allowed && !third_runtime.cleanup_allowed);
    let cleanup_again = f
        .domain
        .poll_shared_runtime(
            &f.owner.p,
            &f.policy,
            ExecutorPollInput {
                after: Some(third_runtime.command.command_id.clone()),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleanup_again.command_digest, first_runtime.command_digest);
    assert!(cleanup_again.cleanup_allowed && !cleanup_again.execution_allowed);
    save(&f, &third_runtime, "qualified", 401).await.unwrap();
    save(&f, &cleanup_again, "stopped", 402).await.unwrap();
    assert!(
        f.domain
            .poll_shared_runtime(
                &f.owner.p,
                &f.policy,
                ExecutorPollInput {
                    after: Some(cleanup_again.command.command_id.clone()),
                }
            )
            .await
            .unwrap()
            .is_none()
    );
    let recovered = history(&f, &first_runtime).await;
    assert!(!recovered.execution_allowed && !recovered.cleanup_allowed);
    assert_eq!(recovered.recorded.unwrap().state, "stopped");
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("SELECT count(*)=3 AND count(DISTINCT r.assignment_id)=3 AS matched FROM hagency_agent_v1.management_shared_runtimes r JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.executor_owner_user_id=$1 AND a.executor_device_id=$2")
        .bind::<Text,_>(&f.owner.p.user_id).bind::<Text,_>(f.owner.p.device_id.as_ref().unwrap())
        .get_result::<Check>(&mut db).await.unwrap().matched);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_owner_direct_funding_is_atomic_idempotent_and_owner_only() {
    use crate::management_agent_budget_types::*;
    let (mut f, request, _, _) = Box::pin(prepared_pool(true)).await;
    f.policy=f.policy.with_agent_budget_deltas(true);
    let b=f.domain.agent_budget(&f.owner.p,&f.policy,&request.id).await.unwrap();
    assert!(!b.can_request && b.can_owner_fund && b.can_owner_fund_direct);
    assert!(f.domain.agent_budget(&f.admin.p,&f.policy,&request.id).await.is_err());
    let input=RequestAgentTopUp { approve_immediately: true, owner_funded: true,
        command_id:"pool_topup_request".into(),expected_request_revision:b.request_revision,
        expected_assignment_generation:b.assignment_generation,expected_budget_revision:b.budget_revision,
        expected_budget_digest:b.budget_digest,add_tokens:2000000,purpose:"Continue the same pool Agent".into(),
    };
    assert!(f.domain.request_agent_topup(&f.outsider.p,&f.policy,&request.id,input.clone()).await.is_err());
    assert!(f.domain.request_agent_topup(&f.admin.p,&f.policy,&request.id,input.clone()).await.is_err());
    let mut delegated=input.clone();delegated.owner_funded=false;
    assert!(f.domain.request_agent_topup(&f.outsider.p,&f.policy,&request.id,delegated).await.is_err());
    let mut stale=input.clone(); stale.expected_budget_revision+=1;
    assert!(f.domain.request_agent_topup(&f.owner.p,&f.policy,&request.id,stale).await.is_err());
    let result=f.domain.request_agent_topup(&f.owner.p,&f.policy,&request.id,input.clone()).await.unwrap();
    assert_eq!(result.receipt.applied_revision,1);
    let top=result.top_up;
    assert_eq!((top.state.as_str(),top.revision),("approved",2));
    assert!(top.owner_funded && !top.can_decide);
    let replay=f.domain.request_agent_topup(&f.owner.p,&f.policy,&request.id,input.clone()).await.unwrap();
    assert_eq!(replay.top_up.id,top.id);
    assert_eq!(replay.top_up.executor_command_id,top.executor_command_id);
    let mut changed=input.clone(); changed.add_tokens+=1;
    assert!(f.domain.request_agent_topup(&f.owner.p,&f.policy,&request.id,changed).await.is_err());
    let d=f.domain.poll_agent_budget(&f.owner.p,&f.policy,ExecutorPollInput::default()).await.unwrap().unwrap();
    assert!(d.application_allowed);
    let c=&d.command;
    assert!(c.owner_funded && c.version==2);
    assert!(c.add_tokens>c.reserve_command.pool_grant.as_ref().unwrap().allocated_tokens);
    assert_eq!(c.approving_administrator_mxid,f.owner.p.mxid);
    assert_eq!(c.reserve_command.requested_tokens,100);
    let mut forged=c.clone();forged.approving_administrator_mxid=f.outsider.p.mxid.clone();
    assert!(forged.validate("test.local").is_err());
    let receipt=AgentBudgetReceipt {version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),top_up_id:c.top_up_id.clone(),agent_reservation_id:c.reserve_receipt.agent_reservation_id.clone().unwrap(),assignment_id:c.assignment_id.clone(),original_reserve_receipt_digest:c.reserve_receipt_digest.clone(),state:"applied".into(),expected_budget_revision:c.expected_budget_revision,previous_budget_digest:c.expected_budget_digest.clone(),budget_revision:2,budget_digest:next_budget_digest(c,200).unwrap(),previous_ceiling_tokens:100,applied_delta_tokens:2000000,effective_ceiling_tokens:2000100,ledger_revision:200,period_key:c.reserve_command.period_key.clone(),reason:None};
    f.domain.record_agent_budget(&f.owner.p,&c.command_id,AgentBudgetResult {command_digest:d.command_digest,token:d.token,receipt}).await.unwrap();
    let b=f.domain.agent_budget(&f.outsider.p,&f.policy,&request.id).await.unwrap();
    assert_eq!((b.original_ceiling_tokens,b.effective_ceiling_tokens,b.budget_revision),(100,2000100,2));
    let replay=f.domain.request_agent_topup(&f.owner.p,&f.policy,&request.id,input).await.unwrap();
    assert_eq!(replay.top_up.native_state,"applied");
    assert_eq!(replay.top_up.executor_command_id,top.executor_command_id);
    assert_eq!(f.domain.pool_agent_request(&f.outsider.p,&request.id).await.unwrap().requested_tokens,100);
}

