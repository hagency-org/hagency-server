//! Device identity survives token refresh, but a revoked device must never
//! reuse a local Olm account whose published keys were deleted by logout.
use crate::{Error, Result};
use diesel::{
    OptionalExtension, sql_query,
    sql_types::{BigInt, Bool, Nullable, Text},
};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
#[derive(diesel::QueryableByName)]
struct Saved {
    #[diesel(sql_type=Text)]
    device_id: String,
    #[diesel(sql_type=Bool)]
    revoked: bool,
}
pub(crate) async fn select(
    db: &mut AsyncPgConnection,
    scope: &str,
    executor: &str,
    generation: i64,
    initial: &str,
) -> Result<String> {
    let saved=sql_query("SELECT device_id,(revoked OR retiring) AS revoked FROM hagency_agent_v1.executor_matrix_devices WHERE scope_id=$1 AND executor_device_id=$2 AND executor_generation=$3 ORDER BY (revoked OR retiring),device_id LIMIT 1")
        .bind::<Text,_>(scope).bind::<Text,_>(executor).bind::<BigInt,_>(generation).get_result::<Saved>(db).await.optional()?;
    match saved {
        Some(s) if !s.revoked => Ok(s.device_id),
        Some(_) => Ok(format!(
            "HAGE2EE{}",
            crate::hash(&crate::secret_token())[..24].to_ascii_uppercase()
        )),
        None if initial.starts_with("HAGE2EE") => Ok(initial.into()),
        None => Err(Error::Invalid("invalid_agent_device")),
    }
}

#[derive(diesel::QueryableByName)]
struct IssuanceDevice {
    #[diesel(sql_type=Text)]
    puppet_mxid: String,
    #[diesel(sql_type=Text)]
    executor_device_id: String,
    #[diesel(sql_type=BigInt)]
    executor_generation: i64,
    #[diesel(sql_type=Text)]
    kind: String,
    #[diesel(sql_type=Bool)]
    revoked: bool,
    #[diesel(sql_type=Bool)]
    retiring: bool,
    #[diesel(sql_type=Nullable<Text>)]
    issuance_nonce: Option<String>,
    #[diesel(sql_type=BigInt)]
    issuance_until_ms: i64,
}

/// A nonce identifies exactly one login across replicas, including late HTTP outcomes.
pub(crate) struct Issuance<'a> {
    scope: &'a str,
    pub(crate) puppet: String,
    executor: &'a str,
    generation: i64,
    kind: &'static str,
    nonce: String,
}

impl<'a> Issuance<'a> {
    pub(crate) fn new(
        scope: &'a str,
        puppet: &str,
        executor: &'a str,
        generation: i64,
        kind: &'static str,
    ) -> Self {
        Self {
            scope,
            puppet: puppet.to_owned(),
            executor,
            generation,
            kind,
            nonce: crate::secret_token(),
        }
    }

    pub(crate) async fn reserve(
        &self,
        db: &mut AsyncPgConnection,
        initial: &str,
        now: i64,
    ) -> Result<String> {
        // A timed-out login retires its old Olm identity. Select a new ID on the second pass.
        for _ in 0..2 {
            let id = select(db, self.scope, self.executor, self.generation, initial).await?;
            sql_query("INSERT INTO hagency_agent_v1.executor_matrix_devices(scope_id,puppet_mxid,device_id,executor_device_id,executor_generation,kind) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(scope_id,device_id) DO NOTHING")
                .bind::<Text,_>(self.scope).bind::<Text,_>(&self.puppet).bind::<Text,_>(&id)
                .bind::<Text,_>(self.executor).bind::<BigInt,_>(self.generation).bind::<Text,_>(self.kind).execute(db).await?;
            let saved = sql_query("SELECT * FROM hagency_agent_v1.executor_matrix_devices WHERE scope_id=$1 AND device_id=$2 FOR UPDATE")
                .bind::<Text,_>(self.scope).bind::<Text,_>(&id).get_result::<IssuanceDevice>(db).await?;
            if saved.puppet_mxid != self.puppet
                || saved.executor_device_id != self.executor
                || saved.executor_generation != self.generation
                || saved.kind != self.kind
                || saved.retiring
                || saved.revoked
            {
                return Err(Error::Conflict("matrix_device_identity_changed"));
            }
            if saved.issuance_nonce.is_some() {
                if saved.issuance_until_ms > now {
                    return Err(Error::Conflict("matrix_device_issuance_pending"));
                }
                sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true,revoked=false WHERE scope_id=$1 AND device_id=$2")
                    .bind::<Text,_>(self.scope).bind::<Text,_>(&id).execute(db).await?;
                continue;
            }
            sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET issuance_nonce=$3,issuance_until_ms=$4 WHERE scope_id=$1 AND device_id=$2")
                .bind::<Text,_>(self.scope).bind::<Text,_>(&id).bind::<Text,_>(&self.nonce).bind::<BigInt,_>(now + 30_000).execute(db).await?;
            return Ok(id);
        }
        Err(Error::Conflict("matrix_device_issuance_pending"))
    }

    pub(crate) async fn confirm(
        &self,
        db: &mut AsyncPgConnection,
        id: &str,
        now: i64,
    ) -> Result<()> {
        let updated = sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET issuance_nonce=NULL,issuance_until_ms=0
            WHERE scope_id=$1 AND device_id=$2 AND puppet_mxid=$3 AND executor_device_id=$4 AND executor_generation=$5 AND kind=$6
              AND NOT revoked AND NOT retiring AND issuance_nonce=$7 AND issuance_until_ms>greatest($8,(extract(epoch from clock_timestamp())*1000)::bigint)")
            .bind::<Text,_>(self.scope).bind::<Text,_>(id).bind::<Text,_>(&self.puppet).bind::<Text,_>(self.executor)
            .bind::<BigInt,_>(self.generation).bind::<Text,_>(self.kind).bind::<Text,_>(&self.nonce).bind::<BigInt,_>(now).execute(db).await?;
        if updated == 1 {
            Ok(())
        } else {
            Err(Error::Unauthorized("transport_scope_not_authorized"))
        }
    }

    pub(crate) async fn retire(
        &self,
        database: &crate::database::Database,
        matrix: &crate::matrix_client::MatrixClient,
        id: &str,
        login_completed: bool,
    ) {
        let retired = database.transaction(async |db| {
            Ok(sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET retiring=true,revoked=false,issuance_nonce=CASE WHEN $4 THEN NULL ELSE issuance_nonce END,issuance_until_ms=CASE WHEN $4 THEN 0 ELSE issuance_until_ms END,last_checked_ms=0 WHERE scope_id=$1 AND device_id=$2 AND issuance_nonce=$3")
                .bind::<Text,_>(self.scope).bind::<Text,_>(id).bind::<Text,_>(&self.nonce).bind::<Bool,_>(login_completed).execute(db).await?)
        }).await;
        match retired {
            Ok(0) => {}
            Ok(_) => {
                if matrix.logout_device(&self.puppet, id).await.is_ok() {
                    let _ = database.transaction(async |db| {
                        Ok(sql_query("UPDATE hagency_agent_v1.executor_matrix_devices SET revoked=true WHERE scope_id=$1 AND device_id=$2 AND retiring AND issuance_nonce IS NULL")
                            .bind::<Text,_>(self.scope).bind::<Text,_>(id).execute(db).await?)
                    }).await;
                }
            }
            Err(_) => tracing::warn!(
                "Failed to mark Matrix device retiring; durable issuance claim awaits cleanup"
            ),
        }
    }
}
