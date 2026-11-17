//! Owner rename of a personal Agent. The Matrix profile is pushed by the API
//! layer after commit; a `creating` identity picks the name up on provisioning.
use super::*;

/// Shared by create and rename so both enforce one name policy.
pub(crate) fn agent_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(Error::Invalid("invalid_agent_name"));
    }
    Ok(())
}

impl DomainStore {
    /// Permanent owner only; retiring or retired identities keep their last name.
    pub async fn rename_agent(&self, p: &Principal, id: &str, name: &str, now: i64) -> Result<Agent> {
        key(id)?;
        agent_name(name)?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, now).await?;
            sql_query("SELECT true AS matched FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2 FOR UPDATE").bind::<Text,_>(id).bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await.map_err(hidden)?;
            let agent = Self::agent_db(db, p, id).await?;
            if matches!(agent.state.as_str(), "retiring" | "retired") {
                return Err(Error::Conflict("agent_retired"));
            }
            sql_query("UPDATE hagency_agent_v1.agents SET display_name=$1 WHERE id=$2").bind::<Text,_>(name).bind::<Text,_>(id).execute(db).await?;
            Self::audit(db, p, "agent.rename", id, now).await?;
            Self::agent_db(db, p, id).await
        }).await
    }
}
