//! Definitive non-publication proof for an expired V4 grant. Time alone is not
//! proof: a tombstone serializes with publication and permanently blocks replay.
use super::*;

pub(super) async fn require_unexpired_publication(
    db: &mut AsyncPgConnection,
    p: &Principal,
    command: &str,
    operation: &str,
    payload_digest: &str,
) -> Result<()> {
    let (executor, _) = device(p)?;
    let row=sql_query("SELECT operation,payload_digest AS digest,''::text AS result FROM hagency_agent_v1.management_grant_publication_tombstones WHERE owner_user_id=$1 AND executor_device_id=$2 AND command_id=$3")
        .bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<Text,_>(command).get_result::<Receipt>(db).await.optional()?;
    if let Some(row) = row {
        if row.operation != operation || row.digest != payload_digest {
            return Err(Error::Conflict("management_command_conflict"));
        }
        return Err(Error::Conflict("grant_publication_expired"));
    }
    Ok(())
}
impl DomainStore {
    pub async fn expire_management_grant_publication(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        command_id: &str,
        input: ExpireGrantPublication,
    ) -> Result<GrantPublicationExpiry> {
        enabled(policy)?;
        key(command_id)?;
        key(&input.target_id)?;
        let (executor, generation) = device(p)?;
        let (receipt, original_digest, publication_digest) = match input.operation.as_str() {
            "grant.publish" => {
                let v: PublishGrantInput = serde_json::from_value(input.publication.clone())
                    .map_err(|_| Error::Invalid("invalid_grant_publication"))?;
                if v.command_id != command_id
                    || v.receipt.contribution_id != input.target_id
                    || v.receipt.succession.is_some()
                {
                    return Err(Error::Invalid("invalid_grant_publication"));
                }
                let d = digest(&input.target_id, &v)?;
                let pd = allocation_digest(&v)?;
                (v.receipt, d, pd)
            }
            "grant.succeed" => {
                let v: SucceedGrantInput = serde_json::from_value(input.publication.clone())
                    .map_err(|_| Error::Invalid("invalid_grant_publication"))?;
                if v.command_id != command_id
                    || v.receipt
                        .succession
                        .as_ref()
                        .is_none_or(|s| s.predecessor_grant_id != input.target_id)
                {
                    return Err(Error::Invalid("invalid_grant_publication"));
                }
                let d = digest(&input.target_id, &v)?;
                let pd = allocation_digest(&v)?;
                (v.receipt, d, pd)
            }
            _ => return Err(Error::Invalid("invalid_management_operation")),
        };
        receipt.validate(&self.server)?;
        if receipt.calendar.is_none() {
            return Err(Error::Invalid("calendar_grant_required"));
        }
        if receipt.owner_mxid != p.mxid
            || receipt.executor_device_id != executor
            || receipt.executor_generation > generation
        {
            return Err(Error::Unauthorized("contribution_executor_required"));
        }
        let receipt_digest = allocation_digest(&receipt)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection| {
            // authorize takes the same transaction advisory lock as publication.
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            if receipt.period_end_ms.min(receipt.delegation_expires_at_ms)>now {return Err(Error::Conflict("grant_publication_not_expired"))}
            let (owner,c)=contribution(db,&receipt.contribution_id).await?;
            if owner!=p.user_id || c.executor_device_id!=executor {return Err(Error::Unauthorized("contribution_executor_required"))}
            let committed=if let Some(saved)=replay::<ResourceGrantView>(db,p,command_id,&input.operation,&original_digest).await? {
                Some(grant_view(&grant(db,&saved.id).await?,now)?)
            }else{
                let existing=sql_query("SELECT * FROM hagency_agent_v1.management_resource_grants WHERE id=$1").bind::<Text,_>(&receipt.grant_id).get_result::<GrantRow>(db).await.optional()?;
                if let Some(g)=existing {if g.owner_user_id!=p.user_id || g.receipt_digest!=receipt_digest {return Err(Error::Conflict("resource_grant_conflict"))}Some(grant_view(&g,now)?)}else{None}
            };
            if committed.is_none() {
                let existing=sql_query("SELECT operation,payload_digest AS digest,''::text AS result FROM hagency_agent_v1.management_grant_publication_tombstones WHERE owner_user_id=$1 AND executor_device_id=$2 AND command_id=$3").bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<Text,_>(command_id).get_result::<Receipt>(db).await.optional()?;
                if let Some(existing)=existing {if existing.operation!=input.operation || existing.digest!=original_digest {return Err(Error::Conflict("management_command_conflict"))}}
                else {
                    sql_query("INSERT INTO hagency_agent_v1.management_grant_publication_tombstones(owner_user_id,executor_device_id,executor_generation,command_id,operation,payload_digest,publication_digest,receipt_digest,observed_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind::<Text,_>(&p.user_id).bind::<Text,_>(executor).bind::<BigInt,_>(receipt.executor_generation).bind::<Text,_>(command_id).bind::<Text,_>(&input.operation).bind::<Text,_>(&original_digest).bind::<Text,_>(&publication_digest).bind::<Text,_>(&receipt_digest).bind::<BigInt,_>(now).execute(db).await?;
                    Self::audit(db,p,"grant.publication.expire",command_id,now).await?;
                }
            }
            Ok(GrantPublicationExpiry{status:if committed.is_some(){"published"}else{"expired_unpublished"}.into(),command_id:command_id.into(),publication_digest,receipt_digest,observed_at_ms:now,grant:committed})
        }).await
    }
}
