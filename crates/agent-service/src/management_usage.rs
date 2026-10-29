//! Known shared-call consumption is a floor, never proof that native liability is complete.
use super::*;
use crate::management_allocation_types::MAX_EXACT;

pub(super) enum Scope<'a> {
    Agent(&'a str),
    Allocation(&'a str),
    Grant(&'a str),
}
#[derive(QueryableByName)]
struct Amount {
    #[diesel(sql_type=BigInt)]
    amount: i64,
}
pub(super) async fn known(db: &mut AsyncPgConnection, scope: Scope<'_>) -> Result<i64> {
    let (column, id) = match scope {
        Scope::Agent(id) => ("q.id", id),
        Scope::Allocation(id) => ("q.allocation_id", id),
        Scope::Grant(id) => ("p.grant_id", id),
    };
    // SUM(bigint) is numeric in PostgreSQL. Cap before narrowing; overflow must
    // retain uncertainty rather than wrap a real liability into a lower value.
    // Per-agent MAX avoids adding a later native settlement on top of the same
    // Matrix work. Aggregate only after including immutable fence/settlement
    // evidence, including agents whose old work receipt has not arrived yet.
    let amount=sql_query(format!("SELECT LEAST(COALESCE(SUM(GREATEST(COALESCE((SELECT SUM(w.known_consumed_tokens) FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id JOIN hagency_agent_v1.management_shared_assignments a ON a.id=r.assignment_id WHERE a.request_id=q.id),0),COALESCE((SELECT MAX((s.payload::jsonb->>'knownConsumedTokens')::bigint) FROM hagency_agent_v1.management_agent_settlement_results s JOIN hagency_agent_v1.management_agent_settlement_commands c ON c.id=s.command_id WHERE c.request_id=q.id),0),COALESCE((SELECT MAX((f.receipt::jsonb->>'knownConsumedTokens')::bigint) FROM hagency_agent_v1.management_agent_executor_commands f JOIN hagency_agent_v1.management_agent_intents i ON i.id=f.intent_id WHERE i.request_id=q.id AND f.operation='agent.fence' AND f.receipt IS NOT NULL),0))),0),9007199254740992)::bigint AS amount FROM hagency_agent_v1.management_agent_requests q JOIN hagency_agent_v1.management_allocations p ON p.id=q.allocation_id WHERE {column}=$1"))
        .bind::<Text,_>(id).get_result::<Amount>(db).await?.amount;
    if !(0..=MAX_EXACT).contains(&amount) {
        return Err(Error::Unavailable("shared_usage_overflow"));
    }
    Ok(amount)
}
