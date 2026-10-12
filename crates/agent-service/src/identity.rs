use crate::{Error, Result};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::time::Duration;

/// Constructed only after trusted Pasion introspection AND Matrix whoami.
/// Not deserializable: HTTP request bodies cannot assert their own authority.
#[derive(Clone, Debug)]
pub struct Identity {
    pub(crate) issuer: String,
    pub(crate) subject: String,
    pub(crate) mxid: String,
    pub(crate) client_id: String,
    pub(crate) valid_until_ms: i64,
}
impl Identity {
    pub fn mxid(&self) -> &str {
        &self.mxid
    }
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
}
#[derive(Clone)]
pub struct Verifier {
    http: Client,
    issuer: Url,
    introspection: Url,
    matrix: Url,
    secret: String,
    server: String,
}
#[derive(Deserialize)]
struct Introspection {
    active: bool,
    sub: Option<String>,
    username: Option<String>,
    scope: Option<String>,
    client_id: Option<String>,
    exp: Option<i64>,
}
#[derive(Deserialize)]
struct Whoami {
    user_id: String,
    #[serde(default)]
    is_guest: bool,
}
impl Verifier {
    /// Origins are pinned by host configuration, never by an OAuth response.
    pub fn new(
        issuer: Url,
        introspection: Url,
        matrix: Url,
        secret: String,
        server: String,
    ) -> Result<Self> {
        if !matches!(issuer.scheme(), "http" | "https")
            || !matches!(introspection.scheme(), "http" | "https")
            || !matches!(matrix.scheme(), "http" | "https")
            || secret.is_empty()
            || server.is_empty()
            || [&issuer, &introspection, &matrix].iter().any(|u| {
                !u.username().is_empty()
                    || u.password().is_some()
                    || u.query().is_some()
                    || u.fragment().is_some()
            })
        {
            return Err(Error::Invalid("invalid_identity_configuration"));
        }
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| Error::Unavailable("identity_unavailable"))?;
        Ok(Self {
            http,
            issuer,
            introspection,
            matrix,
            secret,
            server,
        })
    }
    pub async fn verify(&self, token: &str, now_ms: i64) -> Result<Identity> {
        if !(16..=4096).contains(&token.len()) {
            return Err(Error::Unauthorized("authentication_required"));
        }
        let started = std::time::Instant::now();
        tracing::debug!(stage="identity_introspection", "Verifying owner OAuth proof");
        let response = self
            .http
            .post(self.introspection.clone())
            .bearer_auth(&self.secret)
            .form(&[("token", token)])
            .send()
            .await
            .map_err(|_| Error::Unavailable("pasion_unavailable"))?;
        tracing::debug!(stage="identity_introspection",elapsed_ms=started.elapsed().as_millis() as u64,
            status=response.status().as_u16(),"OAuth introspection responded");
        let raw = bounded(response, "pasion_unavailable").await?;
        let introspection: Introspection = serde_json::from_slice(&raw)
            .map_err(|_| Error::Unavailable("invalid_identity_response"))?;
        let endpoint = self
            .matrix
            .join("/_matrix/client/v3/account/whoami")
            .map_err(|_| Error::Invalid("invalid_identity_configuration"))?;
        // Validate inactive/expired tokens BEFORE forwarding them to Matrix.
        fields(&introspection, now_ms)?;
        tracing::debug!(stage="identity_matrix",elapsed_ms=started.elapsed().as_millis() as u64,"Checking Matrix identity");
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| Error::Unavailable("matrix_unavailable"))?;
        tracing::debug!(stage="identity_matrix",elapsed_ms=started.elapsed().as_millis() as u64,
            status=response.status().as_u16(),"Matrix identity responded");
        let raw = bounded(response, "matrix_unavailable").await?;
        let who: Whoami = serde_json::from_slice(&raw)
            .map_err(|_| Error::Unavailable("invalid_identity_response"))?;
        checked(
            self.issuer.as_str(),
            &self.server,
            introspection,
            who,
            now_ms.max(crate::api::now_ms()),
        )
    }
}
async fn bounded(mut response: reqwest::Response, unavailable: &'static str) -> Result<Vec<u8>> {
    if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
        return Err(Error::Unauthorized("authentication_required"));
    }
    if !response.status().is_success() {
        return Err(Error::Unavailable(unavailable));
    }
    let mut raw = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::Unavailable(unavailable))?
    {
        if raw.len() + chunk.len() > 65536 {
            return Err(Error::Unavailable("invalid_identity_response"));
        }
        raw.extend_from_slice(&chunk);
    }
    Ok(raw)
}
fn fields(i: &Introspection, now: i64) -> Result<()> {
    if !i.active
        || i.exp.is_none_or(|exp| exp <= now / 1000)
        || [
            i.sub.as_deref(),
            i.username.as_deref(),
            i.client_id.as_deref(),
        ]
        .iter()
        .any(|v| v.is_none_or(|s| s.is_empty() || s.len() > 255 || s.chars().any(char::is_control)))
        || !i.scope.as_deref().is_some_and(|scope| {
            scope.split_whitespace().any(|s| {
                matches!(
                    s,
                    "urn:matrix:client:api:*" | "urn:matrix:org.matrix.msc2967.client:api:*"
                )
            })
        })
    {
        return Err(Error::Unauthorized("authentication_required"));
    }
    Ok(())
}
fn checked(
    issuer: &str,
    server: &str,
    i: Introspection,
    who: Whoami,
    now: i64,
) -> Result<Identity> {
    fields(&i, now)?;
    let username = i.username.unwrap();
    if who.is_guest
        || username.contains(':')
        || username.chars().any(char::is_whitespace)
        || username.starts_with("_hagency_")
        || who.user_id != format!("@{username}:{server}")
        || who.user_id.len() > 255
    {
        return Err(Error::Unauthorized("human_identity_required"));
    }
    let expiry = i
        .exp
        .unwrap()
        .checked_mul(1000)
        .ok_or(Error::Unauthorized("authentication_required"))?;
    Ok(Identity {
        issuer: issuer.into(),
        subject: i.sub.unwrap(),
        mxid: who.user_id,
        client_id: i.client_id.unwrap(),
        valid_until_ms: expiry.min(now.saturating_add(30_000)),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Introspection {
        Introspection {
            active: true,
            sub: Some("stable-user".into()),
            username: Some("alice".into()),
            scope: Some("urn:matrix:client:api:*".into()),
            client_id: Some("native-client".into()),
            exp: Some(100),
        }
    }
    fn who() -> Whoami {
        Whoami {
            user_id: "@alice:example.test".into(),
            is_guest: false,
        }
    }
    #[test]
    fn requires_live_scope_and_joint_identity() {
        let good = checked(
            "https://example.test/auth/",
            "example.test",
            input(),
            who(),
            10_000,
        )
        .unwrap();
        assert_eq!(good.valid_until_ms, 40_000);
        for change in 0..5 {
            let mut i = input();
            let mut w = who();
            match change {
                0 => i.active = false,
                1 => i.exp = Some(10),
                2 => i.scope = Some("urn:palpo:admin:*".into()),
                3 => w.is_guest = true,
                _ => w.user_id = "@bob:example.test".into(),
            }
            assert!(checked("issuer", "example.test", i, w, 10_000).is_err());
        }
    }
}
