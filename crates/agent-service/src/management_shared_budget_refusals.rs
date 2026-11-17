//! Definitive never-started proof. No HTTP failure or absent process is proof.
use super::*;
use crate::management_shared_budget_types::*;

pub(super) async fn exists(db: &mut AsyncPgConnection, id: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work_refusals WHERE work_id=$1) AS matched")
        .bind::<Text,_>(id).get_result::<Flag>(db).await?.matched)
}
pub(super) async fn ensure_unfenced(db: &mut AsyncPgConnection, id: &str) -> Result<()> {
    if exists(db, id).await? {
        return Err(Error::Conflict("shared_work_definitively_not_started"));
    }
    Ok(())
}
pub(super) fn validate_zero(r: &SharedWorkReceipt) -> Result<()> {
    if r.state != "failed"
        || r.known_consumed_tokens != 0
        || r.retained_tokens != Some(0)
        || !r.usage_complete
        || r.body.is_some()
        || r.body_digest.is_some()
    {
        return Err(Error::Conflict("shared_no_start_zero_evidence_required"));
    }
    Ok(())
}
#[derive(QueryableByName)]
struct Saved {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
}
impl DomainStore {
    pub async fn refuse_unissued_shared_budget_work(
        &self,
        p: &Principal,
        id: &str,
        input: SharedBudgetWorkRefusalInput,
    ) -> Result<SharedBudgetWorkRefusalRecord> {
        self.shared_budget_work_refusal(p, id, input, true)
            .await?
            .ok_or(Error::NotFound("shared_work_refusal_missing"))
    }
    pub async fn recover_shared_budget_work_refusal(
        &self,
        p: &Principal,
        id: &str,
        input: SharedBudgetWorkRefusalInput,
    ) -> Result<Option<SharedBudgetWorkRefusalRecord>> {
        self.shared_budget_work_refusal(p, id, input, false).await
    }
    async fn shared_budget_work_refusal(
        &self,
        p: &Principal,
        id: &str,
        input: SharedBudgetWorkRefusalInput,
        create: bool,
    ) -> Result<Option<SharedBudgetWorkRefusalRecord>> {
        key(id)?;
        input.validate()?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
            Self::authorize(db,p,0).await?;
            let w=work_row(db,id).await?;let r=runtime(db,&w.runtime_id).await?;
            let original=original(db,&r).await?;owner(&original,p)?;
            let c=budget_attempts::attempt_command(&r,&self.server)?;let v=item(&w,&r)?;
            if r.digest!=input.runtime_command_digest || w.digest!=input.work_digest {return Err(Error::Conflict("shared_work_changed"));}
            let old=sql_query("SELECT payload,digest FROM hagency_agent_v1.management_shared_work_refusals WHERE work_id=$1")
                .bind::<Text,_>(id).get_result::<Saved>(db).await.optional()?;
            if let Some(old)=old {
                let refusal:SharedBudgetWorkRefusal=decode(&old.payload)?;
                refusal.validate_for(&c,&v,&input.input_digest)?;
                if allocation_digest(&refusal)?!=old.digest {return Err(Error::Conflict("shared_work_refusal_changed"));}
                return Ok(Some(SharedBudgetWorkRefusalRecord{refusal,refusal_digest:old.digest}));
            }
            if !create {return Ok(None);}
            // Saved contextual bytes or the exact original plain body, never
            // caller-selected replacement text. Both Start paths share this lock.
            let input_digest=context::saved_input_digest(db,&v).await?;
            if input_digest!=input.input_digest {return Err(Error::Conflict("shared_context_changed"));}
            let (head,_,_)=super::super::super::super::budget::execution_head(db,&original.request_id,&self.server).await?;
            if head.budget_revision<=c.execution_budget.budget_revision || head.budget_digest==c.execution_budget.budget_digest {
                return Err(Error::Conflict("shared_budget_head_unchanged"));
            }
            if w.started_at_ms.is_some() || w.latest_receipt.is_some() || !matches!(w.state.as_str(),"queued"|"acknowledged"|"cancelled") {
                return Err(Error::Conflict("shared_work_may_have_started"));
            }
            let refusal=SharedBudgetWorkRefusal {version:1,refusal_id:format!("sbr_{id}"),work_id:id.into(),work_digest:w.digest,
                runtime_command_id:r.id,runtime_command_digest:r.digest,input_digest,execution_budget:c.execution_budget.clone(),
                current_budget_digest:head.budget_digest,state:"not_started".into(),reason:"budget_head_changed".into(),refused_at_ms:clock(db).await?};
            refusal.validate_for(&c,&v,&input.input_digest)?;let digest=allocation_digest(&refusal)?;
            sql_query("INSERT INTO hagency_agent_v1.management_shared_work_refusals VALUES($1,$2,$3)")
                .bind::<Text,_>(id).bind::<Text,_>(wire(&refusal)?).bind::<Text,_>(&digest).execute(db).await?;
            // Visible terminal work disposition retains the event and all native
            // holds. Only a separate exact native zero-use receipt settles them.
            sql_query("UPDATE hagency_agent_v1.management_shared_work SET state='cancelled' WHERE id=$1")
                .bind::<Text,_>(id).execute(db).await?;
            Ok(Some(SharedBudgetWorkRefusalRecord{refusal,refusal_digest:digest}))
        }).await
    }
}
