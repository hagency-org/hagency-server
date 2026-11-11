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
    let Some(room) = room else {
        return Ok(Vec::new());
    };
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
                WHEN w.state IS NOT NULL THEN w.state
                ELSE i.state END,
            'reason', i.reason,
            'budgetBlock', budget_block.payload::jsonb
        )::text AS payload
        FROM hagency_agent_v1.management_shared_intake i
        JOIN hagency_agent_v1.management_shared_assignments a ON a.id=i.assignment_id
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
    rows.into_iter()
        .map(|r| {
            let mut value: Value = decode(&r.payload)?;
            let block = value
                .as_object_mut()
                .and_then(|v| v.remove("budgetBlock"))
                .unwrap_or(Value::Null);
            // Publish only the reason, never the private liability digest or
            // another message's details. A zero balance is not proof of spending.
            let budget = &block["budget"];
            let state = if budget["usageComplete"] == false
                && budget["knownConsumedTokens"]
                    .as_i64()
                    .zip(budget["allocatedTokens"].as_i64())
                    .is_some_and(|(known, total)| known < total)
            {
                Some("usage_unresolved")
            } else if budget["usageComplete"] == true
                || block == "known_consumption_exhausted"
                || block["reason"] == "known_consumption_exhausted"
            {
                Some("exhausted")
            } else {
                None
            };
            if let Some(state) = state {
                value["budgetState"] = json!(state);
            }
            Ok(value)
        })
        .collect()
}
