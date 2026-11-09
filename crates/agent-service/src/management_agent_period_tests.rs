use super::*;
use crate::recurring_quota::{Cadence, Calendar, RenewalMode};

async fn time(f: &Fixture, now: i64) {
    f.domain.set_management_test_clock(now).await.unwrap();
}
async fn recurring(
    mode: RenewalMode,
) -> (
    Fixture,
    Admission,
    ResourceGrantView,
    PoolAgentRequest,
    PendingManagedAssignment,
    i64,
) {
    recurring_with_limits(mode, None).await
}
async fn recurring_with_limits(mode: RenewalMode, cap: Option<i64>) -> (
    Fixture, Admission, ResourceGrantView, PoolAgentRequest, PendingManagedAssignment, i64,
) {
    recurring_with_managers(mode, cap, false).await
}
async fn recurring_with_managers(mode: RenewalMode, cap: Option<i64>, large: bool) -> (
    Fixture, Admission, ResourceGrantView, PoolAgentRequest, PendingManagedAssignment, i64,
) {
    let now = crate::api::now_ms();
    let old = now - 4 * 86400000;
    let calendar = Calendar {
        cadence: Cadence::Daily,
        time_zone: "America/Los_Angeles".into(),
    };
    recurring_on_calendar(mode, cap, large, calendar, old, now).await
}
async fn recurring_on_calendar(
    mode: RenewalMode,
    cap: Option<i64>,
    large: bool,
    calendar: Calendar,
    old: i64,
    now: i64,
) -> (
    Fixture, Admission, ResourceGrantView, PoolAgentRequest, PendingManagedAssignment, i64,
) {
    let f = succession_fixture().await;
    let a = admitted(&f).await;
    time(&f, old).await;
    let period = calendar.period_at(old as u64).unwrap();
    let mut g = grant_input(&f, &a, &f.outsider.p).await;
    g.receipt.version = if cap.is_some() { 5 } else { 4 };
    g.receipt.per_request_tokens = cap;
    g.receipt.admission_request_id = Some(a.request.id.clone());
    g.receipt.calendar = Some(calendar);
    g.receipt.period_key = period.key;
    g.receipt.period_start_ms = period.start_ms as i64;
    g.receipt.period_end_ms = period.end_ms as i64;
    g.receipt.delegation_expires_at_ms = period.end_ms as i64;
    if large {
        g.receipt.eligible_managers.extend((0..45).map(|n| format!("@manager_{n:02}_{}:example.test", "x".repeat(180))));
        g.receipt.eligible_managers.sort();
    }
    let g = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, g)
        .await
        .unwrap();
    let mut input = input(&a, "calendar_agent");
    input.renewal_mode = Some(mode);
    let r = f
        .domain
        .request_pool_agent(&f.outsider.p, &f.policy, &a.contribution.id, input)
        .await
        .unwrap();
    assert_eq!(r.calendar, g.receipt.calendar);
    assert_eq!(r.renewal_mode, Some(mode));
    let r = decide(&f, &r, true).await.unwrap();
    let assignment = reserve(&f, &r.id, 100).await;
    (f, a, g, r, assignment, now)
}
async fn current_grant(
    f: &Fixture,
    a: &Admission,
    old: &ResourceGrantView,
    now: i64,
) -> ResourceGrantView {
    time(f, now).await;
    let mut receipt = old.receipt.clone();
    let p = receipt
        .calendar
        .as_ref()
        .unwrap()
        .period_at(now as u64)
        .unwrap();
    receipt.grant_id = format!("mgs_{}", &secret_token()[..32]);
    receipt.grant_revision += 1;
    receipt.ledger_revision = 101;
    receipt.period_key = p.key;
    receipt.period_start_ms = p.start_ms as i64;
    receipt.period_end_ms = p.end_ms as i64;
    receipt.delegation_expires_at_ms = p.end_ms as i64;
    receipt.succession = Some(GrantSuccession {
        kind: "renewal".into(),
        predecessor_grant_id: old.id.clone(),
        predecessor_receipt_digest: old.receipt_digest.clone(),
        carried_children: 0,
        carried_retained_tokens: 0,
    });
    attempt(f, a, old, receipt).await.unwrap()
}
async fn acknowledge_period(f: &Fixture, d: &AgentExecutorDelivery, ledger: i64) {
    let AgentExecutorCommand::RenewPeriod(c) = &d.command else {
        panic!("expected period renewal")
    };
    assert!(d.execution_allowed);
    let receipt = ManagedAgentPeriodReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: d.command_digest.clone(),
        request_id: c.reserve_command.request_id.clone(),
        agent_id: c.reserve_command.proposed_agent_id.clone(),
        agent_reservation_id: c.agent_reservation_id.clone(),
        period: c.period.clone(),
        allocated_tokens: c.reserve_command.requested_tokens,
        ledger_revision: ledger,
        state: "authorized".into(),
        reason: None,
    };
    let result = AgentExecutorResultInput {
        command_digest: d.command_digest.clone(),
        token: d.token.clone(),
        receipt: AgentExecutorReceipt::RenewPeriod(receipt),
    };
    let ack = f
        .domain
        .record_managed_agent_executor_result(&f.owner.p, &f.policy, &c.command_id, result.clone())
        .await
        .unwrap();
    assert!(ack.assignment.is_none());
    let replay = f
        .domain
        .record_managed_agent_executor_result(&f.owner.p, &f.policy, &c.command_id, result)
        .await
        .unwrap();
    assert_eq!(ack.receipt_digest, replay.receipt_digest);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_renewal_policy_manual_pool_publication_renews_existing_agents_without_a_second_decision() {
    for mode in [RenewalMode::Owner, RenewalMode::Automatic] {
        let (f, a, g, r, _, now) = recurring(RenewalMode::Owner).await;
        time(&f, now).await;
        assert!(f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput {after:None}).await.unwrap().is_none());
        // The resource owner publishes the next pool period with its chosen
        // rule. Existing immutable Agent terms and IDs are not rewritten.
        let mut selected = g.clone();
        selected.receipt.version = 6;
        selected.receipt.renewal_mode = Some(mode);
        selected.receipt.per_request_tokens = Some(g.receipt.allocated_tokens);
        current_grant(&f, &a, &selected, now).await;
        let delivery = f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput {after:None}).await.unwrap().unwrap();
        let AgentExecutorCommand::RenewPeriod(c) = &delivery.command else {panic!("expected period renewal")};
        assert_eq!(c.renewal_mode, mode);
        assert_eq!(c.reserve_command.request_id, r.id);
        c.validate("example.test").unwrap();
        acknowledge_period(&f, &delivery, 200).await;
        let updated=f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap();
        assert_eq!(updated.renewal_mode,Some(mode));
        assert_eq!(updated.current_period.unwrap().state,"available");
    }
}

/// The same persistent approval/receipt path must respect each owner calendar,
/// including the exact boundary and the short/long periods around DST.
async fn calendar_boundary_renewal(cadence: Cadence, boundary: &str, end: &str) {
    let ms = |value: &str| {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis()
    };
    let boundary = ms(boundary);
    let end = ms(end);
    let calendar = Calendar {
        cadence,
        time_zone: "America/Los_Angeles".into(),
    };
    let expected = calendar.period_at(boundary as u64).unwrap();
    assert_eq!(expected.start_ms, boundary as u64);
    assert_eq!(expected.end_ms, end as u64);
    for mode in [RenewalMode::Owner, RenewalMode::Automatic] {
        let (f, a, g, r, assignment, _) = recurring_on_calendar(
            mode,
            Some(100),
            false,
            calendar.clone(),
            boundary - 60_000,
            boundary,
        )
        .await;
        assert_eq!(g.receipt.period_end_ms, boundary);
        time(&f, boundary - 1).await;
        let before = f
            .domain
            .pool_agent_request(&f.owner.p, &r.id)
            .await
            .unwrap();
        assert_eq!(before.current_period.unwrap().state, "available");
        assert!(!before.can_renew);
        assert!(
            f.domain
                .poll_managed_agent_executor(
                    &f.owner.p,
                    &f.policy,
                    ExecutorPollInput { after: None },
                )
                .await
                .unwrap()
                .is_none()
        );

        time(&f, boundary).await;
        let paused = f
            .domain
            .pool_agent_request(&f.owner.p, &r.id)
            .await
            .unwrap();
        let paused_period = paused.current_period.unwrap();
        assert_eq!(paused_period.key, expected.key);
        assert_eq!(paused_period.state, "awaiting_capacity");
        assert_eq!(paused_period.remaining_tokens, Some(0));
        assert_eq!(
            paused.assignment_id.as_deref(),
            Some(assignment.id.as_str())
        );
        assert!(!paused.can_renew);

        let current = current_grant(&f, &a, &g, boundary).await;
        assert_eq!(current.receipt.period_end_ms, end);
        if mode == RenewalMode::Owner {
            assert!(
                f.domain
                    .poll_managed_agent_executor(
                        &f.owner.p,
                        &f.policy,
                        ExecutorPollInput { after: None },
                    )
                    .await
                    .unwrap()
                    .is_none()
            );
            let input = RenewPoolAgent {
                command_id: "calendar_boundary_owner_renewal".into(),
                expected_revision: r.revision,
                expected_period_key: expected.key.clone(),
            };
            assert_eq!(
                code(
                    f.domain
                        .renew_pool_agent(&f.outsider.p, &f.policy, &r.id, input.clone(),)
                        .await
                ),
                "pool_resource_owner_required"
            );
            f.domain
                .renew_pool_agent(&f.owner.p, &f.policy, &r.id, input)
                .await
                .unwrap();
        }
        let delivery = f
            .domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: None })
            .await
            .unwrap()
            .unwrap();
        let AgentExecutorCommand::RenewPeriod(command) = &delivery.command else {
            panic!("expected a calendar period renewal");
        };
        assert_eq!(command.period, expected);
        assert_eq!(command.reserve_command.requested_tokens, 100);
        assert_eq!(command.assignment_id, assignment.id);
        assert_eq!(
            command.agent_reservation_id,
            assignment.agent_reservation_id
        );
        acknowledge_period(&f, &delivery, 102).await;

        // A second poll and the replayed native receipt cannot add allowance.
        let funded = f
            .domain
            .pool_agent_request(&f.owner.p, &r.id)
            .await
            .unwrap();
        let funded_period = funded.current_period.unwrap();
        assert_eq!(funded_period.state, "available");
        assert_eq!(funded_period.remaining_tokens, Some(100));
        assert_eq!(funded_period.next_reset_ms, end as u64);
        assert_eq!(funded.agent_id, r.agent_id);
        assert_eq!(funded.room_id, r.room_id);
        assert_eq!(funded.assignment_id, Some(assignment.id));
        assert!(!funded.can_renew);
        assert!(
            f.domain
                .poll_managed_agent_executor(
                    &f.owner.p,
                    &f.policy,
                    ExecutorPollInput { after: None },
                )
                .await
                .unwrap()
                .is_none()
        );
        let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
        assert_eq!(sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_agent_executor_commands WHERE operation='agent.period.renew'")
            .get_result::<Count>(&mut db).await.unwrap().count, 1);
    }
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn calendar_boundary_daily_spring_renews_exactly_23_hours_in_both_modes() {
    calendar_boundary_renewal(
        Cadence::Daily,
        "2026-03-08T08:00:00Z",
        "2026-03-09T07:00:00Z",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn calendar_boundary_daily_autumn_renews_exactly_25_hours_in_both_modes() {
    calendar_boundary_renewal(
        Cadence::Daily,
        "2026-11-01T07:00:00Z",
        "2026-11-02T08:00:00Z",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn calendar_boundary_weekly_renews_monday_through_dst_in_both_modes() {
    calendar_boundary_renewal(
        Cadence::Weekly,
        "2026-10-26T07:00:00Z",
        "2026-11-02T08:00:00Z",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn calendar_boundary_monthly_renews_owner_midnight_through_dst_in_both_modes() {
    calendar_boundary_renewal(
        Cadence::Monthly,
        "2026-11-01T07:00:00Z",
        "2026-12-01T08:00:00Z",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn automatic_calendar_renewal_skips_offline_days_and_preserves_identity() {
    automatic_renewal_with_limit(None).await;
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn split_pool_limits_calendar_renewal_preserves_identity_and_does_not_double_fund() {
    automatic_renewal_with_limit(Some(100000)).await;
}
async fn automatic_renewal_with_limit(cap: Option<i64>) {
    let (f, a, g, r, assignment, now) = recurring_with_limits(RenewalMode::Automatic, cap).await;
    time(&f, now).await;
    let paused = f
        .domain
        .pool_agent_request(&f.owner.p, &r.id)
        .await
        .unwrap();
    assert_eq!(paused.current_period.unwrap().state, "awaiting_capacity");
    assert_eq!(
        paused.assignment_id.as_deref(),
        Some(assignment.id.as_str())
    );
    let current = current_grant(&f, &a, &g, now).await;
    let d = f
        .domain
        .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: None })
        .await
        .unwrap()
        .unwrap();
    let AgentExecutorCommand::RenewPeriod(c) = &d.command else {
        panic!("expected renew")
    };
    assert_eq!(c.period.key, current.receipt.period_key);
    assert!(c.period.start_ms > g.receipt.period_end_ms as u64);
    assert_eq!(c.assignment_id, assignment.id);
    assert_eq!(c.agent_reservation_id, assignment.agent_reservation_id);
    acknowledge_period(&f, &d, 102).await;
    let view = f
        .domain
        .pool_agent_request(&f.owner.p, &r.id)
        .await
        .unwrap();
    assert_eq!(view.agent_id, r.agent_id);
    assert_eq!(view.room_id, r.room_id);
    assert_eq!(view.assignment_id, Some(assignment.id));
    let period = view.current_period.unwrap();
    assert_eq!(period.state, "available");
    assert_eq!(period.remaining_tokens, Some(r.requested_tokens));
    assert!(!view.can_renew);
    assert!(
        f.domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: None })
            .await
            .unwrap()
            .is_none()
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert_eq!(sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_agent_executor_commands WHERE operation='agent.period.renew'").get_result::<Count>(&mut db).await.unwrap().count,1);
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn owner_renewal_requires_owner_and_current_period_and_never_refills_twice() {
    let (f, a, g, r, assignment, now) = recurring(RenewalMode::Owner).await;
    let current = current_grant(&f, &a, &g, now).await;
    let view = f
        .domain
        .pool_agent_request(&f.owner.p, &r.id)
        .await
        .unwrap();
    assert_eq!(
        view.current_period.as_ref().unwrap().state,
        "awaiting_owner"
    );
    assert!(view.can_renew);
    assert!(
        !f.domain
            .pool_agent_request(&f.outsider.p, &r.id)
            .await
            .unwrap()
            .can_renew
    );
    assert!(
        f.domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: None })
            .await
            .unwrap()
            .is_none()
    );
    let input = RenewPoolAgent {
        command_id: "renew_once".into(),
        expected_revision: r.revision,
        expected_period_key: current.receipt.period_key.clone(),
    };
    assert_eq!(
        code(
            f.domain
                .renew_pool_agent(&f.outsider.p, &f.policy, &r.id, input.clone())
                .await
        ),
        "pool_resource_owner_required"
    );
    let mut stale = input.clone();
    stale.expected_period_key = g.receipt.period_key;
    assert_eq!(
        code(
            f.domain
                .renew_pool_agent(&f.owner.p, &f.policy, &r.id, stale)
                .await
        ),
        "agent_period_changed"
    );
    let pending = f
        .domain
        .renew_pool_agent(&f.owner.p, &f.policy, &r.id, input.clone())
        .await
        .unwrap();
    assert_eq!(pending.current_period.unwrap().state, "awaiting_executor");
    f.domain
        .renew_pool_agent(&f.owner.p, &f.policy, &r.id, input.clone())
        .await
        .unwrap();
    // A whitelist/capacity amendment must not strand the already queued,
    // immutable per-period command or create a second credit command.
    let mut amended=current.receipt.clone();
    amended.grant_id=format!("mgs_{}",&secret_token()[..32]);amended.grant_revision+=1;amended.ledger_revision=102;
    amended.succession=Some(GrantSuccession{kind:"amendment".into(),predecessor_grant_id:current.id.clone(),predecessor_receipt_digest:current.receipt_digest.clone(),carried_children:0,carried_retained_tokens:0});
    attempt(&f,&a,&current,amended).await.unwrap();
    let d = f
        .domain
        .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: None })
        .await
        .unwrap()
        .unwrap();
    acknowledge_period(&f, &d, 103).await;
    let mut another = input.clone();
    another.command_id = "renew_same_period_again".into();
    f.domain
        .renew_pool_agent(&f.owner.p, &f.policy, &r.id, another)
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: None })
            .await
            .unwrap()
            .is_none()
    );
    let after = f
        .domain
        .managed_agent_assignment(&f.owner.p, &f.policy, &r.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.id, assignment.id);
    assert_eq!(after.state, "pending_native");
    let budget = f
        .domain
        .agent_budget(&f.owner.p, &f.policy, &r.id)
        .await
        .unwrap();
    assert!(!budget.can_owner_fund);
    assert!(!budget.can_request);
    assert_eq!(
        budget.blocking_reason.as_deref(),
        Some("agent_period_allowance_fixed")
    );
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn recurring_terms_and_same_period_early_renewal_are_immutable() {
    let (f, _a, g, r, _assignment, _now) = recurring(RenewalMode::Owner).await;
    let input = RenewPoolAgent {
        command_id: "early_renew".into(),
        expected_revision: r.revision,
        expected_period_key: g.receipt.period_key,
    };
    assert_eq!(
        code(
            f.domain
                .renew_pool_agent(&f.owner.p, &f.policy, &r.id, input)
                .await
        ),
        "agent_period_already_authorized"
    );
    let mut db = AsyncPgConnection::establish(&f.url).await.unwrap();
    assert!(sql_query("UPDATE hagency_agent_v1.management_agent_recurring_terms SET renewal_mode='automatic' WHERE request_id=$1").bind::<Text,_>(&r.id).execute(&mut db).await.is_err());
    db.batch_execute(include_str!("management_shared_claude_down.sql")).await.unwrap();
    db.batch_execute(include_str!("management_shared_context_v2_down.sql")).await.unwrap();
    db.batch_execute(include_str!("management_publication_first_down.sql")).await.unwrap();
    assert!(db.batch_execute(include_str!("management_agent_periods_down.sql"))
        .await.unwrap_err().to_string().contains("agent_period_downgrade_has_authority"));
}

async fn acknowledge_fence(f: &Fixture, ledger: i64) {
    time(f, crate::api::now_ms() + 1000).await;
    let d = f
        .domain
        .poll_managed_agent_executor(&f.owner.p, &f.policy, ExecutorPollInput { after: None })
        .await
        .unwrap()
        .unwrap();
    let AgentExecutorCommand::Fence(c) = &d.command else {
        panic!("expected fence")
    };
    let assignment = f
        .domain
        .managed_agent_assignment(&f.owner.p, &f.policy, &c.reserve_command.request_id)
        .await
        .unwrap()
        .unwrap();
    let r = ManagedAgentFenceReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: d.command_digest.clone(),
        reserve_command_digest: c.reserve_command_digest.clone(),
        request_id: c.reserve_command.request_id.clone(),
        agent_id: c.reserve_command.proposed_agent_id.clone(),
        agent_reservation_id: Some(assignment.agent_reservation_id),
        state: "fenced".into(),
        fence_revision: ledger,
        ledger_revision: ledger,
        known_consumed_tokens: 0,
        retained_tokens: Some(c.reserve_command.requested_tokens),
        usage_complete: false,
        process_state: "stopped".into(),
    };
    f.domain
        .record_managed_agent_executor_result(
            &f.owner.p,
            &f.policy,
            &c.command_id,
            AgentExecutorResultInput {
                command_digest: d.command_digest,
                token: d.token,
                receipt: AgentExecutorReceipt::Fence(r),
            },
        )
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn reallocation_keeps_agent_room_and_fence_history_and_cannot_refill_current_period() {
    let (f, a, g, r, old_assignment, now) = recurring(RenewalMode::Owner).await;
    let current = current_grant(&f, &a, &g, now).await;
    let mut room_db = AsyncPgConnection::establish(&f.url).await.unwrap();
    room_db.batch_execute("INSERT INTO hagency_agent_v1.projects(id,space_id,creation_policy) VALUES('recovery_space','!recovery_space:example.test','{}')").await.unwrap();
    for (name,state,reason) in [("agent_talk","left",Some("allocation_ended")),("kicked","left",Some("membership_removed")),("legacy_left","left",None),("still_joined","active",None)] {
        let room=format!("!{name}:example.test");
        sql_query("INSERT INTO hagency_agent_v1.rooms(room_id,project_id,creation_policy) VALUES($1,'recovery_space','{}')").bind::<Text,_>(&room).execute(&mut room_db).await.unwrap();
        // The old key is already close to the API limit; restored keys must not
        // recursively embed prior generation IDs and become unusable.
        sql_query("INSERT INTO hagency_agent_v1.shared_agent_rooms(id,assignment_id,agent_id,project_id,room_id,state,departure_reason,restoration_requires_membership) VALUES($1,$2,$3,'recovery_space',$4,$5,$6,$7)").bind::<Text,_>(format!("{name}_{}","x".repeat(105))).bind::<Text,_>(&old_assignment.id).bind::<Text,_>(&r.agent_id).bind::<Text,_>(&room).bind::<Text,_>(state).bind::<Nullable<Text>,_>(reason).bind::<Bool,_>(name=="agent_talk").execute(&mut room_db).await.unwrap();
    }
    let stopped = f
        .domain
        .stop_pool_agent(
            &f.owner.p,
            &r.id,
            RevokeInput {
                command_id: "stop_old_generation".into(),
                expected_revision: r.revision,
            },
        )
        .await
        .unwrap();
    assert!(!stopped.can_reallocate);
    acknowledge_fence(&f, 102).await;
    let view = f
        .domain
        .pool_agent_request(&f.owner.p, &r.id)
        .await
        .unwrap();
    assert!(view.can_reallocate);
    assert_eq!(
        view.current_grant_revision,
        Some(current.receipt.grant_revision)
    );
    let input = ReallocatePoolAgent {
        command_id: "recover_old_identity".into(),
        expected_revision: stopped.revision,
        expected_grant_revision: current.receipt.grant_revision,
        renewal_mode: RenewalMode::Automatic,
    };
    assert_eq!(
        code(
            f.domain
                .reallocate_pool_agent(&f.outsider.p, &f.policy, &r.id, input.clone())
                .await
        ),
        "pool_resource_owner_required"
    );
    let recovered = f
        .domain
        .reallocate_pool_agent(&f.owner.p, &f.policy, &r.id, input.clone())
        .await
        .unwrap();
    let replay = f
        .domain
        .reallocate_pool_agent(&f.owner.p, &f.policy, &r.id, input)
        .await
        .unwrap();
    assert_eq!(recovered.id, replay.id);
    assert_ne!(recovered.id, r.id);
    assert_eq!(recovered.agent_id, r.agent_id);
    assert_eq!(recovered.room_id, r.room_id);
    assert_eq!(recovered.rooms_requiring_reinvite,2);
    let d = reserve_delivery(&f, &recovered.id).await;
    assert!(d.execution_allowed);
    let AgentExecutorCommand::Reserve(c) = &d.command else {
        unreachable!()
    };
    assert_eq!(c.allocation_generation, Some(2));
    let receipt = ManagedAgentReservationReceipt {
        version: 1,
        command_id: c.command_id.clone(),
        command_digest: d.command_digest.clone(),
        request_id: c.request_id.clone(),
        agent_id: c.proposed_agent_id.clone(),
        allocation_id: c.allocation_id.clone(),
        agent_reservation_id: Some(format!("child_{}", c.request_id)),
        definition_digest: c.definition_digest.clone(),
        state: "reserved".into(),
        budget_revision: 1,
        ledger_revision: 103,
        allocated_tokens: c.requested_tokens,
        retained_tokens: c.requested_tokens,
        period_key: c.period_key.clone(),
        reason: None,
    };
    let ack = f
        .domain
        .record_managed_agent_executor_result(
            &f.owner.p,
            &f.policy,
            &c.command_id,
            AgentExecutorResultInput {
                command_digest: d.command_digest.clone(),
                token: d.token,
                receipt: AgentExecutorReceipt::Reserve(receipt),
            },
        )
        .await
        .unwrap();
    let assignment = ack.assignment.unwrap();
    assert_ne!(assignment.id, old_assignment.id);
    assert_eq!(sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.shared_agent_rooms WHERE assignment_id=$1 AND room_id='!agent_talk:example.test' AND state='joining'").bind::<Text,_>(&assignment.id).get_result::<Count>(&mut room_db).await.unwrap().count,1);
    #[derive(QueryableByName)]
    struct RestoredRoom { #[diesel(sql_type=Text)] id:String, #[diesel(sql_type=Text)] room_id:String, #[diesel(sql_type=Bool)] restoration_requires_membership:bool }
    let restored=sql_query("SELECT id,room_id,restoration_requires_membership FROM hagency_agent_v1.shared_agent_rooms WHERE assignment_id=$1 ORDER BY room_id").bind::<Text,_>(&assignment.id).load::<RestoredRoom>(&mut room_db).await.unwrap();
    assert_eq!(restored.len(),2,"explicitly kicked and unknown legacy rooms require a new invitation");
    for room in &restored {crate::key(&room.id).unwrap();assert!(room.id.len()<64);}
    assert!(!restored.iter().find(|r|r.room_id=="!agent_talk:example.test").unwrap().restoration_requires_membership);
    assert!(restored.iter().find(|r|r.room_id=="!still_joined:example.test").unwrap().restoration_requires_membership);
    // A new, explicitly requested binding clears the advisory for that room;
    // historical departure evidence remains immutable on the prior assignment.
    for (name,remaining) in [("kicked",1),("legacy_left",0)] {
        sql_query("INSERT INTO hagency_agent_v1.shared_agent_rooms(id,assignment_id,agent_id,project_id,room_id,state) VALUES($1,$2,$3,'recovery_space',$4,'joining')").bind::<Text,_>(format!("explicit_reinvite_{name}")).bind::<Text,_>(&assignment.id).bind::<Text,_>(&r.agent_id).bind::<Text,_>(format!("!{name}:example.test")).execute(&mut room_db).await.unwrap();
        assert_eq!(f.domain.pool_agent_request(&f.owner.p,&recovered.id).await.unwrap().rooms_requiring_reinvite,remaining);
    }
    assert_eq!(assignment.agent_id, old_assignment.agent_id);
    assert_eq!(intent_state(&f, &r.id).await, "fenced");
    assert_eq!(
        f.domain
            .managed_agent_assignment(&f.owner.p, &f.policy, &r.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "fenced"
    );
    assert!(
        !f.domain
            .pool_agent_request(&f.owner.p, &r.id)
            .await
            .unwrap()
            .can_reallocate
    );
    f.domain
        .stop_pool_agent(
            &f.owner.p,
            &recovered.id,
            RevokeInput {
                command_id: "stop_new_generation".into(),
                expected_revision: recovered.revision,
            },
        )
        .await
        .unwrap();
    acknowledge_fence(&f, 104).await;
    assert!(
        !f.domain
            .pool_agent_request(&f.owner.p, &recovered.id)
            .await
            .unwrap()
            .can_reallocate
    );
}

async fn publication_case() -> (Fixture, Admission, PublishGrantInput, i64, i64) {
    let f = succession_fixture().await;
    let a = admitted(&f).await;
    let now = crate::api::now_ms();
    let old = now - 4 * 86400000;
    time(&f, old).await;
    let calendar = Calendar {
        cadence: Cadence::Daily,
        time_zone: "America/Los_Angeles".into(),
    };
    let p = calendar.period_at(old as u64).unwrap();
    let mut input = grant_input(&f, &a, &f.outsider.p).await;
    input.receipt.version = 4;
    input.receipt.admission_request_id = Some(a.request.id.clone());
    input.receipt.calendar = Some(calendar);
    input.receipt.period_key = p.key;
    input.receipt.period_start_ms = p.start_ms as i64;
    input.receipt.period_end_ms = p.end_ms as i64;
    input.receipt.delegation_expires_at_ms = p.end_ms as i64;
    (f, a, input, old, now)
}
fn expiration(a: &Admission, input: &PublishGrantInput) -> ExpireGrantPublication {
    ExpireGrantPublication {
        operation: "grant.publish".into(),
        target_id: a.contribution.id.clone(),
        publication: serde_json::to_value(input).unwrap(),
    }
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn expired_publication_tombstone_blocks_late_delivery_and_replays_exact_scope() {
    let (f, a, input, old, now) = publication_case().await;
    let exp = expiration(&a, &input);
    assert_eq!(
        code(
            f.domain
                .expire_management_grant_publication(
                    &f.outsider.p,
                    &f.policy,
                    &input.command_id,
                    exp.clone()
                )
                .await
        ),
        "contribution_executor_required"
    );
    assert_eq!(
        code(
            f.domain
                .expire_management_grant_publication(
                    &f.owner.p,
                    &f.policy,
                    &input.command_id,
                    exp.clone()
                )
                .await
        ),
        "grant_publication_not_expired"
    );
    time(&f, now).await;
    let result = f
        .domain
        .expire_management_grant_publication(&f.owner.p, &f.policy, &input.command_id, exp.clone())
        .await
        .unwrap();
    assert_eq!(result.status, "expired_unpublished");
    assert!(result.grant.is_none());
    assert_eq!(
        result.publication_digest,
        allocation_digest(&input).unwrap()
    );
    assert_eq!(
        result.receipt_digest,
        allocation_digest(&input.receipt).unwrap()
    );
    assert_eq!(
        f.domain
            .expire_management_grant_publication(&f.owner.p, &f.policy, &input.command_id, exp)
            .await
            .unwrap()
            .status,
        "expired_unpublished"
    );
    // Simulate a delayed publisher whose pre-lock clock was in the old period:
    // even that writer cannot revive the tombstoned command.
    time(&f, old).await;
    assert_eq!(
        code(
            f.domain
                .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
                .await
        ),
        "grant_publication_expired"
    );
    assert!(chain(&f, &a.contribution.id).await.is_empty());
}
#[tokio::test]
#[ignore = "requires dedicated PostgreSQL"]
async fn publication_expiry_serializes_with_publish_and_recovers_accepted_lost_reply() {
    let (f, a, input, old, now) = publication_case().await;
    let other = crate::domain::DomainStore::open(&f.url, "example.test", "_hagency_test_")
        .await
        .unwrap();
    other.set_management_test_clock(now).await.unwrap();
    let exp = expiration(&a, &input);
    let (published, expired) = tokio::join!(
        f.domain
            .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone()),
        other.expire_management_grant_publication(
            &f.owner.p,
            &f.policy,
            &input.command_id,
            exp.clone()
        )
    );
    let expired = expired.unwrap();
    if let Ok(published) = published {
        assert_eq!(expired.status, "published");
        assert_eq!(expired.grant.unwrap().id, published.id);
    } else {
        assert_eq!(expired.status, "expired_unpublished");
        assert!(chain(&f, &a.contribution.id).await.is_empty());
    }
    let (f, a, input, _, now) = publication_case().await;
    let published = f
        .domain
        .publish_management_grant(&f.owner.p, &f.policy, &a.contribution.id, input.clone())
        .await
        .unwrap();
    time(&f, now).await;
    let recovered = f
        .domain
        .expire_management_grant_publication(
            &f.owner.p,
            &f.policy,
            &input.command_id,
            expiration(&a, &input),
        )
        .await
        .unwrap();
    assert_eq!(recovered.status, "published");
    assert_eq!(
        recovered.grant.unwrap().receipt_digest,
        published.receipt_digest
    );
    assert!(old < now);
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn lost_period_receipt_recovers_after_expiry_without_authorizing_a_later_period() {
    let(f,a,g,r,_,now)=recurring(RenewalMode::Owner).await;
    let current=current_grant(&f,&a,&g,now-2*86400000).await;
    f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,RenewPoolAgent{command_id:"late_period_receipt".into(),expected_revision:r.revision,expected_period_key:current.receipt.period_key}).await.unwrap();
    let d=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    let AgentExecutorCommand::RenewPeriod(c)=&d.command else{panic!("expected renew")};
    time(&f,now).await;
    let recovery=f.domain.recover_managed_agent_executor(&f.owner.p,&f.policy,&c.command_id,ExecutorRecoverInput{command_digest:d.command_digest.clone()}).await.unwrap();
    assert!(!recovery.execution_allowed);assert!(recovery.lease_until_ms>now+1000);
    let receipt=ManagedAgentPeriodReceipt{version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),request_id:c.reserve_command.request_id.clone(),agent_id:c.reserve_command.proposed_agent_id.clone(),agent_reservation_id:c.agent_reservation_id.clone(),period:c.period.clone(),allocated_tokens:c.reserve_command.requested_tokens,ledger_revision:102,state:"authorized".into(),reason:None};
    f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,&c.command_id,AgentExecutorResultInput{command_digest:d.command_digest,token:recovery.token,receipt:AgentExecutorReceipt::RenewPeriod(receipt)}).await.unwrap();
    let view=f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap();
    assert_eq!(view.current_period.unwrap().state,"awaiting_capacity");
    assert_ne!(view.blocking_reason.as_deref(),Some("pool_agent_authority_changed"));
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn late_period_receipt_must_precede_the_permanent_native_fence() {
    let(f,a,g,r,_,now)=recurring(RenewalMode::Owner).await;
    let current=current_grant(&f,&a,&g,now).await;
    f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,RenewPoolAgent{command_id:"renew_before_fence".into(),expected_revision:r.revision,expected_period_key:current.receipt.period_key}).await.unwrap();
    let d=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    let AgentExecutorCommand::RenewPeriod(c)=&d.command else{panic!("expected renew")};
    f.domain.stop_pool_agent(&f.owner.p,&r.id,RevokeInput{command_id:"stop_with_pending_period".into(),expected_revision:r.revision}).await.unwrap();
    acknowledge_fence(&f,103).await;
    let mut receipt=ManagedAgentPeriodReceipt{version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),request_id:c.reserve_command.request_id.clone(),agent_id:c.reserve_command.proposed_agent_id.clone(),agent_reservation_id:c.agent_reservation_id.clone(),period:c.period.clone(),allocated_tokens:c.reserve_command.requested_tokens,ledger_revision:104,state:"authorized".into(),reason:None};
    let bad=AgentExecutorResultInput{command_digest:d.command_digest.clone(),token:d.token.clone(),receipt:AgentExecutorReceipt::RenewPeriod(receipt.clone())};
    assert_eq!(code(f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,&c.command_id,bad).await),"agent_executor_fence_conflict");
    receipt.ledger_revision=102;
    f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,&c.command_id,AgentExecutorResultInput{command_digest:d.command_digest,token:d.token,receipt:AgentExecutorReceipt::RenewPeriod(receipt)}).await.unwrap();
    let view=f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap();assert_eq!(view.current_period.unwrap().state,"revoked");assert!(!view.can_renew);assert!(!view.can_reallocate);
}

fn executor_id(c: &AgentExecutorCommand) -> &str {
    match c {
        AgentExecutorCommand::Reserve(c) => &c.command_id,
        AgentExecutorCommand::Fence(c) => &c.command_id,
        AgentExecutorCommand::RenewPeriod(c) => &c.command_id,
    }
}

async fn amend_limit(f: &Fixture, a: &Admission, old: &ResourceGrantView, limit: i64, ledger: i64) -> ResourceGrantView {
    let mut g = old.receipt.clone();
    g.grant_id = format!("mgs_{}", &secret_token()[..32]);
    g.grant_revision += 1;
    g.ledger_revision = ledger;
    g.per_request_tokens = Some(limit);
    g.succession = Some(GrantSuccession {kind:"amendment".into(), predecessor_grant_id:old.id.clone(), predecessor_receipt_digest:old.receipt_digest.clone(), carried_children:0, carried_retained_tokens:0});
    attempt(f,a,old,g).await.unwrap()
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn renewal_projection_enqueue_and_delivery_enforce_the_same_agent_limit() {
    let(f,a,g,r,_,now)=recurring_with_limits(RenewalMode::Owner,Some(1000)).await;
    let current=current_grant(&f,&a,&g,now).await;
    let small=amend_limit(&f,&a,&current,99,102).await;
    let view=f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap();
    assert_eq!(view.current_period.unwrap().state,"request_limit_exceeded");assert!(!view.can_renew);
    let mut input=RenewPoolAgent{command_id:"limit_renew".into(),expected_revision:r.revision,expected_period_key:current.receipt.period_key};
    assert_eq!(code(f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,input.clone()).await),"pool_agent_request_limit_exceeded");
    let enough=amend_limit(&f,&a,&small,100,103).await;
    assert!(f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap().can_renew);
    input.command_id="limit_renew_after_capacity".into();
    f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,input).await.unwrap();
    let d=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    assert!(d.execution_allowed);
    amend_limit(&f,&a,&enough,99,104).await;
    let c=executor_id(&d.command);
    let recovered=f.domain.recover_managed_agent_executor(&f.owner.p,&f.policy,c,ExecutorRecoverInput{command_digest:d.command_digest.clone()}).await.unwrap();
    assert!(!recovered.execution_allowed,"a queued renewal cannot bypass an amended request cap");
}

async fn refuse_period(f: &Fixture, d: &AgentExecutorDelivery, ledger: i64) {
    let AgentExecutorCommand::RenewPeriod(c)=&d.command else {panic!("renew")};
    let receipt=ManagedAgentPeriodReceipt{version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),request_id:c.reserve_command.request_id.clone(),agent_id:c.reserve_command.proposed_agent_id.clone(),agent_reservation_id:c.agent_reservation_id.clone(),period:c.period.clone(),allocated_tokens:0,ledger_revision:ledger,state:"refused".into(),reason:Some("native_authority_unavailable".into())};
    f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,&c.command_id,AgentExecutorResultInput{command_digest:d.command_digest.clone(),token:d.token.clone(),receipt:AgentExecutorReceipt::RenewPeriod(receipt)}).await.unwrap();
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn refused_renewals_allow_explicit_owner_retry_but_never_retry_unknown_or_authorized_outcomes() {
    for mode in [RenewalMode::Owner,RenewalMode::Automatic] {
        let(f,a,g,r,_,now)=recurring(mode).await;
        let current=current_grant(&f,&a,&g,now).await;
        let mut input=RenewPoolAgent{command_id:"first_renewal".into(),expected_revision:r.revision,expected_period_key:current.receipt.period_key};
        f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,input.clone()).await.unwrap();
        let d=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
        input.command_id="unknown_outcome_retry".into();
        assert!(!f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,input.clone()).await.unwrap().can_renew);
        assert!(f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().is_none());
        refuse_period(&f,&d,102).await;
        let failed=f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap();
        assert_eq!(failed.current_period.unwrap().state,"renewal_refused");assert!(failed.can_renew);
        assert!(!f.domain.pool_agent_request(&f.outsider.p,&r.id).await.unwrap().can_renew);
        assert!(f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().is_none(),"unchanged authority must not automatically repeat a definitive refusal");
        input.command_id="explicit_refusal_retry".into();
        assert_eq!(f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,input.clone()).await.unwrap().current_period.unwrap().state,"awaiting_executor");
        let retried=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
        assert_ne!(executor_id(&d.command),executor_id(&retried.command));
        acknowledge_period(&f,&retried,103).await;
        input.command_id="never_double_fund".into();
        assert!(!f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,input).await.unwrap().can_renew);
        let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
        assert_eq!(sql_query("SELECT count(*)::bigint AS count FROM hagency_agent_v1.management_agent_executor_commands WHERE operation='agent.period.renew'").get_result::<Count>(&mut db).await.unwrap().count,2);
        assert!(sql_query("INSERT INTO hagency_agent_v1.management_agent_executor_commands(id,intent_id,operation,created_at_ms,command_payload,period_key) SELECT 'duplicate_period',intent_id,operation,created_at_ms,command_payload,period_key FROM hagency_agent_v1.management_agent_executor_commands WHERE id=$1").bind::<Text,_>(executor_id(&retried.command)).execute(&mut db).await.is_err());
    }
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn failed_unreserved_reallocation_can_advance_to_generation_three() {
    for never_delivered in [true, false] {
    let(f,a,g,r,assignment,now)=recurring(RenewalMode::Owner).await;
    let current=current_grant(&f,&a,&g,now).await;
    let old=f.domain.stop_pool_agent(&f.owner.p,&r.id,RevokeInput{command_id:"stop_first".into(),expected_revision:r.revision}).await.unwrap();
    acknowledge_fence(&f,102).await;
    let second=f.domain.reallocate_pool_agent(&f.owner.p,&f.policy,&old.id,ReallocatePoolAgent{command_id:"second_generation".into(),expected_revision:old.revision,expected_grant_revision:current.receipt.grant_revision,renewal_mode:RenewalMode::Owner}).await.unwrap();
    let second=if never_delivered {
        f.domain.stop_pool_agent(&f.owner.p,&second.id,RevokeInput{command_id:"stop_unreserved_second".into(),expected_revision:second.revision}).await.unwrap()
    } else {
        let d=reserve_delivery(&f,&second.id).await;
        let AgentExecutorCommand::Reserve(c)=&d.command else {panic!("reserve")};
        let receipt=ManagedAgentReservationReceipt{version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),request_id:c.request_id.clone(),agent_id:c.proposed_agent_id.clone(),allocation_id:c.allocation_id.clone(),agent_reservation_id:None,definition_digest:c.definition_digest.clone(),state:"refused".into(),budget_revision:0,ledger_revision:103,allocated_tokens:0,retained_tokens:0,period_key:c.period_key.clone(),reason:Some("capacity_unavailable".into())};
        f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,&c.command_id,AgentExecutorResultInput{command_digest:d.command_digest,token:d.token,receipt:AgentExecutorReceipt::Reserve(receipt)}).await.unwrap();
        f.domain.pool_agent_request(&f.owner.p,&second.id).await.unwrap()
    };
    assert!(!second.can_reallocate,"absence without native proof is not recovery authority");
    let d=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    let AgentExecutorCommand::Fence(c)=&d.command else {panic!("fence")};
    assert_eq!(c.reserve_command.allocation_generation,Some(2));
    let receipt=ManagedAgentFenceReceipt{version:1,command_id:c.command_id.clone(),command_digest:d.command_digest.clone(),reserve_command_digest:c.reserve_command_digest.clone(),request_id:c.reserve_command.request_id.clone(),agent_id:c.reserve_command.proposed_agent_id.clone(),agent_reservation_id:None,state:"absent_fenced".into(),fence_revision:104,ledger_revision:104,known_consumed_tokens:0,retained_tokens:Some(0),usage_complete:true,process_state:"not_started".into()};
    f.domain.record_managed_agent_executor_result(&f.owner.p,&f.policy,&c.command_id,AgentExecutorResultInput{command_digest:d.command_digest,token:d.token,receipt:AgentExecutorReceipt::Fence(receipt)}).await.unwrap();
    assert!(f.domain.pool_agent_request(&f.owner.p,&second.id).await.unwrap().can_reallocate);
    assert!(!f.domain.pool_agent_request(&f.owner.p,&old.id).await.unwrap().can_reallocate);
    let third=f.domain.reallocate_pool_agent(&f.owner.p,&f.policy,&second.id,ReallocatePoolAgent{command_id:"third_generation".into(),expected_revision:second.revision,expected_grant_revision:current.receipt.grant_revision,renewal_mode:RenewalMode::Owner}).await.unwrap();
    assert_eq!(third.agent_id,r.agent_id);
    let d=reserve_delivery(&f,&third.id).await;
    let AgentExecutorCommand::Reserve(c)=&d.command else {panic!("reserve")};
    assert_eq!(c.allocation_generation,Some(3));assert!(d.execution_allowed);
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    let ancestor=sql_query("SELECT hagency_agent_v1.previous_agent_assignment($1) AS value").bind::<Text,_>(&third.id).get_result::<Stored>(&mut db).await.unwrap();
    assert_eq!(ancestor.value,assignment.id,"failed generations preserve the prior room assignment");
    }
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn automatic_renewal_isolates_invalid_agents_and_rotates_beyond_one_thousand() {
    let(f,a,g,r,_,now)=recurring(RenewalMode::Automatic).await;
    current_grant(&f,&a,&g,now).await;
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    // Disposable-fixture corruption simulates historical invalid durable inputs.
    // Every early Agent fails command validation. None may starve the real Agent.
    db.batch_execute("SET session_replication_role=replica").await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_agent_requests SELECT (jsonb_populate_record(NULL::hagency_agent_v1.management_agent_requests,to_jsonb(r)||jsonb_build_object('id','invalid_request_'||n,'proposed_agent_id','invalid_agent_'||n,'room_id','!invalid_'||n||':example.test'))).* FROM hagency_agent_v1.management_agent_requests r CROSS JOIN generate_series(1,1000) n WHERE r.id=$1").bind::<Text,_>(&r.id).execute(&mut db).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_agent_intents SELECT (jsonb_populate_record(NULL::hagency_agent_v1.management_agent_intents,to_jsonb(i)||jsonb_build_object('id','aaa_invalid_'||n,'request_id','invalid_request_'||n,'payload',(i.payload::jsonb||jsonb_build_object('version',0))::text))).* FROM hagency_agent_v1.management_agent_intents i CROSS JOIN generate_series(1,1000) n WHERE i.request_id=$1").bind::<Text,_>(&r.id).execute(&mut db).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_agent_recurring_terms SELECT 'invalid_request_'||n,calendar,renewal_mode FROM hagency_agent_v1.management_agent_recurring_terms CROSS JOIN generate_series(1,1000) n WHERE request_id=$1").bind::<Text,_>(&r.id).execute(&mut db).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_shared_assignments SELECT (jsonb_populate_record(NULL::hagency_agent_v1.management_shared_assignments,to_jsonb(a)||jsonb_build_object('id','invalid_assignment_'||n,'request_id','invalid_request_'||n,'agent_id','invalid_agent_'||n,'agent_reservation_id','invalid_child_'||n))).* FROM hagency_agent_v1.management_shared_assignments a CROSS JOIN generate_series(1,1000) n WHERE a.request_id=$1").bind::<Text,_>(&r.id).execute(&mut db).await.unwrap();
    sql_query("INSERT INTO hagency_agent_v1.management_agent_executor_commands SELECT (jsonb_populate_record(NULL::hagency_agent_v1.management_agent_executor_commands,to_jsonb(w)||jsonb_build_object('id','aaa_invalid_'||n,'intent_id','aaa_invalid_'||n))).* FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id CROSS JOIN generate_series(1,1000) n WHERE i.request_id=$1 AND w.operation='agent.reserve'").bind::<Text,_>(&r.id).execute(&mut db).await.unwrap();
    db.batch_execute("SET session_replication_role=origin").await.unwrap();
    assert!(f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().is_none());
    let d=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    let AgentExecutorCommand::RenewPeriod(c)=d.command else {panic!("renew")};
    assert_eq!(c.reserve_command.request_id,r.id);
    assert!(d.execution_allowed);
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn publication_expiry_uses_the_earlier_delegation_deadline() {
    let(f,a,mut input,_,_)=publication_case().await;
    let deadline=input.receipt.period_end_ms-1000;
    input.receipt.delegation_expires_at_ms=deadline;
    time(&f,deadline).await;
    let result=f.domain.expire_management_grant_publication(&f.owner.p,&f.policy,&input.command_id,expiration(&a,&input)).await.unwrap();
    assert_eq!(result.status,"expired_unpublished");
    assert_eq!(code(f.domain.publish_management_grant(&f.owner.p,&f.policy,&a.contribution.id,input).await),"grant_publication_expired");
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn split_limit_sql_constraint_and_recovery_migration_are_idempotent() {
    let(f,_,g,_,_,_)=recurring_with_limits(RenewalMode::Owner,Some(1000)).await;
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    for _ in 0..2 {
        db.batch_execute(include_str!("management_pool_limits_validation.sql")).await.unwrap();
        db.batch_execute(include_str!("management_agent_periods_recovery.sql")).await.unwrap();
    }
    let mut receipt=g.receipt.clone();
    for limit in [0,receipt.allocated_tokens+1] {
        receipt.grant_id=format!("invalid_limit_{limit}");receipt.per_request_tokens=Some(limit);
        let raw=serde_json::to_string(&receipt).unwrap();
        let result=sql_query("INSERT INTO hagency_agent_v1.management_resource_grants SELECT (jsonb_populate_record(NULL::hagency_agent_v1.management_resource_grants,to_jsonb(g)||jsonb_build_object('id',$2,'chain_root_id',$2,'receipt',$3,'state','revoked'))).* FROM hagency_agent_v1.management_resource_grants g WHERE id=$1")
            .bind::<Text,_>(&g.id).bind::<Text,_>(&receipt.grant_id).bind::<Text,_>(&raw).execute(&mut db).await;
        assert!(result.unwrap_err().to_string().contains("management_pool_split_limits"));
    }
    db.batch_execute(include_str!("management_shared_claude_down.sql")).await.unwrap();
    db.batch_execute(include_str!("management_shared_context_v2_down.sql")).await.unwrap();
    db.batch_execute(include_str!("management_publication_first_down.sql")).await.unwrap();
    assert!(db.batch_execute(include_str!("management_agent_periods_down.sql")).await.unwrap_err().to_string().contains("agent_period_downgrade_has_authority"),"existing recurring authority cannot be downgraded away");
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn automatic_refusal_retries_only_after_the_grant_changes() {
    let(f,a,g,r,_,now)=recurring_with_limits(RenewalMode::Automatic,Some(1000)).await;
    let current=current_grant(&f,&a,&g,now).await;
    let first=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    refuse_period(&f,&first,102).await;
    assert!(f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().is_none());
    let amended=amend_limit(&f,&a,&current,2000,103).await;
    let next=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    assert_ne!(executor_id(&first.command),executor_id(&next.command));
    acknowledge_period(&f,&next,104).await;
    assert_eq!(f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap().current_period.unwrap().state,"available");
    amend_limit(&f,&a,&amended,99,105).await;
    let funded=f.domain.pool_agent_request(&f.owner.p,&r.id).await.unwrap();
    assert_eq!(funded.current_period.unwrap().state,"available","a smaller future request maximum does not rescind this period's funded allowance");
    assert!(!funded.can_renew);
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn oversized_automatic_renewal_cannot_block_another_agents_fence() {
    let(f,a,g,r,_,now)=recurring_with_managers(RenewalMode::Automatic,None,true).await;
    let current=current_grant(&f,&a,&g,now).await;
    assert_eq!(code(f.domain.renew_pool_agent(&f.owner.p,&f.policy,&r.id,RenewPoolAgent{command_id:"oversized_manual".into(),expected_revision:r.revision,expected_period_key:current.receipt.period_key.clone()}).await),"management_payload_too_large");
    let mut input=input(&a,"unrelated_stop");input.expected_grant_revision=current.receipt.grant_revision;input.renewal_mode=Some(RenewalMode::Automatic);
    let other=f.domain.request_pool_agent(&f.outsider.p,&f.policy,&a.contribution.id,input).await.unwrap();
    let other=decide(&f,&other,true).await.unwrap();
    f.domain.stop_pool_agent(&f.owner.p,&other.id,RevokeInput{command_id:"stop_despite_oversized_neighbor".into(),expected_revision:other.revision}).await.unwrap();
    let d=f.domain.poll_managed_agent_executor(&f.owner.p,&f.policy,ExecutorPollInput{after:None}).await.unwrap().unwrap();
    let AgentExecutorCommand::Fence(c)=d.command else{panic!("fence")};
    assert_eq!(c.reserve_command.request_id,other.id);assert!(d.execution_allowed);
}

#[tokio::test]
#[ignore="requires dedicated PostgreSQL"]
async fn recurring_request_projection_preserves_rejection_and_expiry() {
    let(f,a,g,_,_,now)=recurring(RenewalMode::Owner).await;
    let current=current_grant(&f,&a,&g,now).await;
    let mut input=input(&a,"rejected_period");input.expected_grant_revision=current.receipt.grant_revision;input.renewal_mode=Some(RenewalMode::Owner);
    let r=f.domain.request_pool_agent(&f.outsider.p,&f.policy,&a.contribution.id,input.clone()).await.unwrap();
    let rejected=decide(&f,&r,false).await.unwrap();
    assert_eq!(rejected.current_period.unwrap().state,"rejected");
    input.command_id="expired_period".into();input.room_id="!expired_period:example.test".into();
    let pending=f.domain.request_pool_agent(&f.outsider.p,&f.policy,&a.contribution.id,input).await.unwrap();
    time(&f,pending.expires_at_ms).await;
    let expired=f.domain.pool_agent_request(&f.owner.p,&pending.id).await.unwrap();
    assert_eq!(expired.current_period.unwrap().state,"expired");assert!(!expired.can_renew);
}
