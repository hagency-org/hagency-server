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
                WHEN w.state IS NOT NULL THEN w.state
                ELSE i.state END,
            'reason', i.reason
        )::text AS payload
        FROM hagency_agent_v1.management_shared_intake i
        JOIN hagency_agent_v1.management_shared_assignments a ON a.id=i.assignment_id
        LEFT JOIN hagency_agent_v1.management_shared_event_pins pin
            ON pin.assignment_id=i.assignment_id AND pin.event_id=i.event_id
        LEFT JOIN hagency_agent_v1.management_shared_work w ON w.id=pin.work_id
        LEFT JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id
        LEFT JOIN hagency_agent_v1.management_shared_replies reply ON reply.work_id=w.id
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
    rows.into_iter().map(|r| decode(&r.payload)).collect()
}
