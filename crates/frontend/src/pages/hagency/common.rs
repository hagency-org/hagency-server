pub use crate::utils::i18n::{t, t_with};
pub use crate::api::browser_auth;
use dioxus::prelude::*;
use serde_json::Value;
pub fn rows(value: &Value, key: &str) -> Vec<Value> {
    value[key].as_array().cloned().unwrap_or_default()
}
pub fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").to_owned()
}
pub fn name(value: &Value, fallback: &str) -> String {
    let label = text(value, "name");
    if label.trim().is_empty() {
        text(value, fallback)
    } else {
        label
    }
}
pub fn policy(value: &Value) -> Result<Value, String> {
    let raw = &value["creationPolicy"];
    let policy = if let Some(raw) = raw.as_str() {
        serde_json::from_str(raw)
            .map_err(|_| {t("hagency.access.unreadable")}.to_string())?
    } else {
        raw.clone()
    };
    if !policy.is_object() || value["revision"].as_i64().is_none() {
        return Err({t("hagency.access.unreadable_refresh")}.into());
    }
    let recognized = policy["defaultAllow"].is_boolean()
        || matches!(
            policy["mode"].as_str(),
            Some("inherit_project" | "allow_list" | "disabled")
        );
    if !recognized {
        return Err({t("hagency.access.unknown")}.into());
    }
    Ok(policy)
}
/// Server eligibility and binding states are not device execution health.
pub fn state_label(state: &str) -> String {
    match state {
        "active" => t("hagency.state.active"), "paused" => t("hagency.state.paused"),
        "suspended" => t("hagency.state.suspended"), "removed" => t("hagency.state.removed"),
        "accepted" => t("hagency.state.accepted"), "applied" => t("hagency.state.applied"),
        "pending" => t("hagency.state.pending"), "unknown" => t("hagency.state.unknown"),
        "rejected" => t("hagency.state.rejected"), "failed" => t("hagency.state.failed"),
        _ => state.to_owned(),
    }
}
pub fn member_lines(value: &Value, key: &str) -> String {
    rows(value, key)
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn members(value: &str) -> Vec<String> {
    value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
pub async fn mutate(path: &str, method: &str, body: Value) -> Result<String, String> {
    browser_auth::call(path, method, Some(body))
        .await
        .map(|value| {
            let state = value["commandState"].as_str().unwrap_or("accepted");
            let reason = value["pendingReason"].as_str().unwrap_or("");
            if reason.is_empty() {
                t_with("hagency.request.state", &[("state", &state_label(state))])
            } else {
                t_with("hagency.request.reason", &[("state", &state_label(state)), ("reason", reason)])
            }
        })
        .map_err(|e| e.message)
}
#[component]
pub fn Notice(message: Signal<String>) -> Element {
    rsx! {if !message().is_empty(){p{role:"status",class:"hg-note whitespace-pre-wrap", "{message}"}}}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn access_editing_requires_readable_current_policy_and_revision() {
        assert!(policy(&json!({"revision":4,"creationPolicy":"{\"defaultAllow\":false,\"deny\":[\"@bob:s\"]}"})).is_ok());
        assert!(
            policy(
                &json!({"revision":3,"creationPolicy":{"mode":"allow_list","allow":["@alice:s"]}})
            )
            .is_ok()
        );
        for value in [
            json!({"revision":1}),
            json!({"creationPolicy":{"defaultAllow":true}}),
            json!({"revision":2,"creationPolicy":"broken"}),
            json!({"revision":2,"creationPolicy":{"mode":"future_policy"}}),
        ] {
            assert!(policy(&value).is_err());
        }
    }
}
