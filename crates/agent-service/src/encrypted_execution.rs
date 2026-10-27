//! Encrypted owner-conversation execution and original ciphertext publication.
use super::*;
pub(crate) const CAPABILITY: &str = "agent-encrypted-owner-execution-v1";
use diesel::sql_types::Jsonb;
use serde_json::{Value, json};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Action {
    pub lease: LeaseRef,
    pub dispatch_id: String,
    pub execution_id: String,
    pub action: String,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub payload: Option<Value>,
}
impl Action {
    pub(crate) fn clone_for_check(&self) -> Self {
        Self {
            lease: self.lease.clone(),
            dispatch_id: self.dispatch_id.clone(),
            execution_id: self.execution_id.clone(),
            action: "authorize".into(),
            request_id: None,
            kind: None,
            payload: None,
        }
    }
    pub(crate) fn status(&self) -> Self {
        Self {
            action: "status".into(),
            request_id: self.request_id.clone(),
            kind: self.kind.clone(),
            ..self.clone_for_check()
        }
    }
}
#[derive(diesel::QueryableByName, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Publication {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub dispatch_id: String,
    #[diesel(sql_type=Text)]
    pub execution_id: String,
    #[diesel(sql_type=Text)]
    pub request_id: String,
    #[diesel(sql_type=Text)]
    pub kind: String,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Nullable<Text>)]
    pub matrix_event_id: Option<String>,
}
const SELECT: &str = "SELECT id,dispatch_id,execution_id,request_id,kind,state,matrix_event_id FROM hagency_agent_v1.encrypted_publications";
pub(crate) fn validate_payload(payload: &Value, owner: &str) -> Result<()> {
    let bad = || Error::Invalid("invalid_encrypted_publication");
    let o = payload
        .as_object()
        .filter(|o| o.len() == 2 && o.contains_key("content") && o.contains_key("shares"))
        .ok_or_else(bad)?;
    let c = o["content"].as_object().ok_or_else(bad)?;
    if c.keys().any(|k| {
        !matches!(
            k.as_str(),
            "algorithm" | "ciphertext" | "session_id" | "sender_key" | "device_id" | "m.relates_to"
        )
    }) || c.get("algorithm") != Some(&json!("m.megolm.v1.aes-sha2"))
        || c.get("ciphertext")
            .and_then(Value::as_str)
            .is_none_or(|s| s.is_empty() || s.len() > 100000)
    {
        return Err(bad());
    }
    let shares = o["shares"]
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or_else(bad)?;
    for share in shares {
        let s = share.as_object().filter(|o| o.len() == 3).ok_or_else(bad)?;
        if s.get("eventType") != Some(&json!("m.room.encrypted")) {
            return Err(bad());
        }
        key(s.get("txnId").and_then(Value::as_str).ok_or_else(bad)?)?;
        let users = s
            .get("messages")
            .and_then(Value::as_object)
            .filter(|o| o.len() == 1 && o.contains_key(owner))
            .ok_or_else(bad)?;
        let devices = users[owner]
            .as_object()
            .filter(|o| !o.is_empty() && o.len() <= 64)
            .ok_or_else(bad)?;
        for (id, v) in devices {
            if id.is_empty()
                || id.len() > 255
                || id.chars().any(char::is_control)
                || v.as_object().is_none_or(|o| {
                    !(3..=4).contains(&o.len())
                        || o.keys().any(|k| {
                            !matches!(
                                k.as_str(),
                                "algorithm" | "sender_key" | "ciphertext" | "org.matrix.msgid"
                            )
                        })
                })
                || v["algorithm"] != "m.olm.v1.curve25519-aes-sha2"
                || v["sender_key"].as_str().is_none()
                || v.get("org.matrix.msgid").is_some_and(|v| {
                    v.as_str().is_none_or(|s| {
                        s.is_empty() || s.len() > 128 || s.chars().any(char::is_control)
                    })
                })
            {
                return Err(bad());
            }
            let ciphertext = v["ciphertext"]
                .as_object()
                .filter(|o| o.len() == 1)
                .ok_or_else(bad)?;
            for (_, sealed) in ciphertext {
                if sealed.as_object().is_none_or(|o| o.len() != 2)
                    || !matches!(sealed["type"].as_u64(), Some(0 | 1))
                    || sealed["body"]
                        .as_str()
                        .is_none_or(|s| s.is_empty() || s.len() > 100000)
                {
                    return Err(bad());
                }
            }
        }
    }
    if serde_json::to_vec(payload).map_err(|_| bad())?.len() > 131072 {
        return Err(bad());
    }
    Ok(())
}
impl TransportStore {
    pub(crate) async fn encrypted_action(
        &self,
        p: &Principal,
        b: &Action,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<(Value, bool)> {
        key(&b.dispatch_id)?;
        key(&b.execution_id)?;
        if !matches!(
            b.action.as_str(),
            "start" | "authorize" | "finish" | "publish" | "status"
        ) {
            return Err(Error::Invalid("invalid_encrypted_action"));
        }
        if b.action != "publish" && b.payload.is_some()
            || !matches!(b.action.as_str(), "publish" | "status")
                && (b.request_id.is_some() || b.kind.is_some())
        {
            return Err(Error::Invalid("invalid_encrypted_action_fields"));
        }
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
   let(now,_)=Self::authenticate(db,p,now).await?;
   let e=Self::dispatch(db,p,&b.dispatch_id).await?;
   let scope=Self::scope(db,&e.binding_id).await?;
   if !e.state.starts_with("encrypted_")||e.agent_id!=b.lease.agent_id||e.binding_generation!=scope.binding_generation||scope.owner_user_id!=p.user_id||!scope.space_id.is_empty()||e.requester_mxid!=p.mxid{return Err(Error::Unauthorized("encrypted_execution_scope_revoked"));}
   facts_mode(&scope,&e.requester_mxid,f,now,true)?;
   if b.action=="status" {
    let publication=sql_query(format!("{SELECT} WHERE dispatch_id=$1 AND execution_id=$2 AND request_id=$3 AND kind=$4")).bind::<Text,_>(&e.id).bind::<Text,_>(&b.execution_id).bind::<Text,_>(b.request_id.as_deref().unwrap_or("")).bind::<Text,_>(b.kind.as_deref().unwrap_or("")).get_result::<Publication>(db).await.optional()?;
    return Ok((json!({"publication":publication}),false));
   }
   Self::lease(db,p,&b.lease,now).await?;fenced(&e,p,&b.lease)?;
   if b.action=="start" {
    if e.state=="encrypted_running"&&e.execution_id.as_deref()==Some(&b.execution_id){return Ok((json!({"dispatch":e,"newlyStarted":false}),false));}
    Self::event_unexpired(db,&e.id,self.limits.event_ttl_ms,now).await?;
    if e.state!="encrypted_acknowledged"||e.execution_id.is_some(){return Err(Error::Conflict("encrypted_dispatch_not_acknowledged"));}
    let reused=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.owner_events WHERE agent_id=$1 AND execution_id=$2) AS matched").bind::<Text,_>(&e.agent_id).bind::<Text,_>(&b.execution_id).get_result::<Flag>(db).await?.matched;
    if reused{return Err(Error::Conflict("execution_id_reused"));}
    sql_query("UPDATE hagency_agent_v1.owner_events SET state='encrypted_running',execution_id=$1 WHERE id=$2").bind::<Text,_>(&b.execution_id).bind::<Text,_>(&e.id).execute(db).await?;
    let current=Self::clock(db,now).await?;Self::lease(db,p,&b.lease,current).await?;
    return Ok((json!({"dispatch":Self::dispatch(db,p,&e.id).await?,"newlyStarted":true}),false));
   }
   if e.execution_id.as_deref()!=Some(&b.execution_id){return Err(Error::Unauthorized("encrypted_execution_changed"));}
   if b.action=="finish" {
    if e.state=="encrypted_running"{sql_query("UPDATE hagency_agent_v1.owner_events SET state='encrypted_unknown',outcome='host_stopped' WHERE id=$1").bind::<Text,_>(&e.id).execute(db).await?;}
    return Ok((json!({"finished":true}),false));
   }
   if b.action=="authorize" {
    if e.state!="encrypted_running"{return Err(Error::Conflict("execution_not_running"));}
    Self::event_unexpired(db,&e.id,self.limits.event_ttl_ms,now).await?;
    let current=Self::clock(db,now).await?;Self::lease(db,p,&b.lease,current).await?;
    return Ok((json!({"dispatch":e}),false));
   }
   let request=b.request_id.as_deref().ok_or(Error::Invalid("encrypted_publication_identity_required"))?;key(request)?;
   let kind=b.kind.as_deref().ok_or(Error::Invalid("encrypted_publication_kind_required"))?;
   if !matches!(kind,"reply"|"file")||kind=="reply"&&request!="reply"{return Err(Error::Invalid("invalid_encrypted_publication_kind"));}
   let payload=b.payload.as_ref().ok_or(Error::Invalid("encrypted_payload_required"))?;validate_payload(payload,&p.mxid)?;
   let old=sql_query(format!("{SELECT} WHERE dispatch_id=$1 AND kind=$2 AND request_id=$3")).bind::<Text,_>(&e.id).bind::<Text,_>(kind).bind::<Text,_>(request).get_result::<Publication>(db).await.optional()?;
   if let Some(old)=old{
    #[derive(diesel::QueryableByName)]struct Payload{#[diesel(sql_type=Jsonb)]payload:Value}
    let original=sql_query("SELECT payload FROM hagency_agent_v1.encrypted_publications WHERE id=$1").bind::<Text,_>(&old.id).get_result::<Payload>(db).await?;
    if original.payload!=*payload||old.execution_id!=b.execution_id{return Err(Error::Conflict("encrypted_publication_changed"));}
    return Ok((json!({"publication":old}),false));
   }
   if e.state!="encrypted_running"{return Err(Error::Conflict("execution_not_running"));}
   let id=format!("encpub_{}",entity_id()?);
   sql_query("INSERT INTO hagency_agent_v1.encrypted_publications(id,dispatch_id,execution_id,request_id,kind,payload,state,created_at_ms) VALUES($1,$2,$3,$4,$5,$6,'possible',$7)").bind::<Text,_>(&id).bind::<Text,_>(&e.id).bind::<Text,_>(&b.execution_id).bind::<Text,_>(request).bind::<Text,_>(kind).bind::<Jsonb,_>(payload).bind::<BigInt,_>(now).execute(db).await?;
   let current=Self::clock(db,now).await?;Self::lease(db,p,&b.lease,current).await?;
   let publication=sql_query(format!("{SELECT} WHERE id=$1")).bind::<Text,_>(&id).get_result::<Publication>(db).await?;
   Ok((json!({"publication":publication,"room":scope.room_id,"puppet":scope.puppet_mxid}),true))
  }).await
    }
    pub(crate) async fn encrypted_sent(&self, id: &str, event: &str) -> Result<()> {
        matrix_id(event, '$')?;
        let mut db = self.db.lock().await?;
        (*db).transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
   sql_query("UPDATE hagency_agent_v1.encrypted_publications SET state='sent',matrix_event_id=$2 WHERE id=$1 AND state='possible'").bind::<Text,_>(id).bind::<Text,_>(event).execute(db).await?;
   sql_query("UPDATE hagency_agent_v1.owner_events SET state='encrypted_completed',outcome='replied' WHERE id=(SELECT dispatch_id FROM hagency_agent_v1.encrypted_publications WHERE id=$1 AND kind='reply' AND matrix_event_id=$2) AND state IN ('encrypted_running','encrypted_unknown')").bind::<Text,_>(id).bind::<Text,_>(event).execute(db).await?;Ok(())
  }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_publication_accepts_actual_sdk_message_id_but_no_plaintext_or_foreign_recipient() {
        let owner = "@owner:test";
        let mut p = json!({"content":{"algorithm":"m.megolm.v1.aes-sha2","session_id":"session","ciphertext":"sealed"},"shares":[{"eventType":"m.room.encrypted","txnId":"original","messages":{owner:{"SIGNED":{"algorithm":"m.olm.v1.curve25519-aes-sha2","sender_key":"key","org.matrix.msgid":"original_sdk_message","ciphertext":{"recipient":{"body":"sealed","type":0}}}}}}]});
        assert!(validate_payload(&p, owner).is_ok());
        p["shares"][0]["messages"][owner]["SIGNED"]["body"] = json!("plaintext");
        assert!(validate_payload(&p, owner).is_err());
        p["shares"][0]["messages"][owner]["SIGNED"]
            .as_object_mut()
            .unwrap()
            .remove("body");
        assert!(validate_payload(&p, "@foreign:test").is_err());
    }
}
