//! Exact bounded management routes, authenticated with the existing device grant.
use crate::{Error, Result, domain::DomainStore, management_types::*, store::Store};
use salvo::{http::Method, prelude::*};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
mod agents;
mod budget;
mod budget_execution;
mod capacity;
mod core;
mod pools;
mod session;
mod settlement;
mod shared;

async fn body<T: DeserializeOwned>(req: &mut Request) -> Result<T> {
    body_with_limit(req, MAX_BYTES).await
}
async fn body_with_limit<T: DeserializeOwned>(req: &mut Request, max_bytes: usize) -> Result<T> {
    let bytes = req
        .payload_with_max_size(max_bytes)
        .await
        .map_err(|_| Error::Invalid("invalid_management_payload"))?;
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid("invalid_management_payload"))
}
pub(crate) async fn call(
    req: &mut Request,
    store: &Store,
    domain: &DomainStore,
    policy: Option<&ManagementPolicy>,
    credential: &str,
    gateway: &crate::gateway::Gateway,
    matrix: Option<&crate::matrix_client::MatrixClient>,
) -> Result<Option<Value>> {
    let path = req.uri().path().to_owned();
    let Some(suffix) = path.strip_prefix("/hagency/api/v1/management/") else {
        return Ok(None);
    };
    let policy = policy.ok_or(Error::Unavailable("management_not_configured"))?;
    if req.uri().query().is_some() {
        return Err(Error::Invalid("invalid_management_payload"));
    }
    let p = store
        .authenticate(credential, crate::api::now_ms(), true)
        .await?;
    let parts = suffix.split('/').collect::<Vec<_>>();
    // Separate protocol groups keep the debug poll stack bounded as routes grow.
    let mut result = if let Some(value) =
        Box::pin(session::call(req, domain, policy, &p, gateway, &parts)).await?
    {
        value
    } else if let Some(value) = Box::pin(pools::call(req, domain, policy, &p, &parts)).await? {
        value
    } else if let Some(value) =
        Box::pin(shared::call(req, domain, policy, &p, gateway, &parts, matrix)).await?
    {
        value
    } else if let Some(value) = Box::pin(budget_execution::call(
        req, domain, policy, &p, gateway, &parts,
    ))
    .await?
    {
        value
    } else if let Some(value) = Box::pin(settlement::call(req, domain, &p, &parts)).await? {
        value
    } else if let Some(value) = Box::pin(budget::call(req, domain, policy, &p, &parts)).await? {
        value
    } else if let Some(value) =
        Box::pin(agents::call(req, domain, policy, &p, gateway, &parts)).await?
    {
        value
    } else if let Some(value) = Box::pin(capacity::call(
        req, domain, policy, &p, gateway, matrix, &parts,
    ))
    .await?
    {
        value
    } else if let Some(value) =
        Box::pin(core::call(req, domain, policy, &p, gateway, &parts)).await?
    {
        value
    } else {
        return Err(Error::NotFound("not_found"));
    };
    result["observedAtMs"] = json!(domain.management_observed_at_ms().await?);
    // Keep reauthentication as the final await, including after the clock read.
    // Do not return private projections after a concurrent session/device revoke.
    store
        .authenticate(credential, crate::api::now_ms(), true)
        .await?;
    Ok(Some(result))
}
