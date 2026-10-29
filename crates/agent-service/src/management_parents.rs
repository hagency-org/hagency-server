//! Parent close/settlement is reducing authority, never an allocation approval.
//! Central revocation and missing receipts retain the last known native hold.
use super::*;
use crate::management_allocation_types::{
    ExecutorPollInput, MAX_EXACT, NativeGrantReceipt, allocation_digest, valid_digest,
};
use crate::management_parent_types::*;

#[derive(QueryableByName)]
struct Grant {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    contribution_id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    receipt: String,
    #[diesel(sql_type=Text)]
    receipt_digest: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    revision: i64,
}
#[derive(QueryableByName)]
struct Parent {
    #[diesel(sql_type=BigInt)]
    revision: i64,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Text)]
    latest_command_id: String,
    #[diesel(sql_type=Nullable<Text>)]
    receipt: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    receipt_digest: Option<String>,
}
#[derive(QueryableByName)]
struct Work {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    grant_id: String,
    #[diesel(sql_type=Text)]
    owner_user_id: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    command: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<Text>)]
    lease_hash: Option<String>,
    #[diesel(sql_type=BigInt)]
    lease_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    receipt: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    receipt_digest: Option<String>,
}
fn enabled(policy: &ManagementPolicy) -> Result<()> {
    if policy.parent_settlement() {
        Ok(())
    } else {
        Err(Error::Unavailable("parent_settlement_unavailable"))
    }
}
async fn grant(db: &mut AsyncPgConnection, id: &str) -> Result<Grant> {
    sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
async fn parent(db: &mut AsyncPgConnection, id: &str) -> Result<Option<Parent>> {
    Ok(
        sql_query("SELECT * FROM hagency_agent_v1.management_parent_settlements WHERE grant_id=$1")
            .bind::<Text, _>(id)
            .get_result(db)
            .await
            .optional()?,
    )
}
async fn work(db: &mut AsyncPgConnection, id: &str) -> Result<Work> {
    sql_query("SELECT * FROM hagency_agent_v1.management_parent_commands WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
fn executor(w: &Work, p: &Principal) -> Result<()> {
    let (device, generation) = device(p)?;
    if w.owner_user_id != p.user_id
        || w.executor_device_id != device
        || generation < w.executor_generation
    {
        return Err(Error::NotFound("parent_command_not_found"));
    }
    Ok(())
}
fn reducing_actor(p: &Principal, policy: &ManagementPolicy, g: &Grant) -> bool {
    p.user_id == g.owner_user_id || p.mxid == policy.administrator()
}
#[derive(QueryableByName)]
struct Amount {
    #[diesel(sql_type=BigInt)]
    amount: i64,
}
async fn later_children(db: &mut AsyncPgConnection, id: &str, revision: i64) -> Result<i64> {
    let sum=sql_query("SELECT COALESCE(SUM((balance::jsonb->>'retainedTokens')::bigint),0)::bigint AS amount FROM hagency_agent_v1.management_allocations WHERE grant_id=$1 AND (balance::jsonb->>'ledgerRevision')::bigint>$2").bind::<Text,_>(id).bind::<BigInt,_>(revision).get_result::<Amount>(db).await?.amount;
    if !(0..=MAX_EXACT).contains(&sum) {
        return Err(Error::Unavailable("parent_liability_overflow"));
    }
    Ok(sum)
}
pub(super) async fn validate_child_receipt(
    db: &mut AsyncPgConnection,
    grant_id: &str,
    receipt: &crate::management_allocation_types::NativeAllocationReceipt,
) -> Result<()> {
    // Independent native commands cannot share a mutation revision. Treating
    // equality as aggregate inclusion could otherwise hide positive liability.
    let collision = sql_query("SELECT COUNT(*) AS amount FROM hagency_agent_v1.management_parent_commands WHERE grant_id=$1 AND ((receipt::jsonb->>'ledgerRevision')::bigint=$2 OR (receipt::jsonb->>'fenceRevision')::bigint=$2)")
        .bind::<Text,_>(grant_id).bind::<BigInt,_>(receipt.ledger_revision).get_result::<Amount>(db).await?.amount;
    if collision > 0 {
        return Err(Error::Conflict("parent_ledger_revision_conflict"));
    }
    if receipt.state == "reserved"
        && let Some(parent) = parent(db, grant_id).await?
        && let Some(parent) = parent
            .receipt
            .as_deref()
            .map(decode::<NativeGrantSettlementReceipt>)
            .transpose()?
        && receipt.ledger_revision >= parent.fence_revision
    {
        return Err(Error::Conflict("parent_fence_revision_conflict"));
    }
    Ok(())
}
async fn view(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<ParentSettlement> {
    let g = grant(db, id).await?;
    let original: NativeGrantReceipt = decode(&g.receipt)?;
    if !reducing_actor(p, policy, &g) && p.mxid != original.owner_mxid {
        return Err(Error::NotFound("parent_settlement_not_found"));
    }
    let a = parent(db, id).await?;
    let receipt = a
        .as_ref()
        .and_then(|a| a.receipt.as_deref())
        .map(decode::<NativeGrantSettlementReceipt>)
        .transpose()?;
    let later = if let Some(r) = &receipt {
        later_children(db, id, r.ledger_revision).await?
    } else {
        0
    };
    let known = super::usage::known(db, super::usage::Scope::Grant(id)).await?;
    // A late call may arrive after a previously accepted zero settlement. Keep
    // original receipts immutable and conservatively add newly known liability
    // until a newer native aggregate explicitly includes it.
    let newly_known = receipt
        .as_ref()
        .map_or(0, |r| known.saturating_sub(r.known_consumed_tokens).max(0));
    let held = receipt
        .as_ref()
        .map_or(original.allocated_tokens.max(known), |r| r.retained_tokens)
        .checked_add(later)
        .and_then(|n| n.checked_add(newly_known))
        .filter(|n| *n <= MAX_EXACT)
        .ok_or(Error::Unavailable("parent_liability_overflow"))?;
    let latest = if let Some(a) = &a {
        Some(work(db, &a.latest_command_id).await?)
    } else {
        None
    };
    let state = if later > 0 || newly_known > 0 {
        "awaiting_reconciliation".into()
    } else if let Some(a) = &a {
        if a.state == "closing"
            && latest
                .as_ref()
                .is_some_and(|w| w.state == "issued" && w.lease_until_ms <= now)
        {
            "outcome_unknown".into()
        } else {
            a.state.clone()
        }
    } else if g.state == "revoked" || original.period_end_ms <= now {
        "awaiting_native_close".into()
    } else {
        "held".into()
    };
    Ok(ParentSettlement {
        grant_id: id.into(),
        grant_revision: g.revision,
        revision: a.as_ref().map_or(0, |a| a.revision),
        state,
        original_granted_tokens: original.allocated_tokens,
        held_tokens: held,
        known_consumed_tokens: receipt
            .as_ref()
            .map(|r| r.known_consumed_tokens.max(known))
            .or_else(|| (known > 0).then_some(known)),
        unresolved_tokens: receipt.as_ref().map(|r| r.unresolved_tokens + later),
        known_unused_released_tokens: original.allocated_tokens.saturating_sub(held).max(0),
        overrun_tokens: held.saturating_sub(original.allocated_tokens).max(0),
        unreconciled_child_tokens: later + newly_known,
        can_close: reducing_actor(p, policy, &g) && a.is_none(),
        can_settle: reducing_actor(p, policy, &g)
            && receipt.is_some()
            && latest.is_some_and(|w| w.state == "recorded"),
        receipt,
        receipt_digest: a.and_then(|a| a.receipt_digest),
    })
}
async fn changed(
    db: &mut AsyncPgConnection,
    g: &Grant,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<()> {
    let (_, c) = contribution(db, &g.contribution_id).await?;
    sql_query("UPDATE hagency_agent_v1.management_requests SET revision=revision+1 WHERE id=$1")
        .bind::<Text, _>(&c.request_id)
        .execute(db)
        .await?;
    let request = request(db, &c.request_id, p, policy, now).await?;
    let original: NativeGrantReceipt = decode(&g.receipt)?;
    notify(
        db,
        &request,
        [
            original.owner_mxid,
            policy.administrator().into(),
        ],
        now,
    )
    .await
}
async fn insert_work(db: &mut AsyncPgConnection, g: &Grant, c: &GrantClosureCommand) -> Result<()> {
    sql_query("INSERT INTO hagency_agent_v1.management_parent_commands(id,grant_id,owner_user_id,executor_device_id,executor_generation,operation,command,digest) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind::<Text,_>(&c.command_id).bind::<Text,_>(&g.id).bind::<Text,_>(&g.owner_user_id).bind::<Text,_>(&g.executor_device_id).bind::<BigInt,_>(g.executor_generation).bind::<Text,_>(&c.operation).bind::<Text,_>(wire(c)?).bind::<Text,_>(allocation_digest(c)?).execute(db).await?;
    Ok(())
}
/// Once per logical grant, under the existing global management transaction lock.
/// No native proof is fabricated here and no amount is reduced.
pub(super) async fn enqueue_close(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    reason: &str,
    now: i64,
) -> Result<()> {
    if !policy.parent_settlement() || parent(db, id).await?.is_some() {
        return Ok(());
    }
    let g = grant(db, id).await?;
    if g.state != "revoked" {
        return Err(Error::Conflict("parent_grant_not_fenced"));
    }
    let command = GrantClosureCommand {
        version: 1,
        command_id: format!("mpc_{}", entity_id()?),
        operation: "grant.close".into(),
        grant_id: id.into(),
        grant_receipt_digest: g.receipt_digest.clone(),
        previous_settlement_digest: None,
        reason: reason.into(),
        requested_at_ms: now,
    };
    sql_query("INSERT INTO hagency_agent_v1.management_parent_settlements(grant_id,state,close_command_id,latest_command_id) VALUES($1,'closing',$2,$2)").bind::<Text,_>(id).bind::<Text,_>(&command.command_id).execute(db).await?;
    insert_work(db, &g, &command).await?;
    changed(db, &g, p, policy, now).await?;
    DomainStore::audit(db, p, "parent.close.queued", id, now).await?;
    Ok(())
}
pub(super) async fn close_for_contribution(
    db: &mut AsyncPgConnection,
    p: &Principal,
    policy: &ManagementPolicy,
    id: &str,
    now: i64,
) -> Result<()> {
    if !policy.parent_settlement() {
        return Ok(());
    }
    // A superseded grant's capacity moved to its successor, which is closed.
    let grants=sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE contribution_id=$1 AND state<>'superseded' ORDER BY id").bind::<Text,_>(id).load::<Grant>(db).await?;
    for g in grants {
        enqueue_close(db, p, policy, &g.id, "contribution_revoked", now).await?;
    }
    Ok(())
}
impl DomainStore {
    pub async fn management_parent_settlement(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<ParentSettlement> {
        enabled(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                view(db, id, p, policy, now).await
            })
            .await
    }
    pub async fn close_management_parent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ParentCloseInput,
    ) -> Result<ParentSettlement> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_grant_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let g = grant(db, id).await?;
                if !reducing_actor(p, policy, &g) {
                    return Err(Error::Unauthorized("parent_close_forbidden"));
                }
                if let Some(saved) =
                    replay::<ParentSettlement>(db, p, &input.command_id, "parent.close", &digest)
                        .await?
                {
                    return view(db, &saved.grant_id, p, policy, now).await;
                }
                if g.revision != input.expected_grant_revision {
                    return Err(Error::Conflict("management_revision_changed"));
                }
                let reason = if p.user_id == g.owner_user_id {
                    "owner_withdrawal"
                } else {
                    "administrator_withdrawal"
                };
                for successor in super::allocations::retire_chain_from(db, p, policy, id, now).await? {
                    enqueue_close(db, p, policy, &successor, reason, now).await?;
                }
                enqueue_close(
                    db,
                    p,
                    policy,
                    id,
                    if p.user_id == g.owner_user_id {
                        "owner_withdrawal"
                    } else {
                        "administrator_withdrawal"
                    },
                    now,
                )
                .await?;
                let result = view(db, id, p, policy, now).await?;
                Self::audit(db, p, "parent.close", id, now).await?;
                receipt(db, p, &input.command_id, "parent.close", &digest, &result).await?;
                Ok(result)
            })
            .await
    }
    pub async fn settle_management_parent(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RevokeInput,
    ) -> Result<ParentSettlement> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        device(p)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let g=grant(db,id).await?;
            if !reducing_actor(p,policy,&g){return Err(Error::Unauthorized("parent_close_forbidden"));}
            if let Some(saved)=replay::<ParentSettlement>(db,p,&input.command_id,"parent.settle",&digest).await?{return view(db,&saved.grant_id,p,policy,now).await;}
            let a=parent(db,id).await?.ok_or(Error::Conflict("parent_close_required"))?;
            if a.revision!=input.expected_revision{return Err(Error::Conflict("management_revision_changed"));}
            if a.receipt_digest.is_none() || work(db,&a.latest_command_id).await?.state!="recorded"{return Err(Error::Conflict("parent_outcome_unknown"));}
            let command=GrantClosureCommand{version:1,command_id:format!("mpc_{}",entity_id()?),operation:"grant.settle".into(),grant_id:id.into(),grant_receipt_digest:g.receipt_digest.clone(),previous_settlement_digest:a.receipt_digest,reason:"settlement_progress".into(),requested_at_ms:now};
            insert_work(db,&g,&command).await?;
            sql_query("UPDATE hagency_agent_v1.management_parent_settlements SET state='closing',revision=revision+1,latest_command_id=$2 WHERE grant_id=$1").bind::<Text,_>(id).bind::<Text,_>(&command.command_id).execute(db).await?;
            changed(db,&g,p,policy,now).await?;let result=view(db,id,p,policy,now).await?;Self::audit(db,p,"parent.settle",id,now).await?;receipt(db,p,&input.command_id,"parent.settle",&digest,&result).await?;Ok(result)
        }).await
    }
    pub async fn recover_management_parent_command(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RecoverCommandInput,
    ) -> Result<serde_json::Value> {
        enabled(policy)?;
        key(id)?;
        let (executor, _) = device(p)?;
        if !matches!(input.operation.as_str(), "parent.close" | "parent.settle") {
            return Err(Error::Invalid("invalid_management_operation"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let rows=sql_query("SELECT operation,digest,result FROM hagency_agent_v1.management_commands WHERE actor_user_id=$1 AND device_id=$2 AND command_id=$3 ORDER BY device_generation LIMIT 2").bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<Text,_>(id).load::<Receipt>(db).await?;
            if rows.len()>1{return Err(Error::Conflict("management_command_conflict"));}
            let Some(row)=rows.as_slice().first()else{return Ok(serde_json::json!({"recorded":false}))};
            if row.operation!=input.operation{return Err(Error::Conflict("management_command_conflict"));}
            let saved:ParentSettlement=decode(&row.result)?;Ok(serde_json::json!({"recorded":true,"settlement":view(db,&saved.grant_id,p,policy,now).await?}))
        }).await
    }
}
async fn deliver(
    db: &mut AsyncPgConnection,
    p: &Principal,
    w: &Work,
    now: i64,
    historical: bool,
) -> Result<ParentDelivery> {
    executor(w, p)?;
    let token = crate::secret_token();
    let until = now + 30000;
    sql_query("UPDATE hagency_agent_v1.management_parent_commands SET state=CASE WHEN state='pending' THEN 'issued' ELSE state END,lease_hash=$2,lease_until_ms=$3 WHERE id=$1").bind::<Text,_>(&w.id).bind::<Text,_>(hash(&token)).bind::<BigInt,_>(until).execute(db).await?;
    Ok(ParentDelivery {
        command: decode(&w.command)?,
        command_digest: w.digest.clone(),
        token,
        lease_until_ms: until,
        execution_allowed: !historical && w.receipt.is_none(),
        recorded: w.receipt.as_deref().map(decode).transpose()?,
    })
}
impl DomainStore {
    pub async fn poll_management_parent_executor(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        input: ExecutorPollInput,
    ) -> Result<Option<ParentDelivery>> {
        enabled(policy)?;
        let (executor, _) = device(p)?;
        if let Some(id) = &input.after {
            key(id)?;
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let w=sql_query("SELECT * FROM hagency_agent_v1.management_parent_commands WHERE owner_user_id=$1 AND executor_device_id=$2 AND state<>'recorded' AND lease_until_ms<=$3 ORDER BY (id<=$4),id LIMIT 1").bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<BigInt,_>(now).bind::<Text,_>(input.after.as_deref().unwrap_or("")).get_result::<Work>(db).await.optional()?;
            match w{Some(w)=>Ok(Some(deliver(db,p,&w,now,false).await?)),None=>Ok(None)}
        }).await
    }
    pub async fn recover_management_parent_executor(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ParentRecoverInput,
    ) -> Result<ParentDelivery> {
        enabled(policy)?;
        key(id)?;
        device(p)?;
        if !valid_digest(&input.command_digest) {
            return Err(Error::Invalid("invalid_command_digest"));
        }
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                let w = work(db, id).await?;
                executor(&w, p)?;
                if w.digest != input.command_digest {
                    return Err(Error::Conflict("executor_command_conflict"));
                }
                deliver(db, p, &w, now, true).await
            })
            .await
    }
    pub async fn record_management_parent_result(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: ParentResultInput,
    ) -> Result<ParentAcknowledgement> {
        enabled(policy)?;
        key(id)?;
        device(p)?;
        if input.token.len() > 512 || !valid_digest(&input.command_digest) {
            return Err(Error::Invalid("invalid_parent_result"));
        }
        let result_digest = allocation_digest(&input.receipt)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;let w=work(db,id).await?;executor(&w,p)?;
            if input.command_digest!=w.digest{return Err(Error::Conflict("executor_command_conflict"));}
            if let Some(previous)=&w.receipt_digest {
                if previous!=&result_digest{return Err(Error::Conflict("executor_result_conflict"));}
                return Ok(ParentAcknowledgement{command_id:id.into(),command_digest:w.digest,receipt_digest:result_digest,settlement:view(db,&w.grant_id,p,policy,now).await?});
            }
            if w.lease_until_ms<=now || w.lease_hash.as_deref()!=Some(hash(&input.token).as_str()){return Err(Error::Unauthorized("executor_result_ticket_required"));}
            let g=grant(db,&w.grant_id).await?;let original:NativeGrantReceipt=decode(&g.receipt)?;let a=parent(db,&w.grant_id).await?.ok_or(Error::Unavailable("invalid_parent_record"))?;
            let r=&input.receipt;
            let amounts=[r.retained_tokens,r.known_consumed_tokens,r.unresolved_tokens,r.liability_count];
            if r.version!=1 || r.command_id!=id || r.command_digest!=w.digest || r.grant_id!=g.id || r.grant_receipt_digest!=g.receipt_digest || r.enrollment_id!=original.enrollment_id || r.period_key!=original.period_key || amounts.iter().any(|n|!(0..=MAX_EXACT).contains(n)) || r.known_consumed_tokens.checked_add(r.unresolved_tokens)!=Some(r.retained_tokens) || !(1..=MAX_EXACT).contains(&r.fence_revision) || !(1..=MAX_EXACT).contains(&r.ledger_revision) || r.fence_revision<=original.ledger_revision || r.ledger_revision<r.fence_revision || !valid_digest(&r.liability_digest) || (r.retained_tokens>0 && r.liability_count==0) || !matches!((r.state.as_str(),r.unresolved_tokens),("closed",0)|("retained",1..)) {return Err(Error::Invalid("invalid_parent_result"));}
            if r.known_consumed_tokens < super::usage::known(db,super::usage::Scope::Grant(&g.id)).await? {return Err(Error::Conflict("parent_usage_changed"));}
            let last_reserve=sql_query("SELECT COALESCE(MAX((w.receipt::jsonb->>'ledgerRevision')::bigint),0) AS amount FROM hagency_agent_v1.management_executor_commands w JOIN hagency_agent_v1.management_allocations a ON a.id=w.allocation_id WHERE a.grant_id=$1 AND w.operation='project.reserve' AND w.receipt::jsonb->>'state'='reserved'").bind::<Text,_>(&g.id).get_result::<Amount>(db).await?.amount;
            if r.fence_revision<=last_reserve{return Err(Error::Conflict("parent_fence_revision_conflict"));}
            let collision=sql_query("SELECT COUNT(*) AS amount FROM hagency_agent_v1.management_executor_commands w JOIN hagency_agent_v1.management_allocations a ON a.id=w.allocation_id WHERE a.grant_id=$1 AND ((w.receipt::jsonb->>'ledgerRevision')::bigint=$2 OR (w.receipt::jsonb->>'ledgerRevision')::bigint=$3)").bind::<Text,_>(&g.id).bind::<BigInt,_>(r.ledger_revision).bind::<BigInt,_>(r.fence_revision).get_result::<Amount>(db).await?.amount;
            if collision>0{return Err(Error::Conflict("parent_ledger_revision_conflict"));}
            super::agents::executor::validate_external_ledger_receipt(db,&w.owner_user_id,&w.executor_device_id,r.ledger_revision,r.fence_revision).await?;
            if a.latest_command_id!=id{return Err(Error::Conflict("parent_command_superseded"));}
            if let Some(previous)=a.receipt.as_deref().map(decode::<NativeGrantSettlementReceipt>).transpose()?
                && (r.fence_revision!=previous.fence_revision || r.ledger_revision<=previous.ledger_revision || r.known_consumed_tokens<previous.known_consumed_tokens) {
                return Err(Error::Conflict("parent_ledger_revision_changed"));
            }
            // A single irreversible native fence covers every child, including
            // unseen queued reserves. Central status never fabricates child receipts.
            sql_query("UPDATE hagency_agent_v1.management_parent_commands SET state='recorded',receipt=$2,receipt_digest=$3 WHERE id=$1").bind::<Text,_>(id).bind::<Text,_>(wire(r)?).bind::<Text,_>(&result_digest).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_parent_settlements SET state=$2,receipt=$3,receipt_digest=$4,revision=revision+1 WHERE grant_id=$1").bind::<Text,_>(&g.id).bind::<Text,_>(&r.state).bind::<Text,_>(wire(r)?).bind::<Text,_>(&result_digest).execute(db).await?;
            changed(db,&g,p,policy,now).await?;Self::audit(db,p,"parent.executor.result",id,now).await?;
            Ok(ParentAcknowledgement{command_id:id.into(),command_digest:w.digest,receipt_digest:result_digest,settlement:view(db,&g.id,p,policy,now).await?})
        }).await
    }
}
