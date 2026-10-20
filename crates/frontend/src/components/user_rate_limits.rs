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

    let rate_limits_data = use_resource(move || {
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

    // Initialize from data
    if let Some(Some(data)) = &*rate_limits_data.read() {
        if mps.read().is_empty()
            && let Some(v) = data.messages_per_second
        {
            mps.set(v.to_string());
        }
        if burst.read().is_empty()
            && let Some(v) = data.burst_count
        {
            burst.set(v.to_string());
        }
    }

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
                let mut body = serde_json::Map::new();
                if let Ok(v) = mps_val.parse::<u64>() {
                    body.insert("messages_per_second".into(), serde_json::json!(v));
                }
                if let Ok(v) = burst_val.parse::<u64>() {
                    body.insert("burst_count".into(), serde_json::json!(v));
                }

                let (method, req_body) = if body.is_empty() {
                    ("DELETE", None)
                } else {
                    (
                        "POST",
                        Some(serde_json::to_string(&body).unwrap_or_default()),
                    )
                };

                let result: Result<serde_json::Value, _> = api_client(&url, method, req_body).await;
                match result {
                    Ok(_) => show_toast("Rate limits updated", ToastVariant::Success),
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
                        div { class: "space-y-2",
                            Label { "Messages per Second" }
                            Input {
                                r#type: "number".to_string(),
                                placeholder: "Default".to_string(),
                                value: mps.read().clone(),
                                oninput: move |evt: FormEvent| mps.set(evt.value()),
                            }
                        }
                        div { class: "space-y-2",
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
