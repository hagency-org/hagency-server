//! `PATCH /agents/{agentId}`: owner sets an Agent's display name.
use crate::{
    Error, Result, api::now_ms, domain::DomainStore, matrix_client::MatrixClient,
    store::Principal,
};
use salvo::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};

/// Bounds the re-check loop when renames keep landing concurrently.
const PROFILE_SYNC_ATTEMPTS: usize = 3;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rename {
    display_name: String,
}

pub(crate) async fn call(
    domain: &DomainStore,
    matrix: Option<&MatrixClient>,
    p: &Principal,
    id: &str,
    req: &mut Request,
) -> Result<Value> {
    let body: Rename = req
        .parse_json()
        .await
        .map_err(|_| Error::Invalid("invalid_arguments"))?;
    // The process-wide profile lock orders this commit+PUT against other
    // renames and the binding worker. The re-read loop covers replicas.
    let _ordered = crate::domain::PROFILE_WRITES.lock().await;
    let agent = domain.rename_agent(p, id, &body.display_name, now_ms()).await?;
    match matrix {
        Some(m) => sync_profile(domain, m, p, agent).await.map(|agent| json!({"agent":agent})),
        // No client means no appservice, so there is no Matrix profile to set.
        None => Ok(json!({"agent":agent})),
    }
}

/// Renames race each other after commit, so every request re-reads the stored
/// name after its PUT and repeats until Matrix holds the latest one.
async fn sync_profile(
    domain: &DomainStore,
    m: &MatrixClient,
    p: &Principal,
    mut agent: crate::domain::Agent,
) -> Result<crate::domain::Agent> {
    for _ in 0..PROFILE_SYNC_ATTEMPTS {
        m.managed_set_display_name(&agent)
            .await
            .map_err(|_| Error::Unavailable("matrix_display_name_unavailable"))?;
        let stored = domain.agent(p, &agent.id, now_ms()).await?;
        if stored.display_name == agent.display_name {
            return Ok(stored);
        }
        agent = stored;
    }
    Err(Error::Unavailable("matrix_display_name_contended"))
}
