//! Device identity survives token refresh, but a revoked device must never
//! reuse a local Olm account whose published keys were deleted by logout.
use crate::{Error, Result};
use diesel::{
    OptionalExtension, sql_query,
    sql_types::{BigInt, Bool, Text},
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
