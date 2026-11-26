//! Read-only chat status from retained intake and exact work/reply evidence.
use super::*;

#[derive(QueryableByName)]
struct ChatStatus {
    #[diesel(sql_type = Text)]
    payload: String,
}

use crate::management_pool_types::budget_state;

pub(super) async fn query(
    db: &mut AsyncPgConnection,
    p: &Principal,
    room: Option<&str>,
    thread: Option<&str>,
    now: i64,
) -> Result<Vec<Value>> {
    let Some(room) = room else {
        return Ok(Vec::new());
    };
    // RoutedEvent uses snake_case. Do not return message bodies, provider output
    // or other participants' intake. Resource owners need not join the room.
    // Each row names its Agent by Matrix ID, which every member of the room
    // already sees, and says whether its reader can send the message again.
    let rows = sql_query(r#"
        SELECT jsonb_build_object(
            'agentId', a.agent_id, 'agentMxid', mx.puppet_mxid,
            'roomId', i.event_payload::jsonb->>'room_id',
            'eventId', i.event_id,
            'fromViewer', i.event_payload::jsonb->>'sender_mxid'=$2,
            'agentAvailable', control.mode='running' AND EXISTS(
                SELECT 1 FROM hagency_agent_v1.shared_room_memberships m
                WHERE m.assignment_id=a.id AND m.room_id=i.event_payload::jsonb->>'room_id'
                  AND m.state='matrix_verified'),
            -- Final for this Agent, and no turn began for it: it was
            -- withdrawn or stopped while it waited, arrived while the Agent
            -- was stopped, waited out its time, its start failed for good, or
            -- the process it was given to ended first. Nothing but sending it
            -- again will move it. A block on allowance is not such an end:
            -- the message goes on when allowance is granted.
            'endedWithoutTurn', w.started_at_ms IS NULL AND (
                i.state IN ('cancelled','expired','paused')
                OR (i.state='blocked' AND i.reason NOT IN ('budget_blocked','usage_unrepresentable'))
                OR (i.state='bound' AND w.state='cancelled')),
            'threadRoot', coalesce(i.event_payload::jsonb->>'thread_root', i.event_id),
            'receivedAtMs', i.received_at_ms,
            'state', CASE
                WHEN reply.state='sent' THEN 'replied'
                WHEN reply.state IN ('unknown','blocked') THEN 'reply_unknown'
                WHEN w.state='completed' THEN 'reply_pending'
                WHEN w.state='started' AND (r.state<>'qualified' OR r.activity_until_ms<=$4) THEN 'unknown'
                WHEN w.state IS NOT NULL AND w.state NOT IN ('queued','acknowledged') THEN w.state
                WHEN queue.status IS NOT NULL THEN queue.status
                WHEN w.state IS NOT NULL THEN w.state
                ELSE i.state END,
            'reason', CASE WHEN queue.status IS NOT NULL THEN queue.reason
                -- Accepted, never begun, and withdrawn: its process ended,
                -- or its budget or period changed first. A start is recorded
                -- for accepted work only, so nothing of this message has run
                -- or will.
                WHEN w.state='cancelled' AND w.started_at_ms IS NULL THEN 'never_started'
                ELSE i.reason END,
            'resourceRecovery', CASE WHEN queue.reason='budget_blocked' AND req.id IS NOT NULL
                THEN jsonb_build_object('requestId',req.id,'role',
                    CASE WHEN req.administrator_user_id=$1 THEN 'owner' ELSE 'manager' END) END,
            'budgetBlock', CASE WHEN queue.reason='budget_blocked' THEN budget_block.payload::jsonb END
        )::text AS payload
        FROM hagency_agent_v1.management_shared_intake i
        JOIN hagency_agent_v1.management_shared_assignments a ON a.id=i.assignment_id
        LEFT JOIN hagency_agent_v1.management_agent_matrix mx ON mx.assignment_id=a.id
        LEFT JOIN hagency_agent_v1.management_shared_controls control ON control.assignment_id=a.id
        LEFT JOIN hagency_agent_v1.management_agent_requests req ON req.id=a.request_id
            AND req.pool_contribution_id IS NOT NULL AND $1 IN (req.manager_user_id,req.administrator_user_id)
        LEFT JOIN LATERAL (
            SELECT hagency_agent_v1.shared_executor_protocol_reason(
                a.executor_owner_user_id,a.executor_device_id,a.executor_generation,$4) AS reason
        ) protocol ON true
        LEFT JOIN hagency_agent_v1.management_shared_event_pins pin
            ON pin.assignment_id=i.assignment_id AND pin.event_id=i.event_id
        LEFT JOIN hagency_agent_v1.management_shared_work w ON w.id=pin.work_id
        LEFT JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id
        LEFT JOIN hagency_agent_v1.management_shared_replies reply ON reply.work_id=w.id
        LEFT JOIN hagency_agent_v1.management_shared_demands d ON d.intake_id=i.id
        LEFT JOIN hagency_agent_v1.management_shared_budget_demand_terminals t ON t.demand_id=d.id
        LEFT JOIN LATERAL (
            SELECT max(br.budget_revision) AS revision
            FROM hagency_agent_v1.management_agent_budget_commands bc
            JOIN hagency_agent_v1.management_agent_budget_results br ON br.command_id=bc.id
            WHERE bc.request_id=a.request_id AND br.state='applied'
        ) applied ON true
        LEFT JOIN LATERAL (
            SELECT b.payload,b.reason FROM hagency_agent_v1.management_shared_demand_blocks b
            WHERE b.demand_id=d.id AND (b.reason<>'budget_blocked' OR applied.revision IS NULL)
            UNION ALL
            SELECT b.payload,b.reason FROM hagency_agent_v1.management_shared_budget_demand_blocks b
            WHERE b.demand_id=d.id AND b.budget_revision=applied.revision
        ) budget_block ON true
        LEFT JOIN LATERAL (
            SELECT CASE WHEN t.reason='expired' THEN 'expired'
                        WHEN reason IS NULL THEN 'pending' ELSE 'blocked' END AS status, reason
            FROM (SELECT coalesce(t.reason, protocol.reason, budget_block.reason,
                CASE WHEN i.reason='budget_blocked' AND applied.revision IS NOT NULL THEN NULL ELSE i.reason END) AS reason) reasons
            WHERE w.started_at_ms IS NULL AND (w.state IS NULL OR w.state IN ('queued','acknowledged'))
              AND (i.state='pending' OR (i.state='blocked' AND i.reason='budget_blocked') OR w.state IN ('queued','acknowledged'))
              AND (reason IS NOT NULL OR (i.state='blocked' AND i.reason='budget_blocked' AND applied.revision IS NOT NULL))
        ) queue ON true
        WHERE i.event_payload::jsonb->>'room_id'=$3
          AND ($5::text IS NULL OR coalesce(i.event_payload::jsonb->>'thread_root', i.event_id)=$5)
          AND (a.executor_owner_user_id=$1 OR
               i.event_payload::jsonb->>'sender_mxid'=$2)
        ORDER BY i.received_at_ms DESC, i.id DESC LIMIT 16
    "#)
    .bind::<Text, _>(&p.user_id)
    .bind::<Text, _>(&p.mxid)
    .bind::<Text, _>(room)
    .bind::<BigInt, _>(now)
    .bind::<Nullable<Text>, _>(thread)
    .load::<ChatStatus>(db).await?;
    let mut values = rows
        .into_iter()
        .map(|r| {
            let mut value: Value = decode(&r.payload)?;
            let block = value
                .as_object_mut()
                .and_then(|v| v.remove("budgetBlock"))
                .unwrap_or(Value::Null);
            // Publish only the reason, never the private liability digest or
            // another message's details. A zero balance is not proof of spending.
            if let Some(state) = budget_state(&block) {
                value["budgetState"] = json!(state);
            }
            let mut take =
                |key: &str| value.as_object_mut().and_then(|v| v.remove(key)) == Some(json!(true));
            let (sender, available, ended) = (
                take("fromViewer"),
                take("agentAvailable"),
                take("endedWithoutTurn"),
            );
            value["canSendAgain"] = json!(sender && available && ended);
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    // An obsolete worker cannot decrypt a candidate to confirm addressing.
    // Expose only this sender's reference (or the resource owner's), never
    // pretend that its encrypted body has already been admitted as work.
    let candidates=sql_query(r#"
        SELECT jsonb_build_object(
            'agentId',a.agent_id,'agentMxid',m.puppet_mxid,
            'roomId',c.payload::jsonb->>'room_id',
            'eventId',c.event_id,'threadRoot',c.event_id,
            'receivedAtMs',c.received_at_ms,'state','blocked','reason',protocol.reason,
            'canSendAgain',false
        )::text AS payload
        FROM hagency_agent_v1.management_shared_e2ee_candidates c
        JOIN hagency_agent_v1.management_shared_assignments a ON a.id=c.assignment_id
        JOIN hagency_agent_v1.management_shared_controls control ON control.assignment_id=a.id
        JOIN hagency_agent_v1.shared_room_memberships m ON m.assignment_id=a.id
            AND m.room_id=c.payload::jsonb->>'room_id' AND m.state='matrix_verified'
            AND (c.room_binding_id IS NULL OR
                (m.binding_id=c.room_binding_id AND m.binding_generation=c.room_binding_generation))
        CROSS JOIN LATERAL (SELECT hagency_agent_v1.shared_executor_protocol_reason(
            a.executor_owner_user_id,a.executor_device_id,a.executor_generation,$4) AS reason) protocol
        WHERE c.payload::jsonb->>'room_id'=$3 AND c.state<>'ignored'
            AND ($5::text IS NULL OR c.event_id=$5)
            AND c.received_at_ms>$4-86400000 AND control.mode='running' AND a.state='pending_native'
            AND (a.executor_owner_user_id=$1 OR c.payload::jsonb->>'sender_mxid'=$2)
            AND protocol.reason IS NOT NULL
            AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake i
                WHERE i.assignment_id=a.id AND i.event_id=c.event_id)
        ORDER BY c.received_at_ms DESC,c.event_id DESC LIMIT 16
    "#).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(room)
        .bind::<BigInt,_>(now).bind::<Nullable<Text>,_>(thread).load::<ChatStatus>(db).await?;
    for row in candidates {
        values.push(decode(&row.payload)?);
    }
    values.sort_by(|a, b| {
        b["receivedAtMs"]
            .as_i64()
            .cmp(&a["receivedAtMs"].as_i64())
            .then_with(|| b["eventId"].as_str().cmp(&a["eventId"].as_str()))
    });
    values.truncate(16);
    Ok(values)
}
