use dioxus::prelude::*;

use crate::api::client::{api_client, build_url};
use crate::components::ui::button::Button;
use crate::components::ui::card::*;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::LoadingSkeleton;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::types::RateLimitsModel;
use crate::utils::mxid::return_mxid;

#[component]
pub fn UserRateLimits(user_id: String) -> Element {
    let uid = user_id.clone();
    let uid_for_resource = uid.clone();

    let mut rate_limits_data = use_resource(move || {
        let user = uid_for_resource.clone();
        async move {
            let mxid = return_mxid(&user);
            let encoded = urlencoding::encode(&mxid);
            let url = build_url(
                &format!("/_palpo/admin/v1/users/{encoded}/override_ratelimit"),
                &[],
            )
            .ok()?;
            let result: Result<RateLimitsModel, _> = api_client(&url, "GET", None).await;
            result.ok()
        }
    });

    let mut mps = use_signal(String::new);
    let mut burst = use_signal(String::new);

    // Only a new response initializes the draft; clearing an input is an edit.
    use_effect(move || {
        if let Some(Some(data)) = &*rate_limits_data.read() {
            mps.set(
                data.messages_per_second
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            );
            burst.set(data.burst_count.map(|v| v.to_string()).unwrap_or_default());
        }
    });

    let uid_for_save = uid.clone();
    let handle_save = move |_: MouseEvent| {
        let user = uid_for_save.clone();
        let mps_val = mps.read().clone();
        let burst_val = burst.read().clone();

        spawn(async move {
            let mxid = return_mxid(&user);
            let encoded = urlencoding::encode(&mxid);
            if let Ok(url) = build_url(
                &format!("/_palpo/admin/v1/users/{encoded}/override_ratelimit"),
                &[],
            ) {
                let body = match rate_limit_body(&mps_val, &burst_val) {
                    Ok(body) => body,
                    Err(error) => {
                        show_toast(&error, ToastVariant::Error);
                        return;
                    }
                };
                let (method, req_body) = match body {
                    Some(body) => ("POST", Some(body.to_string())),
                    None => ("DELETE", None),
                };

                let result: Result<serde_json::Value, _> = api_client(&url, method, req_body).await;
                match result {
                    Ok(_) => {
                        show_toast("Rate limits updated", ToastVariant::Success);
                        rate_limits_data.restart();
                    }
                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                }
            }
        });
    };

    rsx! {
        Card {
            CardHeader {
                CardTitle { "Rate Limits" }
                CardDescription { "Override rate limiting for this user" }
            }
            CardContent { class: "space-y-4".to_string(),
                match &*rate_limits_data.read() {
                    Some(Some(_)) => rsx! {
                        crate::components::ui::input::Field { class: "space-y-2",
                            Label { "Messages per Second" }
                            Input {
                                r#type: "number".to_string(),
                                placeholder: "Default".to_string(),
                                value: mps.read().clone(),
                                oninput: move |evt: FormEvent| mps.set(evt.value()),
                            }
                        }
                        crate::components::ui::input::Field { class: "space-y-2",
                            Label { "Burst Count" }
                            Input {
                                r#type: "number".to_string(),
                                placeholder: "Default".to_string(),
                                value: burst.read().clone(),
                                oninput: move |evt: FormEvent| burst.set(evt.value()),
                            }
                        }
                        Button {
                            onclick: handle_save,
                            "Save Rate Limits"
                        }
                    },
                    Some(None) => rsx! {
                        p { class: "text-sm text-muted-foreground", "Unable to load rate limits." }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

fn rate_limit_body(mps: &str, burst: &str) -> Result<Option<serde_json::Value>, String> {
    let mut body = serde_json::Map::new();
    for (key, input) in [("messages_per_second", mps), ("burst_count", burst)] {
        if input.trim().is_empty() {
            continue;
        }
        let value = input
            .trim()
            .parse::<i32>()
            .ok()
            .filter(|v| *v >= 0)
            .ok_or_else(|| {
                format!(
                    "{key} must be a non-negative integer no greater than {}",
                    i32::MAX
                )
            })?;
        body.insert(key.into(), serde_json::json!(value));
    }
    Ok((!body.is_empty()).then_some(serde_json::Value::Object(body)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_blank_inputs_restore_defaults() {
        assert_eq!(rate_limit_body("", " ").unwrap(), None);
        for bad in ["-1", "1.5", "2147483648", "bad"] {
            assert!(rate_limit_body(bad, "").is_err());
        }
        assert_eq!(
            rate_limit_body("0", "10").unwrap(),
            Some(serde_json::json!({"messages_per_second":0,"burst_count":10}))
        );
        assert_eq!(
            rate_limit_body("", "2").unwrap(),
            Some(serde_json::json!({"burst_count":2}))
        );
    }
}
