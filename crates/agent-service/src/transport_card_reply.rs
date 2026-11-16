//! One message per answer in plaintext Rooms. The executor's Agent device
//! already edited its run card into the final answer (an `m.replace` of the
//! card, same sender); recording that card as the reply keeps the server from
//! posting the answer a second time. The answer text is kept like any
//! plaintext reply. Fenced exactly like `submit_reply`: owner, device, lease
//! epoch, dispatch, execution, binding generation, fresh Matrix authority.
use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CardReply {
    pub dispatch_id: String,
    pub execution_id: String,
    pub body: String,
    /// The run card's original event; its final edit carries `body`.
    pub matrix_event_id: String,
}

/// Shape checks before any authority lookup.
pub(crate) fn check_card_reply(request: &CardReply, max_body: usize, f: &DeliveryFacts) -> Result<()> {
    key(&request.dispatch_id)?;
    key(&request.execution_id)?;
    matrix_id(&request.matrix_event_id, '$')?;
    message(&request.body, max_body)?;
    if f.encrypted {
        // An encrypted Room records its reply with `record_encrypted_reply`.
        return Err(Error::Conflict("encrypted_room_requires_client_crypto"));
    }
    Ok(())
}

impl TransportStore {
    pub async fn record_card_reply(
        &self,
        p: &Principal,
        reference: &LeaseRef,
        request: CardReply,
        f: &DeliveryFacts,
        now: i64,
    ) -> Result<ReplyIntent> {
        self.observe_event_authority(p, &request.dispatch_id, f, now).await?;
        check_card_reply(&request, self.limits.max_message_bytes, f)?;
        let digest = hash(&serde_json::to_string(&request).map_err(|_| Error::Invalid("invalid_reply"))?);
        let mut guard = self.db.lock().await?;
        let db = &mut *guard;
        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            let (now, _) = Self::authenticate(db, p, now).await?;
            let e = Self::dispatch(db, p, &request.dispatch_id).await?;
            let scope = Self::dispatch_scope(db, p, reference, &e, f, now).await?;
            fenced(&e, p, reference)?;
            if let Some(old) = Self::reply_for_event(db, &e.id).await? {
                if old.payload_digest != digest {
                    return Err(Error::Conflict("reply_payload_changed"));
                }
                return Ok(old);
            }
            if e.state != "running" || e.execution_id.as_deref() != Some(&request.execution_id) {
                return Err(Error::Conflict("execution_not_running"));
            }
            let id = format!("rep_{}", entity_id()?);
            let txn = format!("hagency_{}", hash(&e.id));
            sql_query("INSERT INTO hagency_agent_v1.reply_outbox(id,owner_event_id,agent_id,binding_id,owner_user_id,room_id,puppet_mxid,thread_root,body,payload_digest,matrix_txn_id,binding_generation,dispatch_epoch,delivery_epoch,state,matrix_event_id,created_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13,'sent',$14,$15)")
                .bind::<Text, _>(&id).bind::<Text, _>(&e.id).bind::<Text, _>(&e.agent_id).bind::<Text, _>(&e.binding_id)
                .bind::<Text, _>(&p.user_id).bind::<Text, _>(&e.room_id).bind::<Text, _>(&scope.puppet_mxid)
                .bind::<Text, _>(&e.thread_root).bind::<Text, _>(&request.body).bind::<Text, _>(&digest)
                .bind::<Text, _>(&txn).bind::<BigInt, _>(e.binding_generation).bind::<BigInt, _>(reference.epoch)
                .bind::<Text, _>(&request.matrix_event_id).bind::<BigInt, _>(now)
                .execute(db).await?;
            sql_query("UPDATE hagency_agent_v1.owner_events SET state='completed',outcome='replied' WHERE id=$1")
                .bind::<Text, _>(&e.id).execute(db).await?;
            Self::reply(db, &id).await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> CardReply {
        CardReply { dispatch_id: "dsp_1".into(), execution_id: "exe_1".into(), body: "42".into(), matrix_event_id: "$card".into() }
    }
    fn facts(encrypted: bool) -> DeliveryFacts {
        DeliveryFacts { owner_mxid: "@o:x".into(), requester_mxid: "@o:x".into(), puppet_mxid: "@a:x".into(),
            room_id: "!r:x".into(), space_id: String::new(), observed_at_ms: 0, owner_in_space: true,
            owner_in_room: true, room_in_space: true, requester_in_room: true, puppet_in_room: true,
            puppet_can_send_message: true, encrypted, owner_direct_valid: true, requires_mention: false }
    }
    #[test]
    fn a_card_reply_is_for_plaintext_rooms_with_a_real_card_event() {
        let plain = facts(false);
        check_card_reply(&request(), 1024, &plain).unwrap();
        let encrypted = facts(true);
        assert!(check_card_reply(&request(), 1024, &encrypted).is_err(), "encrypted Rooms use the encrypted record");
        let mut bad = request();
        bad.matrix_event_id = "card".into();
        assert!(check_card_reply(&bad, 1024, &plain).is_err());
        let mut empty = request();
        empty.body.clear();
        assert!(check_card_reply(&empty, 1024, &plain).is_err());
        assert!(check_card_reply(&request(), 1, &plain).is_err(), "the body limit still applies");
        let wire = serde_json::json!({"dispatchId":"d","executionId":"e","body":"b","matrixEventId":"$c","extra":1});
        assert!(serde_json::from_value::<CardReply>(wire).is_err(), "closed schema");
    }
}
