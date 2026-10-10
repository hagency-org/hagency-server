use crate::{Error, Result, entity_id, hash, identity::Identity, key, secret_token};
use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Nullable, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Store {
    db: Arc<Mutex<AsyncPgConnection>>,
    issuer: String,
}
#[derive(diesel::QueryableByName)]
struct User {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    issuer: String,
    #[diesel(sql_type=Text)]
    subject: String,
    #[diesel(sql_type=Text)]
    mxid: String,
    #[diesel(sql_type=Bool)]
    active: bool,
}
#[derive(diesel::QueryableByName)]
pub struct Principal {
    #[diesel(sql_type=Text)]
    pub user_id: String,
    #[diesel(sql_type=Text)]
    pub mxid: String,
    #[diesel(sql_type=Text)]
    pub subject: String,
    #[diesel(sql_type=Text)]
    pub session_id: String,
    #[diesel(sql_type=Text)]
    pub client_id: String,
    #[diesel(sql_type=BigInt)]
    pub valid_until_ms: i64,
    #[diesel(sql_type=Nullable<Text>)]
    pub device_id: Option<String>,
    #[diesel(sql_type=Nullable<BigInt>)]
    pub device_generation: Option<i64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGrant {
    pub token: String,
    pub user_id: String,
    pub mxid: String,
    pub valid_until_ms: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterDevice {
    pub installation_id: String,
    pub name: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceGrant {
    pub device_id: String,
    pub token: String,
    pub generation: i64,
    pub valid_until_ms: i64,
}
#[derive(diesel::QueryableByName)]
struct Device {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=BigInt)]
    generation: i64,
}
impl Store {
    pub async fn open(url: &str, server: &str, issuer: &str) -> Result<Self> {
        let mut db = AsyncPgConnection::establish(url)
            .await
            .map_err(|_| Error::Unavailable("database_unavailable"))?;
        db.transaction::<_,Error,_>(async move |db: &mut AsyncPgConnection| {
            // Serialize initialization across processes; never rewrite deployment identity.
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328902)").await?;
            #[derive(diesel::QueryableByName)] struct Exists { #[diesel(sql_type=Bool)] initialized:bool }
            let exists=sql_query("SELECT to_regclass('hagency_agent_v1.deployment') IS NOT NULL AS initialized").get_result::<Exists>(db).await?;
            if !exists.initialized {db.batch_execute(include_str!("schema.sql")).await?;}
            sql_query("INSERT INTO hagency_agent_v1.deployment(singleton,version,server_name,issuer) VALUES(true,1,$1,$2) ON CONFLICT(singleton) DO NOTHING")
                .bind::<Text,_>(server).bind::<Text,_>(issuer).execute(db).await?;
            #[derive(diesel::QueryableByName)] struct Match { #[diesel(sql_type=Bool)] matched:bool }
            let matched=sql_query("SELECT (version=1 AND server_name=$1 AND issuer=$2) AS matched FROM hagency_agent_v1.deployment WHERE singleton")
                .bind::<Text,_>(server).bind::<Text,_>(issuer).get_result::<Match>(db).await?;
            if !matched.matched { return Err(Error::Conflict("deployment_identity_mismatch")); }
            Ok(())
        }).await?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            issuer: issuer.into(),
        })
    }
    pub async fn sign_in(&self, identity: Identity, now: i64) -> Result<SessionGrant> {
        if identity.issuer != self.issuer || identity.valid_until_ms <= now {
            return Err(Error::Unauthorized("authentication_required"));
        }
        let mut db = self.db.lock().await;
        (*db).transaction::<_,Error,_>(async move |db: &mut AsyncPgConnection| {
            // Unique identity lock also serializes conflicting subject/MXID registrations.
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328903)").await?;
            if identity.valid_until_ms<=crate::api::now_ms() {return Err(Error::Unauthorized("authorization_expired"));}
            let users=sql_query("SELECT id,issuer,subject,mxid,active FROM hagency_agent_v1.users WHERE (issuer=$1 AND subject=$2) OR mxid=$3 FOR UPDATE")
                .bind::<Text,_>(&identity.issuer).bind::<Text,_>(&identity.subject).bind::<Text,_>(&identity.mxid).load::<User>(db).await?;
            let user_id=match users.as_slice() {
                []=> {
                    let id=format!("usr_{}",entity_id()?);
                    sql_query("INSERT INTO hagency_agent_v1.users(id,issuer,subject,mxid) VALUES($1,$2,$3,$4)")
                        .bind::<Text,_>(&id).bind::<Text,_>(&identity.issuer).bind::<Text,_>(&identity.subject).bind::<Text,_>(&identity.mxid).execute(db).await?;
                    id
                },
                [user] if user.issuer==identity.issuer && user.subject==identity.subject && user.mxid==identity.mxid => {
                    if !user.active { return Err(Error::Unauthorized("user_disabled")); } user.id.clone()
                },
                _=>return Err(Error::Conflict("identity_mapping_mismatch")),
            };
            let session_token=secret_token();
            sql_query("INSERT INTO hagency_agent_v1.sessions(id,user_id,token_hash,client_id,valid_until_ms) VALUES($1,$2,$3,$4,$5)")
                .bind::<Text,_>(format!("ses_{}",entity_id()?)).bind::<Text,_>(&user_id).bind::<Text,_>(hash(&session_token)).bind::<Text,_>(&identity.client_id).bind::<BigInt,_>(identity.valid_until_ms).execute(db).await?;
            Ok(SessionGrant {token:session_token,user_id,mxid:identity.mxid,valid_until_ms:identity.valid_until_ms})
        }).await
    }
    async fn principal(
        db: &mut AsyncPgConnection,
        credential: &str,
        now: i64,
        device: bool,
    ) -> Result<Principal> {
        if credential.len() != 64 {
            return Err(Error::Unauthorized("authentication_required"));
        }
        let query = if device {
            "SELECT u.id AS user_id,u.mxid,u.subject,s.id AS session_id,s.client_id,s.valid_until_ms,d.id AS device_id,d.generation AS device_generation FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id JOIN hagency_agent_v1.users u ON u.id=d.user_id AND u.id=s.user_id WHERE d.token_hash=$1 AND NOT d.revoked AND NOT s.revoked AND u.active AND s.valid_until_ms>greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint) FOR UPDATE OF s,d,u"
        } else {
            "SELECT u.id AS user_id,u.mxid,u.subject,s.id AS session_id,s.client_id,s.valid_until_ms,NULL::text AS device_id,NULL::bigint AS device_generation FROM hagency_agent_v1.sessions s JOIN hagency_agent_v1.users u ON u.id=s.user_id WHERE s.token_hash=$1 AND NOT s.revoked AND u.active AND s.valid_until_ms>greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint) FOR UPDATE OF s,u"
        };
        let principal = sql_query(query)
            .bind::<Text, _>(hash(credential))
            .bind::<BigInt, _>(now)
            .get_result::<Principal>(db)
            .await
            .map_err(|error| {
                if error == diesel::result::Error::NotFound {
                    Error::Unauthorized("authorization_expired")
                } else {
                    error.into()
                }
            })?;
        if principal.valid_until_ms <= now.max(crate::api::now_ms()) {
            return Err(Error::Unauthorized("authorization_expired"));
        }
        Ok(principal)
    }
    pub async fn authenticate(
        &self,
        credential: &str,
        now: i64,
        device: bool,
    ) -> Result<Principal> {
        Self::principal(&mut *self.db.lock().await, credential, now, device).await
    }
    pub async fn renew(&self, credential: &str, identity: Identity, now: i64) -> Result<i64> {
        if identity.issuer != self.issuer || identity.valid_until_ms <= now {
            return Err(Error::Unauthorized("authentication_required"));
        }
        let mut db = self.db.lock().await;
        (*db)
            .transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                // An expired session may renew only with fresh matching upstream proof.
                if identity.valid_until_ms<=crate::api::now_ms() {return Err(Error::Unauthorized("authorization_expired"));}
                let p=sql_query("SELECT u.id AS user_id,u.mxid,u.subject,s.id AS session_id,s.client_id,s.valid_until_ms,NULL::text AS device_id,NULL::bigint AS device_generation FROM hagency_agent_v1.sessions s JOIN hagency_agent_v1.users u ON u.id=s.user_id WHERE s.token_hash=$1 AND NOT s.revoked AND u.active FOR UPDATE OF s,u")
                    .bind::<Text,_>(hash(credential)).get_result::<Principal>(db).await
                    .map_err(|e|if e==diesel::result::Error::NotFound {Error::Unauthorized("authorization_expired")}else{e.into()})?;
                let u = sql_query(
                    "SELECT id,issuer,subject,mxid,active FROM hagency_agent_v1.users WHERE id=$1",
                )
                .bind::<Text, _>(&p.user_id)
                .get_result::<User>(db)
                .await?;
                if u.issuer != identity.issuer
                    || u.subject != identity.subject
                    || u.mxid != identity.mxid
                    || p.client_id != identity.client_id
                {
                    return Err(Error::Conflict("identity_mapping_mismatch"));
                }
                if identity.valid_until_ms <= crate::api::now_ms() {
                    return Err(Error::Unauthorized("authorization_expired"));
                }
                sql_query("UPDATE hagency_agent_v1.sessions SET valid_until_ms=$1 WHERE id=$2")
                    .bind::<BigInt, _>(identity.valid_until_ms)
                    .bind::<Text, _>(&p.session_id)
                    .execute(db)
                    .await?;
                Ok(identity.valid_until_ms)
            })
            .await
    }
    pub async fn register_device(
        &self,
        credential: &str,
        request: RegisterDevice,
        now: i64,
    ) -> Result<DeviceGrant> {
        key(&request.installation_id)?;
        if request.name.trim().is_empty()
            || request.name.chars().count() > 128
            || request.name.chars().any(char::is_control)
        {
            return Err(Error::Invalid("invalid_device_name"));
        }
        let mut db = self.db.lock().await;
        (*db).transaction::<_,Error,_>(async move |db: &mut AsyncPgConnection| {
            let p=Self::principal(db,credential,now,false).await?;
            let device_token=secret_token();
            // Same owner/install keeps stable id, rotates credential and fences prior generations.
            let device=sql_query("INSERT INTO hagency_agent_v1.devices(id,user_id,installation_id,name,session_id,token_hash,generation) VALUES($1,$2,$3,$4,$5,$6,1) ON CONFLICT(user_id,installation_id) DO UPDATE SET name=EXCLUDED.name,session_id=EXCLUDED.session_id,token_hash=EXCLUDED.token_hash,generation=hagency_agent_v1.devices.generation+1,revoked=false RETURNING id,generation")
                .bind::<Text,_>(format!("dev_{}",entity_id()?)).bind::<Text,_>(&p.user_id).bind::<Text,_>(&request.installation_id).bind::<Text,_>(request.name.trim()).bind::<Text,_>(&p.session_id).bind::<Text,_>(hash(&device_token)).get_result::<Device>(db).await?;
            Ok(DeviceGrant {device_id:device.id,token:device_token,generation:device.generation,valid_until_ms:p.valid_until_ms})
        }).await
    }
    pub async fn revoke_device(&self, credential: &str, id: &str, now: i64) -> Result<()> {
        key(id)?;
        let mut db = self.db.lock().await;
        (*db).transaction::<_,Error,_>(async move |db: &mut AsyncPgConnection| {
            let p=Self::principal(db,credential,now,false).await?;
            let changed=sql_query("UPDATE hagency_agent_v1.devices SET revoked=true,generation=generation+1 WHERE id=$1 AND user_id=$2")
                .bind::<Text,_>(id).bind::<Text,_>(&p.user_id).execute(db).await?;
            if changed!=1 { return Err(Error::Unauthorized("device_not_owned")); } Ok(())
        }).await
    }
    pub async fn sign_out(&self, credential: &str) -> Result<()> {
        // Idempotent even after expiry; revocation closes every device attached to this session.
        sql_query("UPDATE hagency_agent_v1.sessions SET revoked=true WHERE token_hash=$1")
            .bind::<Text, _>(hash(credential))
            .execute(&mut *self.db.lock().await)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(subject: &str, mxid: &str, expires: i64) -> Identity {
        Identity {
            issuer: "https://example.test/_pasion/".into(),
            subject: subject.into(),
            mxid: mxid.into(),
            client_id: "native-a".into(),
            valid_until_ms: expires,
        }
    }
    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn postgres_identity_device_expiry_rotation_and_revocation() {
        let url = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL")
            .expect("dedicated test database required");
        let store = Store::open(&url, "example.test", "https://example.test/_pasion/")
            .await
            .unwrap();
        let base = crate::api::now_ms();
        let suffix = secret_token();
        let subject = format!("subject-{suffix}");
        let mxid = format!("@alice_{suffix}:example.test");
        let s = store
            .sign_in(identity(&subject, &mxid, base + 40_000), base + 10_000)
            .await
            .unwrap();
        let device = store
            .register_device(
                &s.token,
                RegisterDevice {
                    installation_id: "workstation".into(),
                    name: "My client".into(),
                },
                base + 10_000,
            )
            .await
            .unwrap();
        crate::assert_entity_id(&s.user_id, "usr_");
        crate::assert_entity_id(&device.device_id, "dev_");
        let principal = store
            .authenticate(&s.token, base + 10_000, false)
            .await
            .unwrap();
        crate::assert_entity_id(&principal.session_id, "ses_");
        assert_eq!(s.token.len(), 64);
        assert_eq!(device.token.len(), 64);
        assert_eq!(
            store
                .authenticate(&device.token, base + 39_999, true)
                .await
                .unwrap()
                .user_id,
            s.user_id
        );
        assert!(
            store
                .authenticate(&device.token, base + 40_000, true)
                .await
                .is_err()
        );
        assert!(
            store
                .sign_in(
                    identity(&format!("other-{suffix}"), &mxid, base + 50_000),
                    base + 20_000
                )
                .await
                .is_err()
        );
        assert!(
            store
                .sign_in(
                    identity(
                        &subject,
                        &format!("@bob_{suffix}:example.test"),
                        base + 50_000
                    ),
                    base + 20_000
                )
                .await
                .is_err()
        );
        assert!(
            store
                .renew(
                    &s.token,
                    identity(&format!("other-{suffix}"), &mxid, base + 60_000),
                    base + 40_000
                )
                .await
                .is_err()
        );
        store
            .renew(
                &s.token,
                identity(&subject, &mxid, base + 60_000),
                base + 40_000,
            )
            .await
            .unwrap();
        assert!(
            store
                .authenticate(&device.token, base + 40_000, true)
                .await
                .is_ok()
        );
        let second = store
            .register_device(
                &s.token,
                RegisterDevice {
                    installation_id: "workstation".into(),
                    name: "My client".into(),
                },
                base + 40_000,
            )
            .await
            .unwrap();
        assert_eq!(device.device_id, second.device_id);
        assert_eq!(second.generation, device.generation + 1);
        assert!(
            store
                .authenticate(&device.token, base + 40_000, true)
                .await
                .is_err()
        );
        assert!(
            store
                .authenticate(&second.token, base + 40_000, true)
                .await
                .is_ok()
        );
        let reopened = Store::open(&url, "example.test", "https://example.test/_pasion/")
            .await
            .unwrap();
        assert!(
            reopened
                .authenticate(&second.token, base + 40_000, true)
                .await
                .is_ok()
        );
        assert!(
            Store::open(&url, "foreign.test", "https://example.test/_pasion/")
                .await
                .is_err()
        );
        let stranger_subject = format!("stranger-{suffix}");
        let stranger_mxid = format!("@stranger_{suffix}:example.test");
        let stranger = store
            .sign_in(
                identity(&stranger_subject, &stranger_mxid, base + 60_000),
                base + 40_000,
            )
            .await
            .unwrap();
        assert!(
            store
                .revoke_device(&stranger.token, &second.device_id, base + 40_000)
                .await
                .is_err()
        );
        let mut db = store.db.lock().await;
        assert!(
            sql_query("UPDATE hagency_agent_v1.users SET subject='reassigned' WHERE id=$1")
                .bind::<Text, _>(&s.user_id)
                .execute(&mut *db)
                .await
                .is_err()
        );
        assert!(
            sql_query("DELETE FROM hagency_agent_v1.users WHERE id=$1")
                .bind::<Text, _>(&s.user_id)
                .execute(&mut *db)
                .await
                .is_err()
        );
        #[derive(diesel::QueryableByName)]
        struct Hash {
            #[diesel(sql_type=Text)]
            token_hash: String,
        }
        let stored =
            sql_query("SELECT token_hash FROM hagency_agent_v1.sessions WHERE token_hash=$1")
                .bind::<Text, _>(hash(&s.token))
                .get_result::<Hash>(&mut *db)
                .await
                .unwrap();
        assert_ne!(stored.token_hash, s.token);
        drop(db);
        store.sign_out(&s.token).await.unwrap();
        assert!(
            store
                .authenticate(&second.token, base + 40_000, true)
                .await
                .is_err()
        );
        assert!(
            store
                .renew(
                    &s.token,
                    identity(&subject, &mxid, base + 70_000),
                    base + 40_000
                )
                .await
                .is_err()
        );
        store.sign_out(&s.token).await.unwrap();
    }
}
