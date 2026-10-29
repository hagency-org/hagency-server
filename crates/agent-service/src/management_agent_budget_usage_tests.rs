use super::*;
use crate::management_agent_budget_types::*;
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_overrun_growth_uses_max_existing_liability_and_old_v2_stays_exhausted() {
    let (mut f, r, admin, _, m) = Box::pin(prepared()).await;
    f.policy = f
        .policy
        .with_agent_budget_deltas(true)
        .with_shared_successors(true);
    let (d, a) = qualify(&f).await;
    let w = started_on_database_clock(&f, &r, &m, &a).await;
    let t = ticket(&f, &d, &w).await;
    let mut out = outcome(&d, &w, &t, 201, "completed");
    out.receipt.known_consumed_tokens = 117;
    out.receipt.retained_tokens = Some(117);
    f.domain
        .record_shared_work_result(&f.owner.p, &f.policy, &w.id, out)
        .await
        .unwrap();
    let b = f
        .domain
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap();
    assert_eq!(
        (
            b.original_ceiling_tokens,
            b.effective_ceiling_tokens,
            b.known_consumed_tokens,
            b.conservative_commitment_tokens
        ),
        (100, 100, 117, 117)
    );
    let input = RequestAgentTopUp { approve_immediately: false, owner_funded: false,
        command_id: "overrun_delta".into(),
        expected_request_revision: r.revision,
        expected_assignment_generation: 1,
        expected_budget_revision: 1,
        expected_budget_digest: b.budget_digest,
        add_tokens: 25,
        purpose: "More approved work".into(),
    };
    let t = f
        .domain
        .request_agent_topup(&f.outsider.p, &f.policy, &r.id, input)
        .await
        .unwrap();
    f.domain
        .decide_agent_topup(
            &admin.p,
            &f.policy,
            &t.top_up.id,
            DecideAgentTopUp {
                command_id: "overrun_decide".into(),
                expected_revision: 1,
                expected_authority_revision: r.authority_revision,
                decision: Decision::Approve,
            },
        )
        .await
        .unwrap();
    let before = f
        .domain
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap();
    assert_eq!(before.effective_ceiling_tokens, 100);
    assert_eq!(before.conservative_commitment_tokens, 125);
    assert_eq!(before.native_remaining_tokens, None);
    let delta = f
        .domain
        .poll_agent_budget(&f.owner.p, &f.policy, ExecutorPollInput::default())
        .await
        .unwrap()
        .unwrap();
    let c = &delta.command;
    let receipt = AgentBudgetReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: delta.command_digest.clone(),
        top_up_id: c.top_up_id.clone(),
        agent_reservation_id: c.reserve_receipt.agent_reservation_id.clone().unwrap(),
        assignment_id: c.assignment_id.clone(),
        original_reserve_receipt_digest: c.reserve_receipt_digest.clone(),
        state: "applied".into(),
        expected_budget_revision: 1,
        previous_budget_digest: c.expected_budget_digest.clone(),
        budget_revision: 2,
        budget_digest: next_budget_digest(c, 202).unwrap(),
        previous_ceiling_tokens: 100,
        applied_delta_tokens: 25,
        effective_ceiling_tokens: 125,
        ledger_revision: 202,
        period_key: c.reserve_command.period_key.clone(),
        reason: None,
    };
    f.domain
        .record_agent_budget(
            &f.owner.p,
            &c.command_id,
            AgentBudgetResult {
                command_digest: delta.command_digest.clone(),
                token: delta.token,
                receipt,
            },
        )
        .await
        .unwrap();
    let b = f
        .domain
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap();
    assert_eq!(
        (
            b.effective_ceiling_tokens,
            b.known_consumed_tokens,
            b.conservative_commitment_tokens
        ),
        (125, 117, 125)
    );
    assert_eq!(
        b.conservative_commitment_tokens - before.known_consumed_tokens,
        8
    );
    assert!(!b.execution_supported);
    let stopped = attempts::stop(&f, &d, 203).await;
    let mut start = attempts::start(&d, &stopped);
    start.budget_observation.known_consumed_tokens = 117;
    start.budget_observation.retained_tokens = 117;
    start.budget_observation.remaining_tokens = 0;
    start.budget_observation.usage_complete = true;
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, start.clone(), &m.gateway)
            .await
            .is_err()
    );
    start.budget_observation.allocated_tokens = 125;
    start.budget_observation.remaining_tokens = 8;
    assert!(
        f.domain
            .start_shared_attempt(&f.owner.p, &f.policy, start, &m.gateway)
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "dedicated PostgreSQL"]
async fn agent_budget_pending_sibling_reserve_remains_project_commitment() {
    let (mut f, r, admin, _, m) = Box::pin(prepared()).await;
    f.policy = f.policy.with_agent_budget_deltas(true);
    let ready = f
        .domain
        .management_agent_readiness(&f.outsider.p, &f.policy, &r.project_id)
        .await
        .unwrap()
        .request_context
        .unwrap();
    let sibling = f
        .domain
        .request_managed_agent(
            &f.outsider.p,
            &f.policy,
            &r.project_id,
            RequestManagedAgent {
                command_id: "pending_sibling".into(),
                expected_setup_revision: ready.setup_revision,
                expected_setup_binding_revision: ready.setup_binding_revision,
                expected_authority_revision: ready.authority_revision,
                expected_allocation_revision: ready.allocation_revision,
                definition: r.definition.clone(),
                requested_tokens: ready.requested_token_limit,
            },
            &m.gateway,
        )
        .await
        .unwrap();
    f.domain
        .decide_managed_agent(
            &admin.p,
            &f.policy,
            &sibling.id,
            DecideManagedAgent {
                command_id: "approve_pending_sibling".into(),
                expected_revision: sibling.revision,
                expected_authority_revision: sibling.authority_revision,
                decision: Decision::Approve,
            },
            &m.gateway,
        )
        .await
        .unwrap();
    let b = f
        .domain
        .agent_budget(&f.outsider.p, &f.policy, &r.id)
        .await
        .unwrap();
    let error = f
        .domain
        .request_agent_topup(
            &f.outsider.p,
            &f.policy,
            &r.id,
            RequestAgentTopUp { approve_immediately: false, owner_funded: false,
                command_id: "over_pending_sibling".into(),
                expected_request_revision: r.revision,
                expected_assignment_generation: 1,
                expected_budget_revision: b.budget_revision,
                expected_budget_digest: b.budget_digest,
                add_tokens: 1,
                purpose: "Cannot assume sibling unissued".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "agent_budget_exceeds_project");
}

// Match the production database clock used by runtime issuance and event admission.
// A host timestamp can precede just-issued DB time or be in its future.
async fn started_on_database_clock(
    f: &Fixture,
    r: &ManagedAgentRequest,
    m: &MatrixFixture,
    a: &SharedActivityLease,
) -> SharedWorkItem {
    #[derive(QueryableByName)]
    struct Time {
        #[diesel(sql_type=BigInt)]
        now: i64,
    }
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    let received = sql_query("SELECT (extract(epoch from clock_timestamp())*1000)::bigint AS now")
        .get_result::<Time>(&mut db)
        .await
        .unwrap()
        .now;
    let e = message(f, r, "budget_overrun").await;
    f.domain
        .route_shared_event(&f.policy, &e, received, &m.gateway)
        .await
        .unwrap();
    let w = f
        .domain
        .poll_shared_work(&f.owner.p, &f.policy, a.authority.clone(), &m.gateway)
        .await
        .unwrap()
        .expect("DB-clock fixture event must be admitted on the qualified original runtime");
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input(a, &w), &m.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input(a, &w), &m.gateway)
            .await
            .unwrap()
            .newly_started
    );
    w
}
