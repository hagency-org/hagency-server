//! Edits and redactions of events the shared journal already knows. They are
//! retained as evidence for later context freezes and never become prompts.
use super::*;
use crate::management_shared_context_types::{RevisionKind, RoutedRevision};

async fn known_request(db: &mut AsyncPgConnection, room: &str, event: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work WHERE event_id=$1 AND payload::jsonb->>'roomId'=$2) AS matched")
        .bind::<Text, _>(event)
        .bind::<Text, _>(room)
        .get_result::<Flag>(db)
        .await?
        .matched)
}
async fn known_reply(db: &mut AsyncPgConnection, room: &str, event: &str) -> Result<bool> {
    Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_replies x JOIN hagency_agent_v1.management_shared_work w ON w.id=x.work_id WHERE x.matrix_event_id=$1 AND w.payload::jsonb->>'roomId'=$2) AS matched")
        .bind::<Text, _>(event)
        .bind::<Text, _>(room)
        .get_result::<Flag>(db)
        .await?
        .matched)
}
impl DomainStore {
    /// Called only with a revision parsed from a verified AS transaction.
    /// Unknown targets are dropped: only quoted requests and sent replies matter.
    pub(crate) async fn retain_shared_revision(&self, r: &RoutedRevision, received: i64) -> Result<()> {
        r.validate()?;
        if received <= 0 {
            return Err(Error::Invalid("invalid_event_received_time"));
        }
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                let known = match &r.kind {
                    RevisionKind::Edit { .. } => known_request(db, &r.room_id, &r.target_event_id).await?,
                    RevisionKind::Redaction => {
                        known_request(db, &r.room_id, &r.target_event_id).await?
                            || known_reply(db, &r.room_id, &r.target_event_id).await?
                    }
                };
                if !known {
                    return Ok(());
                }
                let (kind, body) = match &r.kind {
                    RevisionKind::Edit { body } => ("edit", Some(body.clone())),
                    RevisionKind::Redaction => ("redaction", None),
                };
                let digest = body.as_deref().map(hash);
                sql_query("INSERT INTO hagency_agent_v1.management_shared_event_revisions VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(revision_event_id) DO NOTHING")
                    .bind::<Text, _>(&r.revision_event_id)
                    .bind::<Text, _>(&r.room_id)
                    .bind::<Text, _>(&r.target_event_id)
                    .bind::<Text, _>(kind)
                    .bind::<Text, _>(&r.sender_mxid)
                    .bind::<Nullable<Text>, _>(body)
                    .bind::<Nullable<Text>, _>(digest)
                    .bind::<BigInt, _>(received)
                    .execute(db)
                    .await?;
                Ok(())
            })
            .await
    }
}
