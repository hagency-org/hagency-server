//! Native, device-authorized HTTP configuration endpoints.
use crate::{Error, Result, configuration_types::*, domain::DomainStore, store::Store};
use salvo::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

async fn body<T: DeserializeOwned>(req: &mut Request) -> Result<T> {
    let payload = req
        .payload()
        .await
        .map_err(|_| Error::Invalid("invalid_configuration_payload"))?;
    if payload.len() > MAX_BYTES {
        return Err(Error::Invalid("invalid_configuration_payload"));
    }
    serde_json::from_slice(payload).map_err(|_| Error::Invalid("invalid_configuration_payload"))
}
pub(crate) async fn call(
    req: &mut Request,
    store: &Store,
    domain: &DomainStore,
    credential: &str,
) -> Result<Option<Value>> {
    let path = req.uri().path().to_owned();
    let parts = path.split('/').collect::<Vec<_>>();
    let method = req.method().clone();
    let agent = parts.len() >= 7 && parts[4] == "agents" && parts[6] == "configuration";
    let worker = parts.len() >= 5 && parts[4] == "device-configuration";
    if !agent && !worker {
        return Ok(None);
    }
    if req.uri().query().is_some() {
        return Err(Error::Invalid("invalid_configuration_payload"));
    }
    let principal = store
        .authenticate(credential, crate::api::now_ms(), true)
        .await?;
    let result = if agent && parts.len() == 7 && method == salvo::http::Method::GET {
        json!({"configuration":domain.configuration_status(&principal,parts[5],crate::api::now_ms()).await?})
    } else if agent
        && parts.len() == 8
        && parts[7] == "requests"
        && method == salvo::http::Method::POST
    {
        let input: ConfigurationInput = body(req).await?;
        json!({"request":domain.create_configuration_request(&principal,parts[5],input,crate::api::now_ms()).await?})
    } else if agent
        && parts.len() == 9
        && parts[7] == "requests"
        && method == salvo::http::Method::GET
    {
        json!({"request":domain.configuration_request(&principal,parts[5],parts[8],crate::api::now_ms()).await?})
    } else if path == "/hagency/api/v1/device-configuration/poll"
        && method == salvo::http::Method::POST
    {
        let input: ConfigurationPoll = body(req).await?;
        json!({"request":domain.poll_configuration(&principal,input,crate::api::now_ms()).await?})
    } else if worker
        && parts.len() == 8
        && parts[5] == "requests"
        && parts[7] == "complete"
        && method == salvo::http::Method::POST
    {
        let value: Value = body(req).await?;
        if value.as_object().is_none_or(|v| {
            v.len() != 3
                || !v.contains_key("state")
                || !v.contains_key("result")
                || !v.contains_key("errorCode")
        }) {
            return Err(Error::Invalid("invalid_configuration_payload"));
        }
        let input: ConfigurationComplete = serde_json::from_value(value)
            .map_err(|_| Error::Invalid("invalid_configuration_payload"))?;
        json!({"request":domain.complete_configuration(&principal,parts[6],input,crate::api::now_ms()).await?})
    } else {
        return Err(Error::NotFound("not_found"));
    };
    Ok(Some(result))
}
