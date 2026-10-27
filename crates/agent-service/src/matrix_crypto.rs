//! Only the domain's freshly locked execution-host scope can reach this broker.
use super::*;
use crate::domain::agent_crypto::{MAX_BYTES, Operation};
impl MatrixClient {
    async fn crypto_json(
        &self,
        request: reqwest::RequestBuilder,
        until: i64,
    ) -> Result<(u16, Value)> {
        if crate::api::now_ms() >= until {
            return Err(Error::Unauthorized("authorization_expired"));
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| Error::Unavailable("agent_crypto_outcome_unknown"))?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Unavailable("agent_crypto_outcome_unknown"))?
        {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(Error::Unavailable("agent_crypto_response_too_large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Unavailable("invalid_agent_crypto_response"))?;
        if !value.is_object()
            || ["access_token", "refresh_token"]
                .iter()
                .any(|k| value.get(k).is_some())
        {
            return Err(Error::Unavailable("invalid_agent_crypto_response"));
        }
        Ok((status, value))
    }
    pub(crate) async fn crypto_identity_request(
        &self,
        user: &str,
        device: &str,
        operation: Operation,
        body: Value,
        until: i64,
    ) -> Result<(u16, Value)> {
        self.puppet(user)?;
        if matches!(operation, Operation::Provision) {
            let mut url = self.origin.clone();
            url.set_query(None);
            url.set_fragment(None);
            url.set_path("/_matrix/client/v3/register");
            let request=self.http.post(url).bearer_auth(&self.as_token).json(&json!({"type":"m.login.application_service","username":user.split_once(':').ok_or(Error::Invalid("invalid_puppet_identity"))?.0.trim_start_matches('@'),"inhibit_login":true}));
            let (status, value) = self.crypto_json(request, until).await?;
            if value.get("device_id").is_some() {
                return Err(Error::Unavailable("invalid_agent_crypto_response"));
            }
            if (200..300).contains(&status) {
                if value["user_id"] != user {
                    return Err(Error::Unavailable("agent_crypto_identity_mismatch"));
                }
            } else if status != 400 || value["errcode"] != "M_USER_IN_USE" {
                return Ok((status, value));
            }
        }
        let (method, segments) = operation.request(device);
        let mut url = self.origin.clone();
        url.set_query(None);
        url.set_fragment(None);
        url.path_segments_mut()
            .map_err(|_| Error::Invalid("invalid_matrix_origin"))?
            .clear()
            .extend(segments);
        if !matches!(operation, Operation::Versions) {
            url.query_pairs_mut().append_pair("user_id", user);
        }
        if !matches!(operation, Operation::Provision | Operation::Versions) {
            url.query_pairs_mut().append_pair("device_id", device);
        }
        let mut request = self.http.request(method, url).bearer_auth(&self.as_token);
        if matches!(operation, Operation::Provision) {
            request = request.json(&json!({"display_name":"Hagency Agent execution device"}));
        } else if !body.is_null() {
            request = request.json(&body);
        }
        let (status, value) = self.crypto_json(request, until).await?;
        if (200..300).contains(&status)
            && matches!(operation, Operation::Whoami)
            && (value["user_id"] != user
                || value["device_id"] != device
                || value["is_guest"] == true)
        {
            return Err(Error::Unavailable("agent_crypto_identity_mismatch"));
        }
        Ok((status, value))
    }
}
