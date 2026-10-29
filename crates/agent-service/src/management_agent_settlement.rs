//! Separate immutable post-fence observations; no new resource or work authority.
use super::*;
use crate::{
    management_agent_executor_types::{
        AGENT_FENCE_CAPABILITY, ManagedAgentFenceCommand, ManagedAgentFenceReceipt,
    },
    management_agent_settlement_types::*,
    management_allocation_types::{ExecutorPollInput, ExecutorRecoverInput, valid_digest},
    secret_token,
};

#[derive(QueryableByName)]
struct FenceRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type=Text)]
    receipt: String,
    #[diesel(sql_type=Text)]
    receipt_digest: String,
}
async fn fence(
    db: &mut AsyncPgConnection,
    id: &str,
    server: &str,
) -> Result<Option<(FenceRow, ManagedAgentFenceCommand, ManagedAgentFenceReceipt)>> {
    let f=sql_query("SELECT w.id,i.executor_owner_user_id AS owner_user_id,i.executor_device_id,i.executor_generation,i.payload,w.created_at_ms,w.receipt,w.receipt_digest FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND i.state='fenced' AND w.operation='agent.fence' AND w.receipt IS NOT NULL").bind::<Text,_>(id).get_result::<FenceRow>(db).await.optional()?;
    let Some(f) = f else { return Ok(None) };
    let original: ManagedAgentReserveCommand = decode(&f.payload)?;
    let c = ManagedAgentFenceCommand {
        version: 1,
        capability: AGENT_FENCE_CAPABILITY.into(),
        operation: "agent.fence".into(),
        command_id: f.id.clone(),
        reserve_command_digest: allocation_digest(&original)?,
        reserve_command: original,
        issued_at_ms: f.created_at_ms,
    };
    c.validate(server)?;
    let r: ManagedAgentFenceReceipt = decode(&f.receipt)?;
    r.validate_for(&c)?;
    if c.reserve_command.request_id != id || allocation_digest(&r)? != f.receipt_digest {
        return Err(Error::Conflict("agent_settlement_fence_changed"));
    }
    Ok(Some((f, c, r)))
}
#[derive(QueryableByName)]
struct CommandRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    request_id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
}
#[derive(QueryableByName)]
struct Json {
    #[diesel(sql_type=Text)]
    payload: String,
}
#[derive(QueryableByName)]
struct Key {
    #[diesel(sql_type=Text)]
    id: String,
}
async fn command(db: &mut AsyncPgConnection, id: &str) -> Result<CommandRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_settlement_commands WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
fn checked(w: &CommandRow, server: &str) -> Result<ManagedAgentSettlementCommand> {
    let c: ManagedAgentSettlementCommand = decode(&w.payload)?;
    c.validate(server)?;
    if c.command_id != w.id
        || c.fence_command.reserve_command.request_id != w.request_id
        || allocation_digest(&c)? != w.digest
    {
        return Err(Error::Conflict("agent_settlement_command_changed"));
    }
    Ok(c)
}
fn owner(w: &CommandRow, p: &Principal) -> Result<()> {
    let (d, g) = device(p)?;
    if w.owner_user_id != p.user_id || w.executor_device_id != d || g < w.executor_generation {
        return Err(Error::NotFound("agent_settlement_not_found"));
    }
    Ok(())
}
async fn saved(
    db: &mut AsyncPgConnection,
    id: &str,
) -> Result<Option<ManagedAgentSettlementReceipt>> {
    sql_query("SELECT payload FROM hagency_agent_v1.management_agent_settlement_results WHERE command_id=$1").bind::<Text,_>(id).get_result::<Json>(db).await.optional()?.map(|r|decode(&r.payload)).transpose()
}
async fn pending(db: &mut AsyncPgConnection, id: &str) -> Result<Option<String>> {
    Ok(sql_query("SELECT c.id FROM hagency_agent_v1.management_agent_settlement_commands c WHERE c.request_id=$1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_results r WHERE r.command_id=c.id) ORDER BY c.id LIMIT 1").bind::<Text,_>(id).get_result::<Key>(db).await.optional()?.map(|v|v.id))
}
async fn latest(
    db: &mut AsyncPgConnection,
    id: &str,
    server: &str,
) -> Result<Option<ManagedAgentSettlementReceipt>> {
    let w=sql_query("SELECT c.* FROM hagency_agent_v1.management_agent_settlement_commands c JOIN hagency_agent_v1.management_agent_settlement_results r ON r.command_id=c.id WHERE c.request_id=$1 ORDER BY r.ledger_revision DESC LIMIT 1").bind::<Text,_>(id).get_result::<CommandRow>(db).await.optional()?;
    let Some(w) = w else { return Ok(None) };
    let c = checked(&w, server)?;
    let r = saved(db, &w.id)
        .await?
        .ok_or(Error::Conflict("agent_settlement_result_missing"))?;
    r.validate_for(&c)?;
    Ok(Some(r))
}
async fn highwater(db: &mut AsyncPgConnection, id: &str) -> Result<i64> {
    Ok(sql_query("SELECT coalesce(max(ledger_revision),0)::bigint AS now FROM hagency_agent_v1.management_agent_settlement_evidence WHERE request_id=$1").bind::<Text,_>(id).get_result::<Clock>(db).await?.now)
}
async fn visible(db: &mut AsyncPgConnection, r: &Row, p: &Principal) -> Result<()> {
    super::visible(db,r,p).await?;
    Ok(())
}
/// Returns only whether the latest proof can lower a current central balance.
/// All original commands remain intact, and late evidence can make this false.
async fn current(
    db: &mut AsyncPgConnection,
    id: &str,
    r: &ManagedAgentSettlementReceipt,
    known: i64,
    server: &str,
) -> Result<bool> {
    if !r.usage_complete
        || known > r.known_consumed_tokens
        || highwater(db, id).await? > r.ledger_revision
    {
        return Ok(false);
    }
    if r.agent_reservation_id.is_none() {
        return Ok(true);
    }
    let Some(h) = budget::settlement_head(db, id, server).await? else {
        return Ok(false);
    };
    Ok(!h.pending
        && r.budget_revision == Some(h.revision)
        && r.budget_digest.as_deref() == Some(&h.digest)
        && r.effective_ceiling_tokens == Some(h.ceiling))
}
async fn enqueue(
    db: &mut AsyncPgConnection,
    id: &str,
    server: &str,
) -> Result<AgentSettlementRequested> {
    if let Some(id) = pending(db, id).await? {
        let w = command(db, &id).await?;
        return Ok(AgentSettlementRequested {
            request_id: w.request_id,
            command_id: w.id,
            command_digest: w.digest,
        });
    }
    let (f, fc, _) = fence(db, id, server)
        .await?
        .ok_or(Error::Conflict("agent_fence_receipt_required"))?;
    let full=sql_query("SELECT count(*)>=100000 AS matched FROM hagency_agent_v1.management_agent_settlement_commands WHERE owner_user_id=$1").bind::<Text,_>(&f.owner_user_id).get_result::<Flag>(db).await?.matched;
    if full {
        return Err(Error::Unavailable("agent_settlement_capacity"));
    }
    let now = clock(db).await?;
    let c = ManagedAgentSettlementCommand {
        version: 1,
        capability: AGENT_SETTLEMENT_CAPABILITY.into(),
        operation: "agent.settle".into(),
        command_id: format!("mst_{}", entity_id()?),
        fence_command: fc,
        fence_receipt_digest: f.receipt_digest,
        issued_at_ms: now,
    };
    c.validate(server)?;
    let digest = allocation_digest(&c)?;
    sql_query("INSERT INTO hagency_agent_v1.management_agent_settlement_commands VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind::<Text,_>(&c.command_id).bind::<Text,_>(id).bind::<Text,_>(&f.id).bind::<Text,_>(&f.owner_user_id).bind::<Text,_>(&f.executor_device_id).bind::<BigInt,_>(f.executor_generation).bind::<Text,_>(wire(&c)?).bind::<Text,_>(&digest).bind::<BigInt,_>(now).execute(db).await?;
    Ok(AgentSettlementRequested {
        request_id: id.into(),
        command_id: c.command_id,
        command_digest: digest,
    })
}
/// Called only when recording a new fence result. Historical receipt replay and
/// schema migration never enroll old agents into automatic settlement.
pub(super) async fn enqueue_initial(
    db: &mut AsyncPgConnection,
    id: &str,
    server: &str,
) -> Result<()> {
    enqueue(db, id, server).await?;
    Ok(())
}
async fn progress(
    db: &mut AsyncPgConnection,
    r: &Row,
    server: &str,
) -> Result<AgentSettlementProgress> {
    let f = fence(db, &r.id, server).await?;
    let last = latest(db, &r.id, server).await?;
    let base=sql_query("SELECT w.receipt AS payload FROM hagency_agent_v1.management_agent_executor_commands w JOIN hagency_agent_v1.management_agent_intents i ON i.id=w.intent_id WHERE i.request_id=$1 AND w.operation='agent.reserve' AND w.receipt IS NOT NULL").bind::<Text,_>(&r.id).get_result::<Json>(db).await.optional()?.map(|r|decode::<ManagedAgentReservationReceipt>(&r.payload)).transpose()?;
    let known = usage::known(db, usage::Scope::Agent(&r.id)).await?;
    let prior=sql_query("SELECT coalesce(max((s.payload::jsonb->>'knownConsumedTokens')::bigint),0)::bigint AS now FROM hagency_agent_v1.management_agent_settlement_results s JOIN hagency_agent_v1.management_agent_settlement_commands c ON c.id=s.command_id WHERE c.request_id=$1").bind::<Text,_>(&r.id).get_result::<Clock>(db).await?.now;
    let prior_hold=sql_query("SELECT coalesce(max((s.payload::jsonb->>'retainedTokens')::bigint),0)::bigint AS now FROM hagency_agent_v1.management_agent_settlement_results s JOIN hagency_agent_v1.management_agent_settlement_commands c ON c.id=s.command_id WHERE c.request_id=$1").bind::<Text,_>(&r.id).get_result::<Clock>(db).await?.now;
    let known = known
        .max(prior)
        .max(f.as_ref().map_or(0, |(_, _, r)| r.known_consumed_tokens));
    let valid = match &last {
        Some(v) => current(db, &r.id, v, known, server).await?,
        None => false,
    };
    let mut floor = f.as_ref().and_then(|(_, _, r)| r.retained_tokens);
    if f.as_ref().is_none_or(|(_, _, r)| !r.usage_complete) {
        floor = floor
            .into_iter()
            .chain(
                base.as_ref()
                    .filter(|r| r.state == "reserved")
                    .map(|r| r.retained_tokens),
            )
            .chain(budget::cleanup_commitment(db, &r.id, server).await?)
            .max();
    }
    floor = floor
        .into_iter()
        .chain((prior_hold > 0).then_some(prior_hold))
        .max();
    if valid {
        floor = last.as_ref().map(|r| r.retained_tokens)
    }
    floor = floor
        .into_iter()
        .chain((f.is_some() || last.is_some() || known > 0).then_some(known))
        .max();
    let waiting = pending(db, &r.id).await?;
    let reason = if f.is_none() {
        Some("native_fence_pending")
    } else if waiting.is_some() {
        Some("settlement_pending")
    } else if valid {
        Some("settlement_current")
    } else {
        None
    };
    Ok(AgentSettlementProgress {
        request_id: r.id.clone(),
        request_revision: r.revision,
        fence_receipt_digest: f.map(|(f, _, _)| f.receipt_digest),
        pending_command_id: waiting,
        latest_receipt: last,
        settlement_current: valid,
        can_request: reason.is_none(),
        blocking_reason: reason.map(str::to_owned),
        known_consumed_tokens: floor.map(|_| known),
        retained_tokens_floor: floor,
    })
}
async fn deliver(
    db: &mut AsyncPgConnection,
    p: &Principal,
    w: &CommandRow,
    server: &str,
    historical: bool,
) -> Result<AgentSettlementDelivery> {
    owner(w, p)?;
    let c = checked(w, server)?;
    let recorded = saved(db, &w.id).await?;
    let now = clock(db).await?;
    let until = sql_query(
        "SELECT least(valid_until_ms,$2+30000) AS now FROM hagency_agent_v1.sessions WHERE id=$1",
    )
    .bind::<Text, _>(&p.session_id)
    .bind::<BigInt, _>(now)
    .get_result::<Clock>(db)
    .await?
    .now;
    if until <= now {
        return Err(Error::Unauthorized("agent_settlement_ticket_expired"));
    }
    sql_query("DELETE FROM hagency_agent_v1.management_agent_settlement_tickets WHERE command_id=$1 AND expires_at_ms<=$2").bind::<Text,_>(&w.id).bind::<BigInt,_>(now).execute(db).await?;
    if sql_query("SELECT count(*)>=64 AS matched FROM hagency_agent_v1.management_agent_settlement_tickets WHERE command_id=$1").bind::<Text,_>(&w.id).get_result::<Flag>(db).await?.matched{return Err(Error::Unavailable("agent_settlement_ticket_capacity"))}
    let token = secret_token();
    let executable = !historical && recorded.is_none();
    sql_query("INSERT INTO hagency_agent_v1.management_agent_settlement_tickets VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&w.id).bind::<Text,_>(hash(&token)).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(p.device_generation.unwrap()).bind::<BigInt,_>(now).bind::<BigInt,_>(until).execute(db).await?;
    if executable {
        sql_query("INSERT INTO hagency_agent_v1.management_agent_settlement_issuance VALUES($1,$2) ON CONFLICT(command_id) DO NOTHING").bind::<Text,_>(&w.id).bind::<BigInt,_>(now).execute(db).await?;
    }
    DomainStore::authorize(db, p, crate::api::now_ms()).await?;
    Ok(AgentSettlementDelivery {
        command: c,
        command_digest: w.digest.clone(),
        token,
        lease_until_ms: until,
        execution_allowed: executable,
        recorded,
    })
}
impl DomainStore {
    pub async fn managed_agent_settlement(
        &self,
        p: &Principal,
        id: &str,
    ) -> Result<AgentSettlementProgress> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let r = row(db, id).await?;
                visible(db, &r, p).await?;
                progress(db, &r, &self.server).await
            })
            .await
    }
    pub async fn request_agent_settlement(
        &self,
        p: &Principal,
        id: &str,
        input: RequestAgentSettlement,
    ) -> Result<AgentSettlementRequested> {
        key(id)?;
        input.validate()?;
        device(p)?;
        let digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let r = row(db, id).await?;
                visible(db, &r, p).await?;
                if let Some(old) = replay(
                    db,
                    p,
                    &input.command_id,
                    "agent.settlement.request",
                    &digest,
                )
                .await?
                {
                    return Ok(old);
                }
                let status = progress(db, &r, &self.server).await?;
                if r.revision != input.expected_request_revision
                    || status.fence_receipt_digest.as_deref()
                        != Some(&input.expected_fence_receipt_digest)
                {
                    return Err(Error::Conflict("agent_settlement_changed"));
                }
                if !status.can_request {
                    return Err(Error::Conflict("agent_settlement_not_ready"));
                }
                let result = enqueue(db, id, &self.server).await?;
                receipt(
                    db,
                    p,
                    &input.command_id,
                    "agent.settlement.request",
                    &digest,
                    &result,
                )
                .await?;
                let now = clock(db).await?;
                Self::audit(db, p, "agent.settlement.request", id, now).await?;
                Ok(result)
            })
            .await
    }
    pub async fn poll_agent_settlement(
        &self,
        p: &Principal,
        input: ExecutorPollInput,
    ) -> Result<Option<AgentSettlementDelivery>> {
        let (dev, _) = device(p)?;
        if let Some(id) = &input.after {
            key(id)?
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;
            // Only agents already explicitly enrolled by a new fence/request are
            // candidates. New source evidence drives one new observation, not time.
            let candidate=sql_query("SELECT c.request_id AS id FROM hagency_agent_v1.management_agent_settlement_commands c JOIN hagency_agent_v1.management_agent_settlement_results r ON r.command_id=c.id JOIN hagency_agent_v1.management_agent_settlement_evidence e ON e.request_id=c.request_id WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND c.executor_generation<=$3 AND e.ledger_revision>r.ledger_revision AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_commands nc JOIN hagency_agent_v1.management_agent_settlement_results nr ON nr.command_id=nc.id WHERE nc.request_id=c.request_id AND nr.ledger_revision>r.ledger_revision) AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_commands nc WHERE nc.request_id=c.request_id AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_results nr WHERE nr.command_id=nc.id)) ORDER BY c.id LIMIT 1").bind::<Text,_>(&p.user_id).bind::<Text,_>(dev).bind::<BigInt,_>(p.device_generation.unwrap()).get_result::<Key>(db).await.optional()?;
            if let Some(c)=candidate{enqueue(db,&c.id,&self.server).await?;}
            let w=sql_query("SELECT c.* FROM hagency_agent_v1.management_agent_settlement_commands c WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND c.executor_generation<=$3 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_results r WHERE r.command_id=c.id) ORDER BY (c.id<=$4),c.id LIMIT 1").bind::<Text,_>(&p.user_id).bind::<Text,_>(dev).bind::<BigInt,_>(p.device_generation.unwrap()).bind::<Text,_>(input.after.as_deref().unwrap_or("")).get_result::<CommandRow>(db).await.optional()?;
            match w{Some(w)=>Ok(Some(deliver(db,p,&w,&self.server,false).await?)),None=>Ok(None)}
        }).await
    }
    pub async fn recover_agent_settlement(
        &self,
        p: &Principal,
        id: &str,
        input: ExecutorRecoverInput,
    ) -> Result<AgentSettlementDelivery> {
        key(id)?;
        if !valid_digest(&input.command_digest) {
            return Err(Error::Invalid("invalid_agent_settlement_digest"));
        }
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let w = command(db, id).await?;
                owner(&w, p)?;
                if w.digest != input.command_digest {
                    return Err(Error::Conflict("agent_settlement_command_changed"));
                }
                deliver(db, p, &w, &self.server, true).await
            })
            .await
    }
    pub async fn record_agent_settlement(
        &self,
        p: &Principal,
        id: &str,
        input: AgentSettlementResult,
    ) -> Result<ManagedAgentSettlementReceipt> {
        key(id)?;
        if input.token.len() > 512 || !valid_digest(&input.command_digest) {
            return Err(Error::Invalid("invalid_agent_settlement_result"));
        }
        wire(&input.receipt)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let w=command(db,id).await?;owner(&w,p)?;let c=checked(&w,&self.server)?;
            if w.digest!=input.command_digest{return Err(Error::Conflict("agent_settlement_command_changed"))}input.receipt.validate_for(&c)?;
            if let Some(old)=saved(db,id).await?{if allocation_digest(&old)?!=allocation_digest(&input.receipt)?{return Err(Error::Conflict("agent_settlement_result_changed"))}return Ok(old)}
            let now=clock(db).await?;
            if !sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_tickets WHERE command_id=$1 AND token_hash=$2 AND executor_device_id=$3 AND executor_generation=$4 AND expires_at_ms>$5) AS matched").bind::<Text,_>(id).bind::<Text,_>(hash(&input.token)).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(p.device_generation.unwrap()).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched{return Err(Error::Unauthorized("agent_settlement_ticket_required"))}
            if !sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_settlement_issuance WHERE command_id=$1) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched{return Err(Error::Conflict("agent_settlement_not_issued"))}
            let (f,fc,fr)=fence(db,&w.request_id,&self.server).await?.ok_or(Error::Conflict("agent_fence_receipt_required"))?;
            let r=&input.receipt;
            if allocation_digest(&fc)?!=allocation_digest(&c.fence_command)? || f.receipt_digest!=c.fence_receipt_digest || r.fence_revision!=fr.fence_revision || r.agent_reservation_id!=fr.agent_reservation_id || r.ledger_revision<=fr.ledger_revision || r.known_consumed_tokens<fr.known_consumed_tokens{return Err(Error::Conflict("agent_settlement_fence_changed"))}
            executor::validate_external_ledger_receipt(db,&w.owner_user_id,&w.executor_device_id,r.ledger_revision,r.ledger_revision).await?;
            let collision=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3 UNION ALL SELECT 1 FROM hagency_agent_v1.management_parent_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND ((receipt::jsonb->>'ledgerRevision')::bigint=$3 OR (receipt::jsonb->>'fenceRevision')::bigint=$3) UNION ALL SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3) AS matched").bind::<Text,_>(&w.owner_user_id).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(r.ledger_revision).get_result::<Flag>(db).await?.matched;
            if collision{return Err(Error::Conflict("agent_settlement_ledger_changed"))}
            sql_query("INSERT INTO hagency_agent_v1.management_agent_settlement_results VALUES($1,$2,$3,$4)").bind::<Text,_>(id).bind::<Text,_>(wire(r)?).bind::<Text,_>(allocation_digest(r)?).bind::<BigInt,_>(r.ledger_revision).execute(db).await?;
            Self::audit(db,p,"agent.settlement.result",id,now).await?;Ok(input.receipt)
        }).await
    }
}

/// Shared balance projection for allocation/admission accounting. Uses the
/// caller's already checked budget head, so it cannot recurse into budget reads.
pub(super) struct SettlementBalance {
    pub known: i64,
    pub retained: i64,
    pub complete: bool,
}
pub(super) async fn retained_for_head(
    db: &mut AsyncPgConnection,
    id: &str,
    revision: i64,
    digest: &str,
    ceiling: i64,
    known: i64,
    server: &str,
) -> Result<Option<SettlementBalance>> {
    let Some(r) = latest(db, id, server).await? else {
        return Ok(None);
    };
    #[derive(QueryableByName)]
    struct Floor {
        #[diesel(sql_type=BigInt)]
        known: i64,
        #[diesel(sql_type=BigInt)]
        held: i64,
    }
    let floor=sql_query("SELECT coalesce(max((x.payload::jsonb->>'knownConsumedTokens')::bigint),0)::bigint AS known,coalesce(max((x.payload::jsonb->>'retainedTokens')::bigint),0)::bigint AS held FROM hagency_agent_v1.management_agent_settlement_results x JOIN hagency_agent_v1.management_agent_settlement_commands c ON c.id=x.command_id WHERE c.request_id=$1").bind::<Text,_>(id).get_result::<Floor>(db).await?;
    let known = known.max(floor.known);
    let pending=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_commands c WHERE c.request_id=$1 AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_results x WHERE x.command_id=c.id)) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
    let complete = r.usage_complete
        && !pending
        && r.budget_revision == Some(revision)
        && r.budget_digest.as_deref() == Some(digest)
        && r.effective_ceiling_tokens == Some(ceiling)
        && r.known_consumed_tokens >= known
        && highwater(db, id).await? <= r.ledger_revision;
    Ok(Some(SettlementBalance {
        known,
        retained: if complete {
            r.retained_tokens
        } else {
            floor.held
        },
        complete,
    }))
}
pub(super) async fn cleanup_balance(
    db: &mut AsyncPgConnection,
    id: &str,
    server: &str,
) -> Result<Option<(i64, i64, bool)>> {
    if latest(db, id, server).await?.is_none() {
        return Ok(None);
    }
    let r = row(db, id).await?;
    let value = progress(db, &r, server).await?;
    Ok(value.retained_tokens_floor.map(|floor| {
        (
            value.known_consumed_tokens.unwrap_or(0),
            floor,
            !value.settlement_current,
        )
    }))
}
