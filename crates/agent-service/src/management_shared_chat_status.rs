//! Read-only chat status from retained intake and exact work/reply evidence.
use super::*;

#[derive(QueryableByName)]
struct ChatStatus {
    #[diesel(sql_type = Text)]
    payload: String,
}
#[derive(QueryableByName)]
struct Shown {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Text)]
    assignment_id: String,
}

use crate::management_pool_types::budget_state;

/// Whether the Agent's reservation stands now: the check a new message meets
/// when it arrives and the polls make before they offer one (neither the
/// reservation nor its intent is fenced, and its time is not over).
async fn reservation_stands(
    db: &mut AsyncPgConnection,
    id: &str,
    server: &str,
    now: i64,
) -> Result<bool> {
    use super::super::super::intake;
    let stands = match intake::assignment(db, id).await {
        Ok(w) => intake::retainable(db, id, &w, server, now).await,
        Err(e) => Err(e),
    };
    match stands {
        Err(Error::Conflict(_) | Error::NotFound(_)) => Ok(false),
        stands => stands,
    }
}

pub(super) async fn query(
    db: &mut AsyncPgConnection,
    p: &Principal,
    room: Option<&str>,
    thread: Option<&str>,
    now: i64,
    server: &str,
) -> Result<Vec<Value>> {
    let Some(room) = room else {
        return Ok(Vec::new());
    };
    // The reader's 16 newest messages of the room: their own, and every
    // message to an Agent they own.
    let shown = sql_query(
        r#"
        SELECT i.id, i.assignment_id FROM hagency_agent_v1.management_shared_intake i
        JOIN hagency_agent_v1.management_shared_assignments a ON a.id=i.assignment_id
        WHERE i.event_payload::jsonb->>'room_id'=$3
          AND ($4::text IS NULL OR coalesce(i.event_payload::jsonb->>'thread_root', i.event_id)=$4)
          AND (a.executor_owner_user_id=$1 OR i.event_payload::jsonb->>'sender_mxid'=$2)
        ORDER BY i.received_at_ms DESC, i.id DESC LIMIT 16
    "#,
    )
    .bind::<Text, _>(&p.user_id)
    .bind::<Text, _>(&p.mxid)
    .bind::<Text, _>(room)
    .bind::<Nullable<Text>, _>(thread)
    .load::<Shown>(db)
    .await?;
    // A message is answered in the period it arrived in. Each Agent's period
    // is read now, as its poll would read it, so that an end no poll has
    // recorded yet is read all the same; and so is whether its reservation
    // stands.
    let mut periods = std::collections::HashMap::new();
    let mut standing = Vec::new();
    let mut shown_rows = serde_json::Map::new();
    for s in shown {
        let start = match periods.get(&s.assignment_id) {
            Some(start) => *start,
            None => {
                let start =
                    super::super::super::intake::period_start(db, &s.assignment_id, now).await?;
                if reservation_stands(db, &s.assignment_id, server, now).await? {
                    standing.push(s.assignment_id.clone());
                }
                periods.insert(s.assignment_id, start);
                start
            }
        };
        shown_rows.insert(s.id, json!(start));
    }
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
            'agentAvailable', serves.available,
            'endReason', ended.reason,
            -- A copy sent now could run. After a block on usage it could
            -- only once the Agent's period has turned since: the calls the
            -- computer could not account for stay in the period they were
            -- made in, and an Agent without periods has a single one.
            'copyCanRun', ended.reason IS DISTINCT FROM 'usage_unrepresentable'
                OR i.received_at_ms<shown.period_start::bigint,
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
                -- A message given back by a process that ended before its
                -- turn, which then ended without one, reads as any end of a
                -- waiting message: `head_changed`, which every reader's
                -- Desktop reads as "not processed", with its block's cause as
                -- `endReason` (`retry_lapsed`, `retry_exhausted`,
                -- `retry_unsupported`, `reoffer_unsupported` or
                -- `head_changed`).
                ELSE i.reason END,
            'resourceRecovery', CASE WHEN queue.reason='budget_blocked' AND req.id IS NOT NULL
                THEN jsonb_build_object('requestId',req.id,'role',
                    CASE WHEN req.administrator_user_id=$1 THEN 'owner' ELSE 'manager' END) END,
            'budgetBlock', CASE WHEN queue.reason='budget_blocked' THEN budget_block.payload::jsonb END
        )::text AS payload
        FROM hagency_agent_v1.management_shared_intake i
        JOIN jsonb_each_text($3::jsonb) shown(intake_id, period_start) ON shown.intake_id=i.id
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
        -- The budget queue's end. Its `expired` is the end the intake records
        -- as `event_expired` (the message waited out its time), and reads so.
        LEFT JOIN LATERAL (
            SELECT CASE x.reason WHEN 'expired' THEN 'event_expired' ELSE x.reason END AS reason
            FROM hagency_agent_v1.management_shared_budget_demand_terminals x WHERE x.demand_id=d.id
        ) t ON true
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
        -- An end of a message that could still be offered (one that waits,
        -- or waits for allowance) which no write has recorded yet: what the
        -- next poll would record (intake::expire, the budget poll). Read
        -- here, nothing is written.
        LEFT JOIN LATERAL (
            SELECT CASE e.reason WHEN 'event_expired' THEN 'expired'
                                 WHEN 'head_changed' THEN 'blocked' ELSE 'cancelled' END AS state,
                   e.reason
            FROM (SELECT CASE
                WHEN i.room_binding_id IS NOT NULL AND NOT EXISTS(
                    SELECT 1 FROM hagency_agent_v1.shared_room_memberships m
                    WHERE m.binding_id=i.room_binding_id AND m.binding_generation=i.room_binding_generation)
                    THEN 'room_authority_changed'
                WHEN i.expires_at_ms<=$4 OR i.received_at_ms<shown.period_start::bigint
                    THEN 'event_expired'
                -- Only a stop changes the revision, and nothing is offered
                -- for an older one.
                WHEN control.mode IS DISTINCT FROM 'running' OR control.revision<>i.control_revision
                    THEN 'explicit_stop'
                -- Started under a protocol an applied budget retired: it can
                -- neither be bound there nor started in the budget's (the
                -- budget poll ends it so).
                WHEN applied.revision IS NOT NULL AND EXISTS(
                    SELECT 1 FROM hagency_agent_v1.management_shared_demand_starts s WHERE s.demand_id=d.id)
                    THEN 'head_changed'
                END AS reason) e
            WHERE e.reason IS NOT NULL AND w.id IS NULL
              AND (i.state='pending' OR (i.state='blocked' AND i.reason='budget_blocked'))
        ) lapse ON true
        LEFT JOIN LATERAL (
            SELECT CASE WHEN t.reason='event_expired' THEN 'expired'
                        WHEN t.reason IS NULL AND lapse.state IS NOT NULL THEN lapse.state
                        WHEN reason IS NULL THEN 'pending' ELSE 'blocked' END AS status, reason
            FROM (SELECT coalesce(t.reason, lapse.reason, protocol.reason, budget_block.reason,
                CASE WHEN i.reason='budget_blocked' AND applied.revision IS NOT NULL THEN NULL ELSE i.reason END) AS reason) reasons
            -- A message v2 blocked other than on allowance has ended there,
            -- and keeps the reason v2 gave it, also once the budget poll has
            -- recorded an end of its own for it.
            WHERE w.started_at_ms IS NULL AND (w.state IS NULL OR w.state IN ('queued','acknowledged'))
              AND (i.state='pending'
                   OR (i.state='blocked' AND i.reason='budget_blocked')
                   OR w.state IN ('queued','acknowledged'))
              AND (reason IS NOT NULL OR (i.state='blocked' AND i.reason='budget_blocked' AND applied.revision IS NOT NULL))
        ) queue ON true
        -- Final for this Agent, and no turn began for it: nothing but sending
        -- it again will move it. The value is the reason of the record that
        -- ended it; a message that still waits, or waits for allowance in its
        -- period, has none. Every record that can end a message is read here.
        LEFT JOIN LATERAL (
            SELECT CASE
                WHEN w.started_at_ms IS NOT NULL OR w.state IN ('queued','acknowledged') THEN NULL
                -- Given to a process that ended, or refused it, first.
                WHEN w.id IS NOT NULL THEN 'never_started'
                WHEN i.state IN ('cancelled','expired','paused') THEN i.reason
                -- A v2 end names its cause: head_changed, retry_exhausted,
                -- retry_unsupported, or usage_unrepresentable (a block on
                -- usage, which nothing lifts in its period), and for a
                -- message given back also retry_lapsed or reoffer_unsupported.
                WHEN i.state='blocked' AND i.reason<>'budget_blocked'
                    THEN coalesce((SELECT b.payload::jsonb#>>'{}' FROM hagency_agent_v1.management_shared_demand_blocks b
                        WHERE b.demand_id=d.id AND jsonb_typeof(b.payload::jsonb)='string'), i.reason)
                WHEN t.reason IS NOT NULL THEN t.reason
                ELSE lapse.reason END AS reason
        ) ended ON true
        -- The Agent can serve a new message in this room now: its reservation
        -- stands (neither it nor its intent is fenced, and its time is not
        -- over: $5, read by the check a new message meets when it arrives
        -- and the polls make before they offer one), its executor's ledger
        -- was not replaced, it runs, and it is a verified member of the room.
        -- An Agent with an applied budget serves too, through its own queue.
        -- The one definition of an available Agent, which also decides
        -- whether a message is given back at its process's fence (migration
        -- 2026-11-26-000157).
        LEFT JOIN LATERAL (
            SELECT hagency_agent_v1.shared_agent_serves(a.id, i.event_payload::jsonb->>'room_id')
                AND a.id=ANY($5::text[]) AS available
        ) serves ON true
        WHERE a.executor_owner_user_id=$1 OR i.event_payload::jsonb->>'sender_mxid'=$2
        ORDER BY i.received_at_ms DESC, i.id DESC
    "#)
    .bind::<Text, _>(&p.user_id)
    .bind::<Text, _>(&p.mxid)
    .bind::<Text, _>(Value::Object(shown_rows).to_string())
    .bind::<BigInt, _>(now)
    .bind::<Array<Text>, _>(standing)
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
            let mut take = |key: &str| value.as_object_mut().and_then(|v| v.remove(key));
            let (sender, available, runs) = (
                take("fromViewer") == Some(json!(true)),
                take("agentAvailable") == Some(json!(true)),
                take("copyCanRun") == Some(json!(true)),
            );
            let end = take("endReason").filter(Value::is_string);
            value["canSendAgain"] = json!(sender && available && runs && end.is_some());
            if let Some(end) = end {
                value["endReason"] = end;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_state_preserves_known_lower_bound_and_old_evidence() {
        let exhausted = "known_consumption_exhausted";
        for block in [
            json!(exhausted),
            json!({"reason":exhausted}),
            json!({"reason":"budget_blocked","source":exhausted}),
        ] {
            assert_eq!(budget_state(&block), Some("exhausted"));
        }
        for (known, complete, expected) in [
            (99, false, Some("usage_unresolved")),
            (100, false, Some("exhausted")),
            (104, false, Some("exhausted")),
            (100, true, Some("exhausted")),
            (90, true, None),
        ] {
            let block = json!({"budget":{"knownConsumedTokens":known,"allocatedTokens":100,"usageComplete":complete}});
            assert_eq!(budget_state(&block), expected);
        }
        assert_eq!(budget_state(&json!({"budget":{"remainingTokens":0}})), None);
    }
}
