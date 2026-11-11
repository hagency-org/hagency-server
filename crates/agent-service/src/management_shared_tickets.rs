//! Independent, bounded upload tickets; never fresh process authority.
use super::*;

const MAX_LIVE_TICKETS: i64 = 64;
#[derive(QueryableByName)]
struct Ticket {
    #[diesel(sql_type=Text)]
    mode: String,
}

pub(super) async fn issue(
    db: &mut AsyncPgConnection,
    p: &Principal,
    runtime_id: &str,
    token: &str,
    mode: &str,
    expires: i64,
    now: i64,
) -> Result<()> {
    let (device, generation) = device(p)?;
    // These are disposable expired capabilities, not retained process or usage
    // evidence. Never delete a live ticket to make room for a new observer.
    sql_query("DELETE FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1 AND expires_at_ms<=$2")
        .bind::<Text,_>(runtime_id).bind::<BigInt,_>(now).execute(db).await?;
    let full=sql_query("SELECT count(*)>=$2 AS matched FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1")
        .bind::<Text,_>(runtime_id).bind::<BigInt,_>(MAX_LIVE_TICKETS).get_result::<Flag>(db).await?.matched;
    if full {
        return Err(Error::Unavailable("shared_result_ticket_capacity"));
    }
    sql_query("INSERT INTO hagency_agent_v1.management_shared_result_tickets(runtime_id,token_hash,executor_device_id,executor_generation,mode,issued_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind::<Text,_>(runtime_id).bind::<Text,_>(hash(token)).bind::<Text,_>(device)
        .bind::<BigInt,_>(generation).bind::<Text,_>(mode).bind::<BigInt,_>(now).bind::<BigInt,_>(expires).execute(db).await?;
    Ok(())
}

pub(super) async fn mode(
    db: &mut AsyncPgConnection,
    p: &Principal,
    runtime_id: &str,
    token: &str,
    now: i64,
) -> Result<String> {
    let (device, generation) = device(p)?;
    sql_query("SELECT mode FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1 AND token_hash=$2 AND executor_device_id=$3 AND executor_generation=$4 AND expires_at_ms>$5")
        .bind::<Text,_>(runtime_id).bind::<Text,_>(hash(token)).bind::<Text,_>(device)
        .bind::<BigInt,_>(generation).bind::<BigInt,_>(now).get_result::<Ticket>(db).await.optional()?
        .map(|v|v.mode).ok_or(Error::Conflict("shared_result_ticket_changed"))
}

pub(super) async fn has_execute(
    db: &mut AsyncPgConnection,
    p: &Principal,
    runtime_id: &str,
    now: i64,
) -> Result<bool> {
    let (device, generation) = device(p)?;
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_result_tickets WHERE runtime_id=$1 AND executor_device_id=$2 AND executor_generation=$3 AND mode='execute' AND expires_at_ms>$4) AS matched")
        .bind::<Text,_>(runtime_id).bind::<Text,_>(device).bind::<BigInt,_>(generation).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched)
}
