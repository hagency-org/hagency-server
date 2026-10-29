//! Finite independent delivery tickets. Recovery records evidence, never new capacity authority.
use super::*;
use crate::secret_token;
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
struct Saved {
    #[diesel(sql_type=Text)]
    payload: String,
}
async fn command(db: &mut AsyncPgConnection, id: &str) -> Result<CommandRow> {
    sql_query("SELECT * FROM hagency_agent_v1.management_agent_budget_commands WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
fn owner(w: &CommandRow, p: &Principal) -> Result<()> {
    let (d, g) = device(p)?;
    if w.owner_user_id != p.user_id || w.executor_device_id != d || g < w.executor_generation {
        return Err(Error::NotFound("agent_budget_command_not_found"));
    }
    Ok(())
}
fn checked(w: &CommandRow, server: &str) -> Result<AgentBudgetCommand> {
    let c: AgentBudgetCommand = decode(&w.payload)?;
    c.validate(server)?;
    if c.command_id != w.id
        || c.reserve_command.request_id != w.request_id
        || allocation_digest(&c)? != w.digest
    {
        return Err(Error::Conflict("agent_budget_command_changed"));
    }
    Ok(c)
}
async fn result(db: &mut AsyncPgConnection, id: &str) -> Result<Option<AgentBudgetReceipt>> {
    sql_query(
        "SELECT payload FROM hagency_agent_v1.management_agent_budget_results WHERE command_id=$1",
    )
    .bind::<Text, _>(id)
    .get_result::<Saved>(db)
    .await
    .optional()?
    .map(|r| decode(&r.payload))
    .transpose()
}
async fn cancelled(db: &mut AsyncPgConnection, id: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_cancellations WHERE command_id=$1) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched)
}
async fn current(
    db: &mut AsyncPgConnection,
    w: &CommandRow,
    c: &AgentBudgetCommand,
    now: i64,
    server: &str,
) -> Result<bool> {
    let r = row(db, &w.request_id).await?;
    let (b, original, reserved) = base(db, &r.id, server).await?;
    let cx = match fresh_base(db, &r, &b, &original, now).await {
        Ok(v) => v,
        Err(Error::Conflict(_) | Error::NotFound(_)) => return Ok(false),
        Err(e) => return Err(e),
    };
    if now >= c.expires_at_ms
        || cx.roles.authority_revision != c.authority_revision
        || cx.roles.administrator_mxid != c.approving_administrator_mxid
        || b.assignment_id != c.assignment_id
        || b.assignment_generation != c.assignment_generation
        || b.receipt_digest != c.reserve_receipt_digest
    {
        return Ok(false);
    }
    let h = head(db, &r.id, &b, &reserved, server).await?;
    if h.revision != c.expected_budget_revision
        || h.digest != c.expected_budget_digest
        || h.ceiling != c.expected_ceiling_tokens
    {
        return Ok(false);
    }
    // Version 2 binds an explicit owner-funded increase for this exact Agent.
    // Native applies it only if the complete machine/Seat liability fits.
    if c.owner_funded { return Ok(true); }
    match fits_parent(db, &r, &cx, &h, c.add_tokens, server).await {
        Ok(()) => Ok(true),
        Err(Error::Conflict(_)) => Ok(false),
        Err(e) => Err(e),
    }
}
async fn deliver(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    w: &CommandRow,
    server: &str,
    historical: bool,
) -> Result<AgentBudgetDelivery> {
    owner(w, p)?;
    let c = checked(w, server)?;
    let now = clock(db).await?;
    let saved = result(db, &w.id).await?;
    let cancel = cancelled(db, &w.id).await?;
    let live = current(db, w, &c, now, server).await?;
    let application = !historical
        && saved.is_none()
        && !cancel
        && live
        && policy.agent_budget_deltas()
        && p.device_generation == Some(w.executor_generation);
    let cleanup = cancel || !live;
    let mut until = sql_query(
        "SELECT least(valid_until_ms,$2+30000) AS now FROM hagency_agent_v1.sessions WHERE id=$1",
    )
    .bind::<Text, _>(&p.session_id)
    .bind::<BigInt, _>(now)
    .get_result::<Clock>(db)
    .await?
    .now;
    if application {
        until = until.min(c.expires_at_ms)
    }
    if until <= now {
        return Err(Error::Unauthorized("agent_budget_ticket_expired"));
    }
    // DomainStore::authorize holds the transaction-wide advisory lock, including across instances.
    sql_query("DELETE FROM hagency_agent_v1.management_agent_budget_tickets WHERE command_id=$1 AND expires_at_ms<=$2").bind::<Text,_>(&w.id).bind::<BigInt,_>(now).execute(db).await?;
    let full=sql_query("SELECT count(*)>=64 AS matched FROM hagency_agent_v1.management_agent_budget_tickets WHERE command_id=$1").bind::<Text,_>(&w.id).get_result::<Flag>(db).await?.matched;
    if full {
        return Err(Error::Unavailable("agent_budget_ticket_capacity"));
    }
    let token = secret_token();
    let purpose = if application {
        "apply"
    } else if cleanup {
        "cancel"
    } else {
        "observe"
    };
    sql_query(
        "INSERT INTO hagency_agent_v1.management_agent_budget_tickets VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind::<Text, _>(&w.id)
    .bind::<Text, _>(hash(&token))
    .bind::<Text, _>(&w.executor_device_id)
    .bind::<BigInt, _>(p.device_generation.unwrap())
    .bind::<Text, _>(purpose)
    .bind::<BigInt, _>(now)
    .bind::<BigInt, _>(until)
    .execute(db)
    .await?;
    if application {
        sql_query("INSERT INTO hagency_agent_v1.management_agent_budget_issuance VALUES($1,$2,$3,$4) ON CONFLICT(command_id) DO NOTHING").bind::<Text,_>(&w.id).bind::<BigInt,_>(w.executor_generation).bind::<BigInt,_>(now).bind::<BigInt,_>(until).execute(db).await?;
    }
    Ok(AgentBudgetDelivery {
        command: c,
        command_digest: w.digest.clone(),
        token,
        lease_until_ms: until,
        application_allowed: application,
        cancellation_requested: cleanup,
        recorded: saved,
    })
}
async fn validate_ledger(
    db: &mut AsyncPgConnection,
    w: &CommandRow,
    r: &AgentBudgetReceipt,
) -> Result<()> {
    super::super::executor::validate_external_ledger_receipt(
        db,
        &w.owner_user_id,
        &w.executor_device_id,
        r.ledger_revision,
        r.ledger_revision,
    )
    .await?;
    let collision=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_executor_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3 UNION ALL SELECT 1 FROM hagency_agent_v1.management_parent_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND ((receipt::jsonb->>'ledgerRevision')::bigint=$3 OR (receipt::jsonb->>'fenceRevision')::bigint=$3) UNION ALL SELECT 1 FROM hagency_agent_v1.management_resource_grants WHERE owner_user_id=$1 AND executor_device_id=$2 AND (receipt::jsonb->>'ledgerRevision')::bigint=$3) AS matched").bind::<Text,_>(&w.owner_user_id).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(r.ledger_revision).get_result::<Flag>(db).await?.matched;
    if collision {
        return Err(Error::Conflict("agent_budget_ledger_revision_changed"));
    }
    Ok(())
}
impl DomainStore {
    pub async fn poll_agent_budget(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<AgentBudgetDelivery>> {
        let (device, _) = device(p)?;
        if let Some(id) = &input.after {
            key(id)?
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;
 let w=sql_query("SELECT c.* FROM hagency_agent_v1.management_agent_budget_commands c LEFT JOIN hagency_agent_v1.management_agent_budget_results r ON r.command_id=c.id WHERE c.owner_user_id=$1 AND c.executor_device_id=$2 AND r.command_id IS NULL ORDER BY (c.id<=$3),c.id LIMIT 1").bind::<Text,_>(&p.user_id).bind::<Text,_>(device).bind::<Text,_>(input.after.as_deref().unwrap_or("")).get_result::<CommandRow>(db).await.optional()?;
 match w{Some(w)=>Ok(Some(deliver(db,p,policy,&w,&self.server,false).await?)),None=>Ok(None)}}).await
    }
    pub async fn recover_agent_budget(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ExecutorRecoverInput,
    ) -> Result<AgentBudgetDelivery> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let w = command(db, id).await?;
                owner(&w, p)?;
                if w.digest != input.command_digest {
                    return Err(Error::Conflict("agent_budget_command_changed"));
                }
                deliver(db, p, policy, &w, &self.server, true).await
            })
            .await
    }
    pub async fn record_agent_budget(
        &self,
        p: &Principal,
        id: &str,
        input: AgentBudgetResult,
    ) -> Result<AgentBudgetReceipt> {
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let w=command(db,id).await?;owner(&w,p)?;let c=checked(&w,&self.server)?;
 if input.command_digest!=w.digest{return Err(Error::Conflict("agent_budget_command_changed"))}input.receipt.validate_for(&c)?;
 // First immutable receipt wins, independently of subsequent ticket expiry or disabled issuance.
 if let Some(old)=result(db,id).await?{if allocation_digest(&old)?!=allocation_digest(&input.receipt)?{return Err(Error::Conflict("agent_budget_result_changed"))}return Ok(old)}
 let ticket=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_tickets WHERE command_id=$1 AND token_hash=$2 AND executor_device_id=$3 AND executor_generation=$4 AND expires_at_ms>$5) AS matched").bind::<Text,_>(id).bind::<Text,_>(hash(&input.token)).bind::<Text,_>(&w.executor_device_id).bind::<BigInt,_>(p.device_generation.unwrap()).bind::<BigInt,_>(now).get_result::<Flag>(db).await?.matched;if !ticket{return Err(Error::Conflict("agent_budget_ticket_changed"))}
 if input.receipt.state=="applied"{let issued=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_agent_budget_issuance WHERE command_id=$1) AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;if !issued{return Err(Error::Conflict("agent_budget_not_issued"))}}
 let (b,_,reserved)=base(db,&w.request_id,&self.server).await?;let h=head(db,&w.request_id,&b,&reserved,&self.server).await?;
 if c.expected_budget_revision!=h.revision||c.expected_budget_digest!=h.digest||c.expected_ceiling_tokens!=h.ceiling||input.receipt.ledger_revision<=h.ledger{return Err(Error::Conflict("agent_budget_changed"))}
 validate_ledger(db,&w,&input.receipt).await?;
 sql_query("INSERT INTO hagency_agent_v1.management_agent_budget_results VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(id).bind::<Text,_>(wire(&input.receipt)?).bind::<Text,_>(allocation_digest(&input.receipt)?).bind::<BigInt,_>(input.receipt.ledger_revision).bind::<Text,_>(&input.receipt.state).bind::<BigInt,_>(input.receipt.budget_revision).execute(db).await?;
 Self::audit(db,p,"agent.budget.result",id,now).await?;Ok(input.receipt)}).await
    }
}
