//! Recovery changes authority, never the original uncertain Matrix command.
use super::*;
impl DomainStore {
    pub async fn cancel_management_project_setup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: CancelProjectSetup,
    ) -> Result<ProjectReadiness> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        if input.expected_revision <= 0 {
            return Err(Error::Invalid("invalid_management_revision"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let current=setup_id(db,id).await?;let plan=plan(db,&current.project_id).await?;
            if plan.manager_user_id!=p.user_id && p.mxid!=policy.administrator(){return Err(Error::Unauthorized("project_setup_cancel_denied"));}
            if let Some(saved)=replay::<ProjectReadiness>(db,p,&input.command_id,"project.setup.cancel",&payload_digest).await?{return view(db,&saved.project_id,p,policy,now).await;}
            if current.revision!=input.expected_revision{return Err(Error::Conflict("management_revision_changed"));}
            if current.state=="cancelled"{return Err(Error::Conflict("project_setup_cancelled"));}
            sql_query("UPDATE hagency_agent_v1.management_project_setups SET state='cancelled',revision=revision+1,verified_at_ms=NULL WHERE id=$1").bind::<Text,_>(id).execute(db).await?;
            changed(db,&plan,p,policy,now).await?;let result=view(db,&plan.id,p,policy,now).await?;
            Self::audit(db,p,"project.setup.cancel",id,now).await?;
            receipt(db,p,&input.command_id,"project.setup.cancel",&payload_digest,&result).await?;Ok(result)
        }).await
    }
    pub async fn rebase_management_project_setup(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: RebaseProjectSetup,
    ) -> Result<ProjectReadiness> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        key(&input.allocation_id)?;
        if [
            input.expected_revision,
            input.expected_project_revision,
            input.expected_allocation_revision,
            input.expected_authority_revision,
        ]
        .iter()
        .any(|n| *n <= 0)
        {
            return Err(Error::Invalid("invalid_project_setup"));
        }
        let payload_digest = digest(id, &input)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;let now=clock(db).await?;
            let current=setup_id(db,id).await?;let plan=plan(db,&current.project_id).await?;
            if plan.manager_user_id!=p.user_id{return Err(Error::Unauthorized("project_manager_required"));}
            if let Some(saved)=replay::<ProjectReadiness>(db,p,&input.command_id,"project.setup.rebase",&payload_digest).await?{return view(db,&saved.project_id,p,policy,now).await;}
            if plan.state!="approved"{return Err(Error::Conflict("project_unavailable"));}
            if current.revision!=input.expected_revision || plan.revision!=input.expected_project_revision{return Err(Error::Conflict("management_revision_changed"));}
            let auth=authority(db,&plan.id).await?.filter(|a|a.active).ok_or(Error::Conflict("project_administrator_required"))?;
            if auth.revision!=input.expected_authority_revision{return Err(Error::Conflict("management_revision_changed"));}
            let capacity=allocations::setup_capacity(db,&plan.id,Some(&input.allocation_id),now).await?.filter(|_|policy.project_reservations()).ok_or(Error::Conflict("live_reservation_required"))?;
            if capacity.revision!=input.expected_allocation_revision{return Err(Error::Conflict("management_revision_changed"));}
            let next=binding(&current)?.revision+1;
            sql_query("INSERT INTO hagency_agent_v1.management_setup_bindings(setup_id,revision,allocation_id,allocation_receipt_digest,plan_revision,authority_revision,administrator_mxid) VALUES($1,$2,$3,$4,$5,$6,$7)").bind::<Text,_>(id).bind::<BigInt,_>(next).bind::<Text,_>(&capacity.id).bind::<Text,_>(&capacity.receipt_digest).bind::<BigInt,_>(plan.revision).bind::<BigInt,_>(auth.revision).bind::<Text,_>(&auth.mxid).execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.management_project_setups SET current_binding_revision=$2,state='awaiting_matrix_setup',revision=revision+1,verified_at_ms=NULL WHERE id=$1").bind::<Text,_>(id).bind::<BigInt,_>(next).execute(db).await?;
            changed(db,&plan,p,policy,now).await?;let result=view(db,&plan.id,p,policy,now).await?;
            Self::audit(db,p,"project.setup.rebase",id,now).await?;
            receipt(db,p,&input.command_id,"project.setup.rebase",&payload_digest,&result).await?;Ok(result)
        }).await
    }
}
