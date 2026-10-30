//! Saved thread context and delivery evidence; never a substitute for live work authority.
use super::*;
use crate::management_allocation_types::allocation_encoding;
use crate::management_shared_context_types::*;

#[derive(QueryableByName)]
struct ContextRow {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    input_digest: String,
}
#[derive(QueryableByName)]
struct DeliveryRow {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    matrix_content: String,
}
#[derive(QueryableByName)]
struct Outbound {
    #[diesel(sql_type=Text)]
    content: String,
    #[diesel(sql_type=Text)]
    content_digest: String,
    #[diesel(sql_type=Text)]
    matrix_txn_id: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=Nullable<Text>)]
    matrix_event_id: Option<String>,
    #[diesel(sql_type=Nullable<BigInt>)]
    sent_at_ms: Option<i64>,
}
#[derive(QueryableByName)]
struct Number {
    #[diesel(sql_type=BigInt)]
    count: i64,
}

pub(in super::super) async fn register_mode(
    db: &mut AsyncPgConnection,
    policy: &ManagementPolicy,
    id: &str,
) -> Result<()> {
    let encrypted=sql_query("SELECT coalesce((payload::jsonb->>'encrypted')::boolean,false) AS matched FROM hagency_agent_v1.management_shared_work WHERE id=$1").bind::<Text,_>(id).get_result::<Flag>(db).await?.matched;
    sql_query("INSERT INTO hagency_agent_v1.management_shared_input_modes VALUES($1,$2)")
        .bind::<Text, _>(id)
        .bind::<Text, _>(if policy.shared_thread_context() && !encrypted {
            "thread_context_v1"
        } else {
            "plain_text_v1"
        })
        .execute(db)
        .await?;
    Ok(())
}
async fn mode(db: &mut AsyncPgConnection, id: &str) -> Result<String> {
    Ok(sql_query("SELECT mode AS payload FROM hagency_agent_v1.management_shared_input_modes WHERE work_id=$1")
        .bind::<Text,_>(id).get_result::<Json>(db).await.optional()?.map_or_else(||"plain_text_v1".into(),|r|r.payload))
}
pub(super) async fn require_plain(db: &mut AsyncPgConnection, id: &str) -> Result<()> {
    if mode(db, id).await? != "plain_text_v1" {
        return Err(Error::Conflict("shared_work_context_required"));
    }
    Ok(())
}
fn fresh(policy: &ManagementPolicy) -> Result<()> {
    if !policy.shared_thread_context() {
        return Err(Error::Unavailable(
            "shared_thread_context_issuance_unavailable",
        ));
    }
    Ok(())
}
async fn owned(
    db: &mut AsyncPgConnection,
    p: &Principal,
    id: &str,
    input: &SharedWorkEvidenceInput,
    server: &str,
) -> Result<(WorkRow, Runtime, SharedWorkItem)> {
    DomainStore::authorize(db, p, 0).await?;
    key(id)?;
    let w = work_row(db, id).await?;
    let r = runtime(db, &w.runtime_id).await?;
    owner(&original(db, &r).await?, p)?;
    command(&r, server)?;
    if w.digest != input.work_digest || r.digest != input.runtime_command_digest {
        return Err(Error::Conflict("shared_work_changed"));
    }
    let item = item(&w, &r)?;
    Ok((w, r, item))
}
async fn saved_context(
    db: &mut AsyncPgConnection,
    w: &SharedWorkItem,
) -> Result<Option<SharedContextRecord>> {
    let row=sql_query("SELECT payload,digest,input_digest FROM hagency_agent_v1.management_shared_contexts WHERE work_id=$1")
        .bind::<Text,_>(&w.id).get_result::<ContextRow>(db).await.optional()?;
    let Some(row) = row else { return Ok(None) };
    let c: SharedThreadContext = decode(&row.payload)?;
    if allocation_digest(&c)? != row.digest || hash(&render_context(&c, w)?) != row.input_digest {
        return Err(Error::Conflict("shared_context_changed"));
    }
    Ok(Some(SharedContextRecord {
        context: c,
        context_digest: row.digest,
        input_digest: row.input_digest,
    }))
}
pub(super) async fn saved_input_digest(
    db: &mut AsyncPgConnection,
    w: &SharedWorkItem,
) -> Result<String> {
    if mode(db, &w.id).await? == "plain_text_v1" {
        return Ok(w.body_digest.clone());
    }
    Ok(saved_context(db, w)
        .await?
        .ok_or(Error::Conflict("shared_context_required"))?
        .input_digest)
}
fn delivery_matches(d: &SharedReplyDelivery, w: &SharedWorkItem, content: &str) -> Result<()> {
    d.validate()?;
    let value: Value = decode(content)?;
    if d.work_id != w.id
        || d.work_digest != allocation_digest(w)?
        || d.assignment_id != w.assignment_id
        || d.runtime_command_digest != w.runtime_command_digest
        || d.room_id != w.room_id
        || d.thread_root != w.thread_root
        || d.source_event_id != w.event_id
        || hash(content) != d.matrix_content_digest
        // A rendering change cannot invalidate immutable delivery evidence.
        // Compare the exact closed content described by the receipt, including
        // its original optional HTML, while retaining the original byte hash.
        || value != d.content()
    {
        return Err(Error::Conflict("shared_reply_delivery_changed"));
    }
    Ok(())
}

#[cfg(test)]
mod delivery_format_tests {
    use super::*;

    #[test]
    fn shared_context_delivery_preserves_legacy_markdown_and_exact_rich_evidence() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/management-shared-thread-context-v1.json"
        )).unwrap();
        let w: SharedWorkItem = serde_json::from_value(fixture["previousWork"].clone()).unwrap();
        let mut d: SharedReplyDelivery = serde_json::from_value(fixture["delivery"].clone()).unwrap();
        d.body = "## Older answer\n\n**Markdown** remains unchanged.".into();
        d.body_digest = hash(&d.body);
        let legacy = serde_json::to_string(&d.content()).unwrap();
        d.matrix_content_digest = hash(&legacy);
        assert_ne!(d.content(), crate::workers::reply_content(false, &w.room_id, &w.thread_root, &d.body));
        delivery_matches(&d, &w, &legacy).unwrap();
        assert!(serde_json::to_value(&d).unwrap().get("formattedBody").is_none());
        let rich = crate::workers::reply_content(false, &w.room_id, &w.thread_root, &d.body);
        d.formatted_body = Some(rich["formatted_body"].as_str().unwrap().into());
        let rich = serde_json::to_string(&rich).unwrap();
        d.matrix_content_digest = hash(&rich);
        delivery_matches(&d, &w, &rich).unwrap();
        let mut changed = d.clone();
        changed.formatted_body.as_mut().unwrap().push('!');
        assert!(delivery_matches(&changed, &w, &rich).is_err());
        for field in ["body", "msgtype", "format", "m.relates_to", "unexpected"] {
            let mut content: Value = serde_json::from_str(&rich).unwrap();
            content[field] = json!("changed");
            let content = serde_json::to_string(&content).unwrap();
            let mut changed = d.clone();
            changed.matrix_content_digest = hash(&content);
            assert!(delivery_matches(&changed, &w, &content).is_err(), "{field}");
        }
    }
}
async fn delivery(
    db: &mut AsyncPgConnection,
    w: &SharedWorkItem,
) -> Result<SharedReplyObservation> {
    let old=sql_query("SELECT payload,digest,matrix_content FROM hagency_agent_v1.management_shared_reply_deliveries WHERE work_id=$1")
        .bind::<Text,_>(&w.id).get_result::<DeliveryRow>(db).await.optional()?;
    if let Some(old) = old {
        let receipt: SharedReplyDelivery = decode(&old.payload)?;
        delivery_matches(&receipt, w, &old.matrix_content)?;
        if allocation_digest(&receipt)? != old.digest {
            return Err(Error::Conflict("shared_reply_delivery_changed"));
        }
        return Ok(SharedReplyObservation {
            state: Some("sent".into()),
            delivery: Some(SharedDeliveryRecord {
                receipt,
                receipt_digest: old.digest,
            }),
        });
    }
    let out=sql_query("SELECT content,content_digest,matrix_txn_id,state,matrix_event_id,sent_at_ms FROM hagency_agent_v1.management_shared_replies WHERE work_id=$1")
        .bind::<Text,_>(&w.id).get_result::<Outbound>(db).await.optional()?;
    let Some(out) = out else {
        return Ok(SharedReplyObservation {
            state: None,
            delivery: None,
        });
    };
    if out.state != "sent" {
        return Ok(SharedReplyObservation {
            state: Some(out.state),
            delivery: None,
        });
    }
    let value: Value = decode(&out.content)?;
    let body = value["body"]
        .as_str()
        .ok_or(Error::Conflict("shared_reply_delivery_changed"))?
        .to_string();
    let receipt = SharedReplyDelivery {
        version: 1,
        work_id: w.id.clone(),
        work_digest: allocation_digest(w)?,
        assignment_id: w.assignment_id.clone(),
        runtime_command_digest: w.runtime_command_digest.clone(),
        room_id: w.room_id.clone(),
        thread_root: w.thread_root.clone(),
        source_event_id: w.event_id.clone(),
        matrix_txn_id: out.matrix_txn_id,
        matrix_event_id: out
            .matrix_event_id
            .ok_or(Error::Conflict("shared_reply_delivery_missing"))?,
        matrix_content_digest: out.content_digest,
        body_digest: hash(&body),
        body,
        formatted_body: value.get("formatted_body").and_then(Value::as_str).map(str::to_owned),
        sent_at_ms: out
            .sent_at_ms
            .ok_or(Error::Conflict("shared_reply_delivery_missing"))?,
    };
    delivery_matches(&receipt, w, &out.content)?;
    let digest = allocation_digest(&receipt)?;
    sql_query(
        "INSERT INTO hagency_agent_v1.management_shared_reply_deliveries VALUES($1,$2,$3,$4)",
    )
    .bind::<Text, _>(&w.id)
    .bind::<Text, _>(allocation_encoding(&receipt)?)
    .bind::<Text, _>(&digest)
    .bind::<Text, _>(&out.content)
    .execute(db)
    .await?;
    Ok(SharedReplyObservation {
        state: Some("sent".into()),
        delivery: Some(SharedDeliveryRecord {
            receipt,
            receipt_digest: digest,
        }),
    })
}
async fn freeze(db: &mut AsyncPgConnection, w: &SharedWorkItem) -> Result<SharedContextRecord> {
    if let Some(saved) = saved_context(db, w).await? {
        return Ok(saved);
    }
    let ordinal=sql_query("INSERT INTO hagency_agent_v1.management_shared_context_sequences VALUES($1,$2,$3,1) ON CONFLICT(assignment_id,room_id,thread_root) DO UPDATE SET ordinal=hagency_agent_v1.management_shared_context_sequences.ordinal+1 RETURNING ordinal AS count")
        .bind::<Text,_>(&w.assignment_id).bind::<Text,_>(&w.room_id).bind::<Text,_>(&w.thread_root).get_result::<Number>(db).await?.count;
    let binding = sql_query(
        "SELECT demand_id AS id FROM hagency_agent_v1.management_shared_demand_bindings WHERE work_id=$1 UNION ALL SELECT demand_id AS id FROM hagency_agent_v1.management_shared_budget_demand_bindings WHERE work_id=$1",
    )
    .bind::<Text, _>(&w.id)
    .get_result::<Id>(db)
    .await
    .optional()?;
    let (demand_binding_digest, control_revision) = if let Some(binding) = binding {
        let b = if let Some(b) = demands::binding(db, &binding.id).await? {
            Some(b)
        } else {
            demands::budget::binding(db, &binding.id).await?
        }
        .ok_or(Error::Conflict("shared_context_binding_changed"))?;
        if b.work_id != w.id
            || b.work_digest != allocation_digest(w)?
            || b.assignment_id != w.assignment_id
            || b.runtime_command_digest != w.runtime_command_digest
            || b.event_id != w.event_id
            || b.received_at_ms != w.created_at_ms
        {
            return Err(Error::Conflict("shared_context_binding_changed"));
        }
        (Some(allocation_digest(&b)?), Some(b.control_revision))
    } else {
        (None, None)
    };
    let rows=sql_query("SELECT w.* FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_replies x ON x.work_id=w.id WHERE x.state='sent' AND w.payload::jsonb->>'assignmentId'=$1 AND w.payload::jsonb->>'roomId'=$2 AND w.payload::jsonb->>'threadRoot'=$3 AND (w.created_at_ms,w.id)<($4,$5) ORDER BY w.created_at_ms DESC,w.id DESC LIMIT 9")
        .bind::<Text,_>(&w.assignment_id).bind::<Text,_>(&w.room_id).bind::<Text,_>(&w.thread_root).bind::<BigInt,_>(w.created_at_ms).bind::<Text,_>(&w.id).load::<WorkRow>(db).await?;
    let mut c = SharedThreadContext {
        version: 1,
        input_profile: "shared_thread_text_v1".into(),
        work_id: w.id.clone(),
        work_digest: allocation_digest(w)?,
        assignment_id: w.assignment_id.clone(),
        runtime_command_digest: w.runtime_command_digest.clone(),
        demand_binding_digest,
        control_revision,
        room_id: w.room_id.clone(),
        thread_root: w.thread_root.clone(),
        event_id: w.event_id.clone(),
        received_at_ms: w.created_at_ms,
        context_ordinal: ordinal,
        coverage: "prior_delivered_pairs_only".into(),
        omitted_earlier: false,
        history: vec![],
    };
    let total = rows.len();
    for old in rows.into_iter().take(MAX_HISTORY_PAIRS) {
        let r = runtime(db, &old.runtime_id).await?;
        let old = item(&old, &r)?;
        let sent = delivery(db, &old)
            .await?
            .delivery
            .ok_or(Error::Conflict("shared_reply_delivery_missing"))?;
        c.history.insert(
            0,
            SharedContextPair {
                work_id: old.id.clone(),
                work_digest: allocation_digest(&old)?,
                event_id: old.event_id,
                requester_mxid: old.requester_mxid,
                received_at_ms: old.created_at_ms,
                user_body: old.body,
                user_body_digest: old.body_digest,
                delivery: sent.receipt,
                delivery_digest: sent.receipt_digest,
            },
        );
        if allocation_encoding(&c.history)?.len() > MAX_HISTORY_BYTES {
            c.history.remove(0);
            break;
        }
        match render_context(&c, w) {
            Ok(_) => {}
            Err(Error::Invalid("shared_context_input_too_large")) => {
                c.history.remove(0);
                break;
            }
            Err(error) => return Err(error),
        }
    }
    c.omitted_earlier = total > c.history.len();
    let input_digest = hash(&render_context(&c, w)?);
    let digest = allocation_digest(&c)?;
    sql_query(
        "INSERT INTO hagency_agent_v1.management_shared_contexts VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind::<Text, _>(&w.id)
    .bind::<Text, _>(&w.assignment_id)
    .bind::<Text, _>(&w.room_id)
    .bind::<Text, _>(&w.thread_root)
    .bind::<BigInt, _>(ordinal)
    .bind::<Text, _>(allocation_encoding(&c)?)
    .bind::<Text, _>(&digest)
    .bind::<Text, _>(&input_digest)
    .execute(db)
    .await?;
    Ok(SharedContextRecord {
        context: c,
        context_digest: digest,
        input_digest,
    })
}
impl DomainStore {
    /// Descriptive preflight, before a native call hold or start. Policy-off reads remain available.
    pub async fn shared_work_input_mode(
        &self,
        p: &Principal,
        id: &str,
        input: SharedWorkEvidenceInput,
    ) -> Result<SharedWorkInputMode> {
        self.shared_work_input_mode_with_protocol(p, id, input, WorkProtocol::Legacy)
            .await
    }
    pub async fn shared_budget_work_input_mode(
        &self,
        p: &Principal,
        id: &str,
        input: SharedWorkEvidenceInput,
    ) -> Result<SharedWorkInputMode> {
        self.shared_work_input_mode_with_protocol(p, id, input, WorkProtocol::Budget)
            .await
    }
    async fn shared_work_input_mode_with_protocol(
        &self,
        p: &Principal,
        id: &str,
        input: SharedWorkEvidenceInput,
        protocol: WorkProtocol,
    ) -> Result<SharedWorkInputMode> {
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                let (_, r, w) = owned(db, p, id, &input, &self.server).await?;
                protocol.check(&r)?;
                let mode = mode(db, id).await?;
                let saved = saved_context(db, &w).await?;
                Ok(SharedWorkInputMode {
                    mode,
                    context_digest: saved.as_ref().map(|s| s.context_digest.clone()),
                    input_digest: saved.map(|s| s.input_digest),
                })
            })
            .await
    }
    pub async fn observe_shared_reply(
        &self,
        p: &Principal,
        id: &str,
        input: SharedWorkEvidenceInput,
    ) -> Result<SharedReplyObservation> {
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                    .await?;
                let (_, _, w) = owned(db, p, id, &input, &self.server).await?;
                delivery(db, &w).await
            })
            .await
    }
    pub async fn recover_shared_context(
        &self,
        p: &Principal,
        id: &str,
        input: SharedContextRecoverInput,
    ) -> Result<SharedContextRecord> {
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                let (_, _, w) = owned(
                    db,
                    p,
                    id,
                    &SharedWorkEvidenceInput {
                        work_digest: input.work_digest,
                        runtime_command_digest: input.runtime_command_digest,
                    },
                    &self.server,
                )
                .await?;
                let saved = saved_context(db, &w)
                    .await?
                    .ok_or(Error::NotFound("shared_context_missing"))?;
                if saved.context_digest != input.context_digest {
                    return Err(Error::Conflict("shared_context_changed"));
                }
                Ok(saved)
            })
            .await
    }
    pub async fn freeze_shared_context(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedContextInput,
        gateway: &Gateway,
    ) -> Result<SharedContextRecord> {
        self.freeze_shared_context_with_protocol(
            p,
            policy,
            id,
            input,
            gateway,
            WorkProtocol::Legacy,
        )
        .await
    }
    pub async fn freeze_shared_budget_context(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedContextInput,
        gateway: &Gateway,
    ) -> Result<SharedContextRecord> {
        self.freeze_shared_context_with_protocol(
            p,
            policy,
            id,
            input,
            gateway,
            WorkProtocol::Budget,
        )
        .await
    }
    async fn freeze_shared_context_with_protocol(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedContextInput,
        gateway: &Gateway,
        protocol: WorkProtocol,
    ) -> Result<SharedContextRecord> {
        key(id)?;
        fresh(policy)?;
        let observed = self
            .shared_work_facts(p, policy, &input.authority, gateway, protocol)
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                    .await?;
                Self::authorize(db, p, 0).await?;
                let r = runtime(db, &input.authority.runtime_command_id).await?;
                protocol.check(&r)?;
                require_budget_head(db, &r, &self.server).await?;
                let now = clock(db).await?;
                if !runtime_issuing(db, policy, &r).await? {
                    return Err(Error::Unavailable("shared_execution_issuance_unavailable"));
                }
                require_activity(db, p, &r, &input.authority, &self.server, now).await?;
                facts(&membership(db, &r).await?, &observed)?;
                let row = work_row(db, id).await?;
                let w = item(&row, &r)?;
                if row.digest != input.work_digest || mode(db, id).await? != "thread_context_v1" {
                    return Err(Error::Conflict("shared_context_binding_changed"));
                }
                if !matches!(row.state.as_str(), "queued" | "acknowledged")
                    && saved_context(db, &w).await?.is_none()
                {
                    return Err(Error::Conflict("shared_context_missing_before_start"));
                }
                freeze(db, &w).await
            })
            .await
    }
    pub async fn start_shared_context_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedContextStartInput,
        gateway: &Gateway,
    ) -> Result<SharedContextStart> {
        self.start_shared_context_work_with_protocol(
            p,
            policy,
            id,
            input,
            gateway,
            WorkProtocol::Legacy,
        )
        .await
    }
    pub async fn start_shared_budget_context_work(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedContextStartInput,
        gateway: &Gateway,
    ) -> Result<SharedContextStart> {
        self.start_shared_context_work_with_protocol(
            p,
            policy,
            id,
            input,
            gateway,
            WorkProtocol::Budget,
        )
        .await
    }
    async fn start_shared_context_work_with_protocol(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: SharedContextStartInput,
        gateway: &Gateway,
        protocol: WorkProtocol,
    ) -> Result<SharedContextStart> {
        key(id)?;
        fresh(policy)?;
        let observed = self
            .shared_work_facts(p, policy, &input.authority, gateway, protocol)
            .await?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            Self::authorize(db,p,0).await?;let r=runtime(db,&input.authority.runtime_command_id).await?;protocol.check(&r)?;require_budget_head(db,&r,&self.server).await?;let now=clock(db).await?;
            if !runtime_issuing(db,policy,&r).await?{return Err(Error::Unavailable("shared_execution_issuance_unavailable"));}
            let original=require_activity(db,p,&r,&input.authority,&self.server,now).await?;facts(&membership(db,&r).await?,&observed)?;
            let row=work_row(db,id).await?;let w=item(&row,&r)?;
            if row.digest!=input.work_digest || mode(db,id).await?!="thread_context_v1" {return Err(Error::Conflict("shared_context_binding_changed"));}
            refusals::ensure_unfenced(db,id).await?;let saved=saved_context(db,&w).await?.ok_or(Error::Conflict("shared_context_required"))?;
            if saved.context_digest!=input.context_digest || saved.input_digest!=input.input_digest {return Err(Error::Conflict("shared_context_changed"));}
            if row.state=="queued" {return Err(Error::Conflict("shared_work_not_acknowledged"));}
            let newly=row.state=="acknowledged";
            if newly {
                let busy=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_work WHERE runtime_id=$1 AND (state IN('started','unknown') OR (started_at_ms IS NOT NULL AND NOT usage_complete))) AS matched").bind::<Text,_>(&r.id).get_result::<Flag>(db).await?.matched;
                if busy{return Err(Error::Conflict("shared_work_unsettled"));}
                let exhausted=sql_query("SELECT coalesce(sum(w.known_consumed_tokens),0)>=$2 AS matched FROM hagency_agent_v1.management_shared_work w JOIN hagency_agent_v1.management_shared_runtimes r ON r.id=w.runtime_id WHERE r.assignment_id=$1").bind::<Text,_>(&w.assignment_id).bind::<BigInt,_>(original.execution_budget.as_ref().map_or(original.reserve_command.requested_tokens,|b|b.effective_ceiling_tokens)).get_result::<Flag>(db).await?.matched;
                if exhausted{return Err(Error::Conflict("shared_budget_exhausted"));}
                sql_query("INSERT INTO hagency_agent_v1.management_shared_context_starts VALUES($1,$2,$3,$4)").bind::<Text,_>(id).bind::<Text,_>(&saved.context_digest).bind::<Text,_>(&saved.input_digest).bind::<BigInt,_>(input.authority.epoch).execute(db).await?;
                sql_query("UPDATE hagency_agent_v1.management_shared_work SET state='started',started_at_ms=$2,start_epoch=$3 WHERE id=$1").bind::<Text,_>(id).bind::<BigInt,_>(now).bind::<BigInt,_>(input.authority.epoch).execute(db).await?;
            }else if row.started_at_ms.is_none(){return Err(Error::Conflict("shared_work_not_started"));}
            Ok(SharedContextStart{start:SharedWorkStart{work:w,work_digest:row.digest,activity_epoch:input.authority.epoch,newly_started:newly},context_digest:saved.context_digest,input_digest:saved.input_digest})
        }).await
    }
}
