use crate::{Error, Result, entity_id, hash, identity::Identity, key, secret_token};
use diesel::{BoolExpressionMethods, ExpressionMethods, QueryDsl, SelectableHelper};
use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Nullable, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct Store {
    db: Arc<crate::database::Database>,
    issuer: String,
}
#[derive(diesel::Queryable, diesel::QueryableByName, diesel::Selectable)]
#[diesel(table_name = crate::schema::agent_tables::users)]
#[diesel(check_for_backend(diesel::pg::Pg))]
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
#[derive(diesel::Queryable, diesel::QueryableByName)]
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
#[derive(diesel::Queryable, diesel::QueryableByName, diesel::Selectable)]
#[diesel(table_name = crate::schema::agent_tables::devices)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct Device {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=BigInt)]
    generation: i64,
}
impl Store {
    /// Explicit, one-way rename of this deployment's embedded auth mount.
    /// Stable user IDs retain Agent ownership; old sessions require a new login.
    pub async fn migrate_auth_path(url: &str, server: &str, issuer: &str) -> Result<bool> {
        let parsed = url::Url::parse(issuer).map_err(|_| Error::Invalid("invalid_issuer"))?;
        if parsed.path() != "/auth/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(Error::Invalid("invalid_issuer"));
        }
        let legacy = parsed
            .join("/_pasion/")
            .map_err(|_| Error::Invalid("invalid_issuer"))?
            .to_string();
        let mut db = AsyncPgConnection::establish(url)
            .await
            .map_err(|_| Error::Unavailable("database_unavailable"))?;
        db.transaction::<_,Error,_>(async move |db: &mut AsyncPgConnection| {
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328902); SELECT pg_advisory_xact_lock(5210750088328903)").await?;
            db.batch_execute("LOCK TABLE hagency_agent_v1.deployment, hagency_agent_v1.users, hagency_agent_v1.sessions, hagency_agent_v1.devices IN ACCESS EXCLUSIVE MODE").await?;
            #[derive(diesel::Queryable, diesel::QueryableByName)]
            struct Deployment {
                #[diesel(sql_type=Text)] issuer: String,
                #[diesel(sql_type=Bool)] same_server: bool,
            }
            let deployment={
                use crate::schema::agent_tables::deployment as q;
                let params = (server,);
                q::table
                    .select((
                        q::issuer,
                        (((q::version).eq(1)).and((q::server_name).eq(&params.0))),
                    ))
                    .filter(q::singleton)
                    .get_result::<Deployment>(db)
                    .await
            }?;
            if !deployment.same_server || (deployment.issuer != issuer && deployment.issuer != legacy) {
                return Err(Error::Conflict("deployment_identity_mismatch"));
            }
            if deployment.issuer == issuer { return Ok(false); }
            #[derive(diesel::Queryable, diesel::QueryableByName)]
            struct Consistent { #[diesel(sql_type=Bool)] consistent: bool }
            let users=sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.users WHERE issuer<>$1) AS consistent")
                .bind::<Text,_>(&legacy).get_result::<Consistent>(db).await?;
            if !users.consistent { return Err(Error::Conflict("identity_mapping_mismatch")); }
            db.batch_execute("UPDATE hagency_agent_v1.sessions SET revoked=true; UPDATE hagency_agent_v1.devices SET revoked=true; DROP TRIGGER retain_identity ON hagency_agent_v1.users").await?;
            {
                use crate::schema::agent_tables::users as q;
                let params = (issuer,);
                diesel::update(q::table)
                    .set((q::issuer.eq(&params.0),))
                    .execute(db)
                    .await
            }?;
            {
                use crate::schema::agent_tables::deployment as q;
                let params = (issuer,);
                diesel::update(q::table)
                    .filter(q::singleton)
                    .set((q::issuer.eq(&params.0),))
                    .execute(db)
                    .await
            }?;
            db.batch_execute("CREATE TRIGGER retain_identity BEFORE UPDATE OR DELETE ON hagency_agent_v1.users FOR EACH ROW EXECUTE FUNCTION hagency_agent_v1.retain_identity()").await?;
            Ok(true)
        }).await
    }

    pub async fn open(url: &str, server: &str, issuer: &str) -> Result<Self> {
        let mut db = AsyncPgConnection::establish(url)
            .await
            .map_err(|_| Error::Unavailable("database_unavailable"))?;
        db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
            // Serialize initialization across processes; never rewrite deployment identity.
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328902)")
                .await?;
            #[derive(diesel::Queryable, diesel::QueryableByName)]
            struct Exists {
                #[diesel(sql_type=Bool)]
                initialized: bool,
            }
            let exists = sql_query(
                "SELECT to_regclass('hagency_agent_v1.deployment') IS NOT NULL AS initialized",
            )
            .get_result::<Exists>(db)
            .await?;
            if !exists.initialized {
                db.batch_execute(include_str!("schema.sql")).await?;
            }
            {
                use crate::schema::agent_tables::deployment as q;
                let params = (server, issuer);
                diesel::insert_into(q::table)
                    .values((
                        q::singleton.eq(true),
                        q::version.eq(1),
                        q::server_name.eq(&params.0),
                        q::issuer.eq(&params.1),
                    ))
                    .on_conflict((q::singleton,))
                    .do_nothing()
                    .execute(db)
                    .await
            }?;
            #[derive(diesel::Queryable, diesel::QueryableByName)]
            struct Match {
                #[diesel(sql_type=Bool)]
                matched: bool,
            }
            let matched = {
                use crate::schema::agent_tables::deployment as q;
                let params = (server, issuer);
                q::table
                    .select((((((q::version).eq(1)).and((q::server_name).eq(&params.0)))
                        .and((q::issuer).eq(&params.1))),))
                    .filter(q::singleton)
                    .get_result::<Match>(db)
                    .await
            }?;
            if !matched.matched {
                return Err(Error::Conflict("deployment_identity_mismatch"));
            }
            Ok(())
        })
        .await?;
        Ok(Self {
            db: Arc::new(crate::database::Database::business(url)?),
            issuer: issuer.into(),
        })
    }
    pub async fn sign_in(&self, identity: Identity, now: i64) -> Result<SessionGrant> {
        if identity.issuer != self.issuer || identity.valid_until_ms <= now {
            return Err(Error::Unauthorized("authentication_required"));
        }
        let started = std::time::Instant::now();
        tracing::debug!(
            stage = "session_create_lock",
            "Waiting for owner session connection"
        );
        self.db
            .with_connection(async |db| {
                tracing::debug!(
                    stage = "session_create_lock",
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Owner session connection acquired"
                );
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    // All identity/device mutations enter the domain authority lock before rows.
                    // Configuration publication holds owner rows while locking both device sessions.
                    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                        .await?;
                    // Unique identity lock also serializes conflicting subject/MXID registrations.
                    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328903)")
                        .await?;
                    if identity.valid_until_ms <= crate::api::now_ms() {
                        return Err(Error::Unauthorized("authorization_expired"));
                    }
                    let users = {
                        use crate::schema::agent_tables::users as q;
                        let params = (&identity.issuer, &identity.subject, &identity.mxid);
                        q::table
                            .select(User::as_select())
                            .filter(
                                (((q::issuer).eq(&params.0)).and((q::subject).eq(&params.1)))
                                    .or((q::mxid).eq(&params.2)),
                            )
                            .for_update()
                            .load::<User>(db)
                            .await
                    }?;
                    let user_id = match users.as_slice() {
                        [] => {
                            let id = format!("usr_{}", entity_id()?);
                            {
                                use crate::schema::agent_tables::users as q;
                                let params =
                                    (&id, &identity.issuer, &identity.subject, &identity.mxid);
                                diesel::insert_into(q::table)
                                    .values((
                                        q::id.eq(&params.0),
                                        q::issuer.eq(&params.1),
                                        q::subject.eq(&params.2),
                                        q::mxid.eq(&params.3),
                                    ))
                                    .execute(db)
                                    .await
                            }?;
                            id
                        }
                        [user]
                            if user.issuer == identity.issuer
                                && user.subject == identity.subject
                                && user.mxid == identity.mxid =>
                        {
                            if !user.active {
                                return Err(Error::Unauthorized("user_disabled"));
                            }
                            user.id.clone()
                        }
                        _ => return Err(Error::Conflict("identity_mapping_mismatch")),
                    };
                    let session_token = secret_token();
                    {
                        use crate::schema::agent_tables::sessions as q;
                        let params = (
                            format!("ses_{}", entity_id()?),
                            &user_id,
                            hash(&session_token),
                            &identity.client_id,
                            identity.valid_until_ms,
                        );
                        diesel::insert_into(q::table)
                            .values((
                                q::id.eq(&params.0),
                                q::user_id.eq(&params.1),
                                q::token_hash.eq(&params.2),
                                q::client_id.eq(&params.3),
                                q::valid_until_ms.eq(&params.4),
                            ))
                            .execute(db)
                            .await
                    }?;
                    Ok(SessionGrant {
                        token: session_token,
                        user_id,
                        mxid: identity.mxid,
                        valid_until_ms: identity.valid_until_ms,
                    })
                })
                .await
            })
            .await
    }
    async fn principal(
        db: &mut AsyncPgConnection,
        credential: &str,
        now: i64,
        device: bool,
        lock_rows: bool,
    ) -> Result<Principal> {
        if credential.len() != 64 {
            return Err(Error::Unauthorized("authentication_required"));
        }
        let query = if device {
            "SELECT u.id AS user_id,u.mxid,u.subject,s.id AS session_id,s.client_id,s.valid_until_ms,d.id AS device_id,d.generation AS device_generation FROM hagency_agent_v1.devices d JOIN hagency_agent_v1.sessions s ON s.id=d.session_id JOIN hagency_agent_v1.users u ON u.id=d.user_id AND u.id=s.user_id WHERE d.token_hash=$1 AND NOT d.revoked AND NOT s.revoked AND u.active AND s.valid_until_ms>greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint)"
        } else {
            "SELECT u.id AS user_id,u.mxid,u.subject,s.id AS session_id,s.client_id,s.valid_until_ms,NULL::text AS device_id,NULL::bigint AS device_generation FROM hagency_agent_v1.sessions s JOIN hagency_agent_v1.users u ON u.id=s.user_id WHERE s.token_hash=$1 AND NOT s.revoked AND u.active AND s.valid_until_ms>greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint)"
        };
        let query = if lock_rows {
            format!(
                "{query} FOR UPDATE OF {}",
                if device { "s,d,u" } else { "s,u" }
            )
        } else {
            query.to_owned()
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
        self.db
            .with_connection(async |db| {
                // Read-only identity snapshot: every operational Domain/Transport call
                // revalidates it under its own transaction lock. Autocommit row locks here
                // can invert owner/session ordering against configuration publication.
                Self::principal(&mut *db, credential, now, device, false).await
            })
            .await
    }
    pub async fn renew(&self, credential: &str, identity: Identity, now: i64) -> Result<i64> {
        if identity.issuer != self.issuer || identity.valid_until_ms <= now {
            return Err(Error::Unauthorized("authentication_required"));
        }
        let started = std::time::Instant::now();
        tracing::debug!(
            stage = "session_renew_lock",
            "Waiting for owner renewal connection"
        );
        self.db.with_connection(async |db| {

        tracing::debug!(
            stage = "session_renew_lock",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Owner renewal connection acquired"
        );
        db
            .transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
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
                {
                    use crate::schema::agent_tables::sessions as q;
                    let params = (identity.valid_until_ms, &p.session_id);
                    diesel::update(q::table)
                        .filter((q::id).eq(&params.1))
                        .set((q::valid_until_ms.eq(&params.0),))
                        .execute(db)
                        .await
                }?;
                Ok(identity.valid_until_ms)
            })
            .await
    }).await
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
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async move |db: &mut AsyncPgConnection| {
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            let p=Self::principal(db,credential,now,false,true).await?;
            let device_token=secret_token();
            // Same owner/install keeps stable id, rotates credential and fences prior generations.
            let device=sql_query("INSERT INTO hagency_agent_v1.devices(id,user_id,installation_id,name,session_id,token_hash,generation) VALUES($1,$2,$3,$4,$5,$6,1) ON CONFLICT(user_id,installation_id) DO UPDATE SET name=EXCLUDED.name,session_id=EXCLUDED.session_id,token_hash=EXCLUDED.token_hash,generation=hagency_agent_v1.devices.generation+1,revoked=false RETURNING id,generation")
                .bind::<Text,_>(format!("dev_{}",entity_id()?)).bind::<Text,_>(&p.user_id).bind::<Text,_>(&request.installation_id).bind::<Text,_>(request.name.trim()).bind::<Text,_>(&p.session_id).bind::<Text,_>(hash(&device_token)).get_result::<Device>(db).await?;
            Ok(DeviceGrant {device_id:device.id,token:device_token,generation:device.generation,valid_until_ms:p.valid_until_ms})
        }).await
    }).await
    }
    pub async fn revoke_device(&self, credential: &str, id: &str, now: i64) -> Result<()> {
        key(id)?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                        .await?;
                    let p = Self::principal(db, credential, now, false, true).await?;
                    let changed = {
                        use crate::schema::agent_tables::devices as q;
                        let params = (id, &p.user_id);
                        diesel::update(q::table)
                            .filter(((q::id).eq(&params.0)).and((q::user_id).eq(&params.1)))
                            .set((q::revoked.eq(true), q::generation.eq(q::generation + 1)))
                            .execute(db)
                            .await
                    }?;
                    if changed != 1 {
                        return Err(Error::Unauthorized("device_not_owned"));
                    }
                    Ok(())
                })
                .await
            })
            .await
    }
    pub async fn sign_out(&self, credential: &str) -> Result<()> {
        // Idempotent even after expiry; revocation closes every device attached to this session.
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                        .await?;
                    sql_query(
                        "UPDATE hagency_agent_v1.sessions SET revoked=true WHERE token_hash=$1",
                    )
                    .bind::<Text, _>(hash(credential))
                    .execute(db)
                    .await?;
                    Ok(())
                })
                .await
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
    async fn postgres_auth_path_migration_retains_identity_and_revokes_old_devices() {
        let source = std::env::var("HAGENCY_AGENT_TEST_DATABASE_URL").unwrap();
        let name = format!(
            "hagency_auth_migrate_{}",
            &crate::hash(&secret_token())[..16]
        );
        let mut control = AsyncPgConnection::establish(&source).await.unwrap();
        control
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let mut isolated = url::Url::parse(&source).unwrap();
        isolated.set_path(&format!("/{name}"));
        let outcome = tokio::spawn(async move {
            let url = isolated.as_str();
            let old = "https://example.test/_pasion/";
            let new = "https://example.test/auth/";
            let now = crate::api::now_ms();
            let store = Store::open(url, "example.test", old).await.unwrap();
            let proof = Identity {
                issuer: old.into(),
                subject: "migration-owner".into(),
                mxid: "@migration:example.test".into(),
                client_id: "migration-client".into(),
                valid_until_ms: now + 60000,
            };
            let session = store.sign_in(proof.clone(), now).await.unwrap();
            let device = store
                .register_device(
                    &session.token,
                    RegisterDevice {
                        installation_id: "migration-device".into(),
                        name: "Migration".into(),
                    },
                    now,
                )
                .await
                .unwrap();
            assert!(Store::open(url, "example.test", new).await.is_err());
            assert!(
                Store::migrate_auth_path(url, "foreign.test", new)
                    .await
                    .is_err()
            );
            assert!(
                Store::migrate_auth_path(url, "example.test", "https://foreign.test/auth/")
                    .await
                    .is_err()
            );
            assert!(store.authenticate(&device.token, now, true).await.is_ok());
            assert!(
                Store::migrate_auth_path(url, "example.test", new)
                    .await
                    .unwrap()
            );
            assert!(
                !Store::migrate_auth_path(url, "example.test", new)
                    .await
                    .unwrap()
            );
            let upgraded = Store::open(url, "example.test", new).await.unwrap();
            assert!(
                upgraded
                    .authenticate(&device.token, now, true)
                    .await
                    .is_err()
            );
            assert!(
                upgraded
                    .authenticate(&session.token, now, false)
                    .await
                    .is_err()
            );
            assert!(Store::open(url, "example.test", old).await.is_err());
            let fresh = upgraded
                .sign_in(
                    Identity {
                        issuer: new.into(),
                        ..proof
                    },
                    now,
                )
                .await
                .unwrap();
            assert_eq!(fresh.user_id, session.user_id);
            let mut db = AsyncPgConnection::establish(url).await.unwrap();
            assert!(
                sql_query("UPDATE hagency_agent_v1.users SET issuer=$1 WHERE id=$2")
                    .bind::<Text, _>(old)
                    .bind::<Text, _>(&fresh.user_id)
                    .execute(&mut db)
                    .await
                    .is_err()
            );
        })
        .await;
        control
            .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
            .await
            .unwrap();
        outcome.unwrap();
    }

    fn identity(subject: &str, mxid: &str, expires: i64) -> Identity {
        Identity {
            issuer: "https://example.test/auth/".into(),
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
        let store = Store::open(&url, "example.test", "https://example.test/auth/")
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
        let reopened = Store::open(&url, "example.test", "https://example.test/auth/")
            .await
            .unwrap();
        assert!(
            reopened
                .authenticate(&second.token, base + 40_000, true)
                .await
                .is_ok()
        );
        assert!(
            Store::open(&url, "foreign.test", "https://example.test/auth/")
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
        let mut db = store.db.test_connection().await.unwrap();
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
        #[derive(diesel::Queryable, diesel::QueryableByName)]
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
