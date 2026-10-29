//! Exact, bounded human review; private grant receipts and bearer capabilities
//! stay native. Existing public allocation balance receipts remain visible.
use super::*;
const MAX_PAGE_BYTES: usize = 96 * 1024;

fn append_page(
    items: &mut Vec<ProjectAllocationDetail>,
    bytes: &mut usize,
    item: ProjectAllocationDetail,
) -> Result<bool> {
    let size = serde_json::to_vec(&item)
        .map_err(|_| Error::Unavailable("allocation_projection_invalid"))?
        .len()
        + 1;
    if *bytes + size > MAX_PAGE_BYTES {
        if items.is_empty() {
            return Err(Error::Unavailable("allocation_projection_too_large"));
        }
        return Ok(false);
    }
    *bytes += size;
    items.push(item);
    Ok(true)
}

async fn detail(
    db: &mut AsyncPgConnection,
    id: &str,
    p: &Principal,
    policy: &ManagementPolicy,
    now: i64,
) -> Result<ProjectAllocationDetail> {
    // Establish visibility before reading descriptive parent or project fields.
    let allocation = view(db, id, p, policy, now).await?;
    let m = plan(db, &allocation.project_id).await?;
    let (_, c) = contribution(db, &m.contribution_id).await?;
    let g = grant(db, &allocation.grant_id).await?;
    let v = grant_view(&g, now)?;
    let r = v.receipt;
    // Allocation visibility can retain the original delegated coordinator even
    // when the contribution has a newer coordinator. Read only this request
    // after the exact allocation ACL; do not widen the generic Inbox endpoint.
    let row = sql_query(format!("{REQUESTS} WHERE r.id=$1"))
        .bind::<Text, _>(&allocation.request_id)
        .get_result::<RequestRow>(db)
        .await?;
    let mut request = project_request(decode(&row.payload)?, p, policy, now);
    request.can_decide = allocation.can_decide;
    Ok(ProjectAllocationDetail {
        request,
        allocation,
        project_name: m.name,
        resource_label: c.label,
        grant: AllocationGrantSummary {
            id: v.id,
            revision: v.revision,
            state: v.state,
            owner_mxid: r.owner_mxid.clone(),
            coordinator_mxid: r.owner_mxid,
            total_tokens: r.allocated_tokens,
            period_key: r.period_key,
            period_start_ms: r.period_start_ms,
            period_end_ms: r.period_end_ms,
            delegation_expires_at_ms: r.delegation_expires_at_ms,
            native_available_tokens: None,
        },
    })
}
impl DomainStore {
    pub async fn management_allocation_detail(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
    ) -> Result<ProjectAllocationDetail> {
        enabled(policy)?;
        key(id)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let now = clock(db).await?;
                detail(db, id, p, policy, now).await
            })
            .await
    }
    pub async fn management_project_allocations(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        project: &str,
        query: ProjectAllocationsQuery,
    ) -> Result<ProjectAllocationPage> {
        enabled(policy)?;
        key(project)?;
        if let Some(after) = &query.after {
            key(after)?;
        }
        let limit = query.limit.unwrap_or(16);
        if !(1..=16).contains(&limit) {
            return Err(Error::Invalid("invalid_allocation_query"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,crate::api::now_ms()).await?;
            let now=clock(db).await?;
            // The exact allocation ACL precedes both cursor and page bound.
            // Capacity coordination belongs to the resource owner, including historical grants.
            let related=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_allocations a JOIN hagency_agent_v1.management_resource_grants g ON g.id=a.grant_id JOIN hagency_agent_v1.management_projects m ON m.id=a.project_id WHERE a.project_id=$1 AND (m.manager_user_id=$2 OR g.owner_user_id=$2 OR g.receipt::jsonb->>'ownerMxid'=$3 OR $3=$4)) AS matched")
                .bind::<Text,_>(project).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(policy.administrator()).get_result::<Flag>(db).await?.matched;
            if !related {super::super::projects::project(db,project,p,policy,now).await?;}
            let rows=sql_query("SELECT a.* FROM hagency_agent_v1.management_allocations a JOIN hagency_agent_v1.management_resource_grants g ON g.id=a.grant_id JOIN hagency_agent_v1.management_projects m ON m.id=a.project_id WHERE a.project_id=$1 AND (m.manager_user_id=$2 OR g.owner_user_id=$2 OR g.receipt::jsonb->>'ownerMxid'=$3 OR $3=$4) AND a.id>$5 ORDER BY a.id LIMIT $6")
                .bind::<Text,_>(project).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(policy.administrator()).bind::<Text,_>(query.after.as_deref().unwrap_or("")).bind::<BigInt,_>(i64::from(limit)+1).load::<AllocationRow>(db).await?;
            let mut more=rows.len()>usize::from(limit);
            let mut items=Vec::new();
            let mut bytes=512;
            for a in rows.into_iter().take(usize::from(limit)) {
                if !append_page(&mut items,&mut bytes,detail(db,&a.id,p,policy,now).await?)? {more=true;break;}
            }
            let next_after=if more {items.last().map(|v|v.allocation.id.clone())}else{None};
            Ok(ProjectAllocationPage{items,next_after})
        }).await
    }
}

#[cfg(test)]
mod bounds {
    use super::*;
    #[test]
    fn allocation_ui_page_byte_budget_retains_a_resumable_cursor() {
        let text = "\u{0001}".repeat(128);
        let mxid = "\u{0001}".repeat(255);
        let name = "\u{0001}".repeat(160);
        let mut items = Vec::new();
        let mut bytes = 512;
        let mut truncated = false;
        for n in 0..16 {
            let item:ProjectAllocationDetail=serde_json::from_value(serde_json::json!({
                "projectName":name,"resourceLabel":name,
                "allocation":{"id":format!("allocation_{n:02}"),"projectId":text,"projectRevision":1,"grantId":text,"requestedTokens":1,"state":"reserved","revision":3,"requestId":text,"requesterMxid":mxid,"coordinatorMxid":mxid,"periodKey":"2026-10","expiresAtMs":1,"canDecide":false,"canRelease":false,"blockingReason":text,"result":null,"balance":null,"observedUsage":null},
                "request":{"id":text,"kind":"project_allocation","contributionId":text,"projectId":text,"allocationId":text,"projectName":name,"resourceLabel":name,"requesterMxid":mxid,"approverMxid":mxid,"state":"approved","revision":2,"createdAtMs":1,"expiresAtMs":2,"decidedAtMs":1,"canDecide":false},
                "grant":{"id":text,"revision":1,"state":"active","ownerMxid":mxid,"coordinatorMxid":mxid,"totalTokens":1,"periodKey":"2026-10","periodStartMs":1,"periodEndMs":2,"delegationExpiresAtMs":2,"nativeAvailableTokens":null}
            })).unwrap();
            if !append_page(&mut items, &mut bytes, item).unwrap() {
                truncated = true;
                break;
            }
        }
        assert!(truncated && !items.is_empty() && items.len() < 16);
        let cursor = items.last().unwrap().allocation.id.clone();
        let length = items.len();
        let page = ProjectAllocationPage {
            items,
            next_after: Some(cursor.clone()),
        };
        let envelope = serde_json::json!({"allocations":page,"observedAtMs":i64::MAX});
        assert!(serde_json::to_vec(&envelope).unwrap().len() < 128 * 1024);
        assert_eq!(cursor, format!("allocation_{:02}", length - 1));
    }
}
