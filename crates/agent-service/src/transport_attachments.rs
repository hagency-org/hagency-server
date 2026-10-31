use super::*;
use diesel::{ExpressionMethods, QueryDsl};
#[derive(diesel::Queryable, diesel::QueryableByName)]
struct AttachmentRow {
    #[diesel(sql_type=diesel::sql_types::Jsonb)]
    descriptor: serde_json::Value,
    #[diesel(sql_type=Text)]
    descriptor_digest: String,
}
impl TransportStore {
    /// The original running execution is the only selector. A client cannot
    /// supply another event, Room, requester, MXC URL or decryption key.
    pub async fn execution_attachments(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        id: &str,
        execution: &str,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<Vec<crate::attachments::Attachment>> {
        self.observe_event_authority(p, id, f, now).await?;
        if !f.requester_in_room {
            self.reject_event_delivery(id, f, now).await?;
            return Err(Error::Unauthorized("requester_membership_revoked"));
        }
        key(execution)?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::lock(db).await?;
                    let (now, _) = Self::authenticate(db, p, now).await?;
                    let event = Self::dispatch(db, p, id).await?;
                    Self::dispatch_scope(db, p, reference, &event, f, now).await?;
                    fenced(&event, p, reference)?;
                    if event.state != "running" || event.execution_id.as_deref() != Some(execution)
                    {
                        return Err(Error::Conflict("execution_not_running"));
                    }
                    let row = {
                        use crate::schema::agent_tables::event_attachments as q;
                        let params = (id,);
                        q::table
                            .select((q::descriptor, q::descriptor_digest))
                            .filter((q::dispatch_id).eq(&params.0))
                            .get_result::<AttachmentRow>(db)
                            .await
                    }
                    .optional()?;
                    let Some(row) = row else { return Ok(vec![]) };
                    let attachment: crate::attachments::Attachment =
                        serde_json::from_value(row.descriptor)
                            .map_err(|_| Error::Unavailable("invalid_stored_attachment"))?;
                    attachment
                        .validate()
                        .map_err(|_| Error::Unavailable("invalid_stored_attachment"))?;
                    if attachment.descriptor_digest != row.descriptor_digest {
                        return Err(Error::Unavailable("invalid_stored_attachment"));
                    }
                    Ok(vec![attachment])
                })
                .await
            })
            .await
    }
}
