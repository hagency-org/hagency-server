//! `PATCH /agents/{agentId}`: owner sets an Agent's display name.
use crate::{
    Error, Result, api::now_ms, domain::DomainStore, matrix_client::MatrixClient,
    store::Principal,
};
use salvo::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};

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
    let agent = domain.rename_agent(p, id, &body.display_name, now_ms()).await?;
    // A `creating` puppet is not registered yet; provision_identity sets the
    // stored name. No client means no appservice, so there is no profile to set.
    if let Some(m) = matrix.filter(|_| agent.state != "creating") {
        m.managed_set_display_name(&agent)
            .await
            .map_err(|_| Error::Unavailable("matrix_display_name_unavailable"))?;
    }
    Ok(json!({"agent":agent}))
}
