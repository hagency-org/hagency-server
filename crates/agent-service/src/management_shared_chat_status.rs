//! Read-only chat status from retained intake and exact work/reply evidence.
use super::*;

#[derive(QueryableByName)]
struct ChatStatus {
    #[diesel(sql_type = Text)]
    payload: String,
}

pub(super) async fn query(
    db: &mut AsyncPgConnection,
    p: &Principal,
    room: Option<&str>,
    now: i64,
) -> Result<Vec<Value>> {
    let Some(room) = room else { return Ok(Vec::new()); };
    // RoutedEvent uses snake_case. Do not return message bodies, provider output
    // or other participants' intake. Resource owners need not join the room.
    let rows = sql_query(r#"
        SELECT jsonb_build_object(
            'agentId', a.agent_id, 'roomId', i.event_payload::jsonb->>'room_id',
            'eventId', i.event_id,
            'threadRoot', coalesce(i.event_payload::jsonb->>'thread_root', i.event_id),
            'receivedAtMs', i.received_at_ms,
            'state', CASE
                WHEN reply.state='sent' THEN 'replied'
                WHEN reply.state IN ('unknown','blocked') THEN 'reply_unknown'
                WHEN w.state='completed' THEN 'reply_pending'
                WHEN w.state='started' AND (r.state<>'qualified' OR r.activity_until_ms<=$4) THEN 'unknown'
                WHEN w.started_at_ms IS NULL AND (i.state='pending' OR w.state IN ('queued','acknowledged'))
                    AND protocol.reason IS NOT NULL THEN 'blocked'
                WHEN w.state IS NOT NULL THEN w.state
                ELSE i.state END,
            'reason', CASE WHEN w.started_at_ms IS NULL AND (i.state='pending' OR w.state IN ('queued','acknowledged'))
                THEN coalesce(protocol.reason,i.reason) ELSE i.reason END,
            'budgetBlock', budget_block.payload::jsonb
        )::text AS payload
        FROM hagency_agent_v1.management_shared_intake i
        JOIN hagency_agent_v1.management_shared_assignments a ON a.id=i.assignment_id
        LEFT JOIN LATERAL (
            SELECT hagency_agent_v1.shared_executor_protocol_reason(
                a.executor_owner_user_id,a.executor_device_id,a.executor_generation,$4) AS reason
        ) protocol ON true
        LEFT JOIN hagency_agent_v1.management_shared_event_pins pin
            ON pin.assignment_id=i.assignment_id AND pin.event_id=i.event_id
        LEFT JOIN hagency_agent_v1.management_shared_work w ON w.id=pin.work_id
        LEFT JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id
        LEFT JOIN hagency_agent_v1.management_shared_replies reply ON reply.work_id=w.id
        LEFT JOIN LATERAL (
            SELECT blocks.payload FROM (
                SELECT b.payload, 0::bigint AS revision
                FROM hagency_agent_v1.management_shared_demands d
                JOIN hagency_agent_v1.management_shared_demand_blocks b ON b.demand_id=d.id
                WHERE d.intake_id=i.id
                UNION ALL
                SELECT b.payload, b.budget_revision AS revision
                FROM hagency_agent_v1.management_shared_demands d
                JOIN hagency_agent_v1.management_shared_budget_demand_blocks b ON b.demand_id=d.id
                WHERE d.intake_id=i.id
            ) blocks ORDER BY revision DESC LIMIT 1
        ) budget_block ON true
        WHERE i.event_payload::jsonb->>'room_id'=$3
          AND (a.executor_owner_user_id=$1 OR
               i.event_payload::jsonb->>'sender_mxid'=$2)
        ORDER BY i.received_at_ms DESC, i.id DESC LIMIT 16
    "#)
    .bind::<Text, _>(&p.user_id)
    .bind::<Text, _>(&p.mxid)
    .bind::<Text, _>(room)
    .bind::<BigInt, _>(now)
    .load::<ChatStatus>(db).await?;
    let mut values = rows.into_iter().map(|r| {
        let mut value:Value=decode(&r.payload)?;
        let block=value.as_object_mut().and_then(|v|v.remove("budgetBlock")).unwrap_or(Value::Null);
        // Publish only the reason, never the private liability digest or
        // another message's details. A zero balance is not proof of spending.
        let budget=&block["budget"];
        let state=if budget["usageComplete"]==false && budget["knownConsumedTokens"].as_i64().zip(budget["allocatedTokens"].as_i64()).is_some_and(|(known,total)|known<total) {
            Some("usage_unresolved")
        } else if budget["usageComplete"]==true || block=="known_consumption_exhausted" || block["reason"]=="known_consumption_exhausted" {
            Some("exhausted")
        } else {None};
        if let Some(state)=state {value["budgetState"]=json!(state);}
        Ok(value)
    }).collect::<Result<Vec<_>>>()?;
    // An obsolete worker cannot decrypt a candidate to confirm addressing.
    // Expose only this sender's reference (or the resource owner's), never
    // pretend that its encrypted body has already been admitted as work.
    let candidates=sql_query(r#"
        SELECT jsonb_build_object(
            'agentId',a.agent_id,'roomId',c.payload::jsonb->>'room_id',
            'eventId',c.event_id,'threadRoot',c.event_id,
            'receivedAtMs',c.received_at_ms,'state','blocked','reason',protocol.reason
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
            AND c.received_at_ms>$4-86400000 AND control.mode='running' AND a.state='pending_native'
            AND (a.executor_owner_user_id=$1 OR c.payload::jsonb->>'sender_mxid'=$2)
            AND protocol.reason IS NOT NULL
            AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_intake i
                WHERE i.assignment_id=a.id AND i.event_id=c.event_id)
        ORDER BY c.received_at_ms DESC,c.event_id DESC LIMIT 16
    "#).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(room)
        .bind::<BigInt,_>(now).load::<ChatStatus>(db).await?;
    for row in candidates { values.push(decode(&row.payload)?); }
    values.sort_by(|a,b| b["receivedAtMs"].as_i64().cmp(&a["receivedAtMs"].as_i64())
        .then_with(|| b["eventId"].as_str().cmp(&a["eventId"].as_str())));
    values.truncate(16);
    Ok(values)
}
