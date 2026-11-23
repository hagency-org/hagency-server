//! Generic allocation notices for current members of a verified bound room.
use super::*;

impl DomainStore {
    pub(crate) async fn room_allowance_page(
        &self,
        p: &Principal,
        room: &str,
        after: &str,
        gateway: &Gateway,
    ) -> Result<serde_json::Value> {
        let candidates=self.db.transaction(async |db| {
            Self::authorize(db,p,0).await?;
            Ok(sql_query("SELECT a.id FROM hagency_agent_v1.management_shared_assignments a
                JOIN hagency_agent_v1.shared_room_memberships m ON m.assignment_id=a.id
                JOIN hagency_agent_v1.management_agent_requests q ON q.id=a.request_id
                WHERE m.room_id=$1 AND m.state='matrix_verified' AND q.pool_contribution_id IS NOT NULL
                AND a.state='pending_native' AND a.id>$2 ORDER BY a.id LIMIT 9")
                .bind::<Text,_>(room).bind::<Text,_>(after).load::<Id>(db).await?)
        }).await?;
        let mut items = Vec::new();
        let mut next = None;
        if candidates.is_empty() || !gateway.viewer_joined(room, &p.mxid).await? {
            return Ok(json!({"items":[],"next":null}));
        }
        for candidate in candidates.iter().take(8) {
            let id = &candidate.id;
            let observed = self
                .db
                .transaction(async |db| intake::member_for(db, id, room).await)
                .await;
            let prior = match observed {
                Ok(m) => m,
                Err(Error::NotFound(_)) => continue,
                Err(e) => return Err(e),
            };
            // Release the database connection during Matrix I/O. Recheck
            // binding generation, device/grant authority and freshness on return.
            let fact = gateway
                .shared_delivery(
                    room,
                    &prior.space_id,
                    &prior.manager_mxid,
                    &p.mxid,
                    &prior.puppet_mxid,
                )
                .await?;
            let notice = self
                .db
                .transaction(async |db| {
                    Self::authorize(db, p, 0).await?;
                    let current = match intake::member_for(db, id, room).await {
                        Ok(m) => m,
                        Err(Error::NotFound(_)) => return Ok(None),
                        Err(e) => return Err(e),
                    };
                    if !current.pool_agent
                        || current.binding_id != prior.binding_id
                        || current.binding_generation != prior.binding_generation
                        || fact.requester_mxid != p.mxid
                        || facts(&current, &fact).is_err()
                    {
                        return Ok(None);
                    }
                    let now = clock(db).await?;
                    let w = intake::assignment(db, id).await?;
                    if !intake::retainable(db, id, &w, &self.server, now).await? {
                        return Ok(None);
                    }
                    let reason = pool::room_allowance_reason(db, &w.request_id, p, now).await?;
                    Ok(reason.map(|state| json!({"agentMxid":current.puppet_mxid,"state":state})))
                })
                .await?;
            if let Some(notice) = notice {
                items.push(notice);
            }
        }
        if candidates.len() > 8 {
            next = Some(&candidates[7].id);
        }
        Ok(json!({"items":items,"next":next}))
    }
}
