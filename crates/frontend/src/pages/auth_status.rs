use dioxus::prelude::*;

use gloo_net::http::Request;

use crate::api::auth;
use crate::api::server_info;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::icons::Icon;
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::PageHeader;
use crate::utils::i18n::t;

#[derive(Debug, Clone)]
struct DiagnosticCheck {
    label: String,
    status: DiagnosticStatus,
    detail: String,
}

#[derive(Debug, Clone, PartialEq)]
enum DiagnosticStatus {
    Pass,
    Warn,
    Fail,
    Skipped,
}

#[component]
pub fn AuthStatusPage() -> Element {
    // All API calls go through the same-origin Nginx reverse proxy, so probe
    // relative paths (empty base) like the rest of the app does. The displayed
    // value reflects the current window origin for clarity.
    let base_url = String::new();
    let display_origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "same origin".to_string());
    let base_url_for_flows = base_url.clone();
    let base_url_for_issuer = base_url.clone();

    let login_flows = use_resource(move || {
        let url = base_url_for_flows.clone();
        async move { auth::get_login_flows(&url).await.map_err(|e| e.message) }
    });

    let server_version = use_resource(|| async { server_info::get_server_version().await.ok() });

    // Delegated auth issuer + OIDC discovery
    let issuer_data = use_resource(move || {
        let url = base_url_for_issuer.clone();
        async move {
            // Step 1: Try to get auth issuer from homeserver
            let issuer_result = auth::get_auth_issuer(&url).await;
            let issuer_url = match issuer_result {
                Ok(val) => val
                    .get("issuer")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                Err(_) => None,
            };

            let mut checks: Vec<DiagnosticCheck> = Vec::new();

            // Check 1: Auth issuer endpoint
            match &issuer_url {
                Some(url) => {
                    checks.push(DiagnosticCheck {
                        label: "Auth Issuer Endpoint".to_string(),
                        status: DiagnosticStatus::Pass,
                        detail: format!("Issuer: {url}"),
                    });

                    // Step 2: Fetch OIDC discovery from issuer
                    match auth::get_oidc_discovery(url).await {
                        Ok(doc) => {
                            checks.push(DiagnosticCheck {
                                label: "OIDC Discovery".to_string(),
                                status: DiagnosticStatus::Pass,
                                detail: "openid-configuration fetched successfully".to_string(),
                            });

                            // Check required endpoints
                            let authorization_endpoint =
                                doc.get("authorization_endpoint").and_then(|v| v.as_str());
                            let token_endpoint = doc.get("token_endpoint").and_then(|v| v.as_str());
                            let registration_endpoint =
                                doc.get("registration_endpoint").and_then(|v| v.as_str());
                            let device_authorization_endpoint = doc
                                .get("device_authorization_endpoint")
                                .and_then(|v| v.as_str());

                            // Authorization endpoint
                            checks.push(DiagnosticCheck {
                                label: "Authorization Endpoint".to_string(),
                                status: if authorization_endpoint.is_some() {
                                    DiagnosticStatus::Pass
                                } else {
                                    DiagnosticStatus::Fail
                                },
                                detail: authorization_endpoint.unwrap_or("Missing").to_string(),
                            });

                            // Token endpoint
                            checks.push(DiagnosticCheck {
                                label: "Token Endpoint".to_string(),
                                status: if token_endpoint.is_some() {
                                    DiagnosticStatus::Pass
                                } else {
                                    DiagnosticStatus::Fail
                                },
                                detail: token_endpoint.unwrap_or("Missing").to_string(),
                            });

                            // DCR (Dynamic Client Registration)
                            checks.push(DiagnosticCheck {
                                label: "Dynamic Client Registration".to_string(),
                                status: if registration_endpoint.is_some() {
                                    DiagnosticStatus::Pass
                                } else {
                                    DiagnosticStatus::Warn
                                },
                                detail: registration_endpoint
                                    .unwrap_or("Not available — clients must be pre-registered")
                                    .to_string(),
                            });

                            // Device authorization (device-code flow)
                            checks.push(DiagnosticCheck {
                                label: "Device Authorization (Device Code Flow)".to_string(),
                                status: if device_authorization_endpoint.is_some() {
                                    DiagnosticStatus::Pass
                                } else {
                                    DiagnosticStatus::Warn
                                },
                                detail: device_authorization_endpoint
                                    .unwrap_or("Not available")
                                    .to_string(),
                            });

                            // Scopes
                            if let Some(scopes) =
                                doc.get("scopes_supported").and_then(|v| v.as_array())
                            {
                                let scope_list: Vec<String> = scopes
                                    .iter()
                                    .filter_map(|s| s.as_str().map(String::from))
                                    .collect();
                                let has_openid = scope_list.contains(&"openid".to_string());
                                let has_matrix_api =
                                    scope_list.iter().any(|s| s.contains("matrix"));
                                checks.push(DiagnosticCheck {
                                    label: "Supported Scopes".to_string(),
                                    status: if has_openid {
                                        DiagnosticStatus::Pass
                                    } else {
                                        DiagnosticStatus::Fail
                                    },
                                    detail: if scope_list.len() > 8 {
                                        format!(
                                            "{} scopes (openid: {}, matrix: {})",
                                            scope_list.len(),
                                            has_openid,
                                            has_matrix_api
                                        )
                                    } else {
                                        scope_list.join(", ")
                                    },
                                });
                            }
                        }
                        Err(e) => {
                            checks.push(DiagnosticCheck {
                                label: "OIDC Discovery".to_string(),
                                status: DiagnosticStatus::Fail,
                                detail: format!("Failed: {}", e.message),
                            });
                        }
                    }
                }
                None => {
                    checks.push(DiagnosticCheck {
                        label: "Auth Issuer Endpoint".to_string(),
                        status: DiagnosticStatus::Warn,
                        detail: "Not available — server may not support delegated auth (MSC2965)"
                            .to_string(),
                    });
                    checks.push(DiagnosticCheck {
                        label: "OIDC Discovery".to_string(),
                        status: DiagnosticStatus::Skipped,
                        detail: "Skipped — no issuer URL available".to_string(),
                    });
                }
            }

            Some(checks)
        }
    });

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("auth_status.title"),
                description: t("auth_status.subtitle"),
            }

            // Server connection info
            Card {
                CardHeader {
                    CardTitle { {t("auth_status.server_info")} }
                }
                CardContent {
                    div { class: "space-y-4",
                        div { class: "flex items-center justify-between py-2",
                            span { class: "text-sm font-medium text-muted-foreground", {t("auth_status.base_url")} }
                            span { class: "text-sm font-mono", "{display_origin}" }
                        }
                        div { class: "flex items-center justify-between py-2",
                            span { class: "text-sm font-medium text-muted-foreground", {t("auth_status.server_version")} }
                            match &*server_version.read() {
                                Some(Some(version)) => rsx! {
                                    span { class: "text-sm", "{version}" }
                                },
                                Some(None) => rsx! {
                                    span { class: "text-sm text-muted-foreground", "-" }
                                },
                                None => rsx! {
                                    span { class: "text-sm text-muted-foreground", "Loading..." }
                                },
                            }
                        }
                    }
                }
            }

            // Login flows / auth capabilities
            Card {
                CardHeader {
                    CardTitle {
                        div { class: "flex items-center gap-2",
                            Icon { name: "shield".to_string(), class: "h-5 w-5".to_string() }
                            {t("auth_status.auth_capabilities")}
                        }
                    }
                    CardDescription { "Pasion discovery and the homeserver’s advertised Matrix login flows." }
                }
                CardContent {
                    match &*login_flows.read() {
                        Some(Ok(flows)) => {
                            let delegated = crate::utils::storage::get_item("oauth_enabled").as_deref() == Some("true");
                            let has_password = !delegated && flows.iter().any(|f| f.flow_type == "m.login.password");
                            let has_sso = flows.iter().any(|f| f.flow_type.contains("sso"));
                            let has_token = flows.iter().any(|f| f.flow_type == "m.login.token");
                            let discovery_checks = issuer_data().flatten();
                            let has_oidc = discovery_checks.as_ref().is_some_and(|checks| checks.iter().any(|c| c.label == "OIDC Discovery" && c.status == DiagnosticStatus::Pass));

                            rsx! {
                                div { class: "space-y-6",
                                    // Capability summary
                                    div { class: "grid gap-4 md:grid-cols-2 lg:grid-cols-4",
                                        CapabilityCard {
                                            label: "Personal Matrix password login".to_string(),
                                            enabled: has_password,
                                            icon: "key",
                                        }
                                        CapabilityCard {
                                            label: "SSO / SAML".to_string(),
                                            enabled: has_sso,
                                            icon: "log-in",
                                        }
                                        CapabilityCard {
                                            label: "Token Login".to_string(),
                                            enabled: has_token,
                                            icon: "hash",
                                        }
                                        CapabilityCard {
                                            label: "OIDC / Delegated Auth".to_string(),
                                            enabled: has_oidc,
                                            icon: "shield",
                                            loading: issuer_data().is_none(),
                                        }
                                    }

                                    // Detailed flow list
                                    div {
                                        h4 { class: "text-sm font-medium mb-2", {t("auth_status.login_flows")} }
                                        div { class: "flex flex-wrap gap-2",
                                            for flow in flows.iter() {
                                                {
                                                    let flow_type = flow.flow_type.clone();
                                                    let variant = if flow_type == "m.login.password" {
                                                        BadgeVariant::Default
                                                    } else if flow_type.contains("sso") {
                                                        BadgeVariant::Secondary
                                                    } else {
                                                        BadgeVariant::Outline
                                                    };
                                                    rsx! {
                                                        Badge {
                                                            key: "{flow_type}",
                                                            variant,
                                                            "{flow_type}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Warnings / hints
                                    if delegated {
                                        p {class:"text-sm text-muted-foreground", "Personal accounts sign in through Pasion. Matrix login flows below describe advertised protocol endpoints, including Appservice access; they do not enable native password login for people."}
                                    }
                                    if !has_password && !delegated {
                                        div { class: "rounded-md bg-yellow-500/10 border border-yellow-500/20 p-3",
                                            p { class: "text-sm text-yellow-600 dark:text-yellow-400",
                                                Icon { name: "alert-triangle".to_string(), class: "h-4 w-4 inline mr-1".to_string() }
                                                {t("auth_status.no_password_warning")}
                                            }
                                        }
                                    }
                                    if has_sso && !has_password {
                                        div { class: "rounded-md bg-blue-500/10 border border-blue-500/20 p-3",
                                            p { class: "text-sm text-blue-600 dark:text-blue-400",
                                                Icon { name: "info".to_string(), class: "h-4 w-4 inline mr-1".to_string() }
                                                {t("auth_status.sso_only_hint")}
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        Some(Err(err)) => rsx! {
                            div { class: "rounded-md bg-destructive/10 p-3",
                                p { class: "text-sm text-destructive", "Failed to fetch login flows: {err}" }
                            }
                        },
                        None => rsx! { PageSkeleton {} },
                    }
                }
            }
            // Delegated Auth Diagnostics
            Card {
                CardHeader {
                    CardTitle {
                        div { class: "flex items-center gap-2",
                            Icon { name: "activity".to_string(), class: "h-5 w-5".to_string() }
                            {t("auth_status.diagnostics_title")}
                        }
                    }
                    CardDescription { {t("auth_status.diagnostics_desc")} }
                }
                CardContent {
                    match &*issuer_data.read() {
                        Some(Some(checks)) => rsx! {
                            div { class: "space-y-2",
                                for check in checks.iter() {
                                    {
                                        let (icon_name, icon_class) = match check.status {
                                            DiagnosticStatus::Pass => ("check-circle", "h-4 w-4 text-green-500"),
                                            DiagnosticStatus::Warn => ("alert-triangle", "h-4 w-4 text-yellow-500"),
                                            DiagnosticStatus::Fail => ("x-circle", "h-4 w-4 text-destructive"),
                                            DiagnosticStatus::Skipped => ("minus-circle", "h-4 w-4 text-muted-foreground"),
                                        };
                                        let label = check.label.clone();
                                        let detail = check.detail.clone();
                                        rsx! {
                                            div {
                                                key: "{label}",
                                                class: "flex items-start gap-3 rounded-md border p-3",
                                                Icon { name: icon_name.to_string(), class: icon_class.to_string() }
                                                div { class: "min-w-0 flex-1",
                                                    p { class: "text-sm font-medium", "{label}" }
                                                    p { class: "text-xs text-muted-foreground break-all", "{detail}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        Some(None) => rsx! {
                            p { class: "text-sm text-muted-foreground", "No server configured." }
                        },
                        None => rsx! { PageSkeleton {} },
                    }
                }
            }
            // Dev-only Pasion / MAS diagnostics panel
            PasionDiagnosticsPanel { base_url: base_url.clone() }
        }
    }
}

/// Dev-only diagnostics panel that probes Pasion/MAS endpoints
#[component]
fn PasionDiagnosticsPanel(base_url: String) -> Element {
    let mut expanded = use_signal(|| false);
    let mut running = use_signal(|| false);
    let mut results = use_signal(Vec::<DiagnosticCheck>::new);

    let handle_run = move |_: MouseEvent| {
        let url = base_url.clone();
        running.set(true);
        results.set(Vec::new());

        spawn(async move {
            let mut checks = Vec::new();

            // Check 1: Pasion site-config (MAS admin endpoint)
            let site_config_url = format!("{url}/_matrix/client/v3/login");
            match Request::get(&site_config_url)
                .header("Accept", "application/json")
                .send()
                .await
            {
                Ok(resp) if resp.status() < 400 => {
                    checks.push(DiagnosticCheck {
                        label: "Matrix Login Endpoint".to_string(),
                        status: DiagnosticStatus::Pass,
                        detail: format!("HTTP {} — reachable", resp.status()),
                    });
                }
                Ok(resp) => {
                    checks.push(DiagnosticCheck {
                        label: "Matrix Login Endpoint".to_string(),
                        status: DiagnosticStatus::Fail,
                        detail: format!("HTTP {}", resp.status()),
                    });
                }
                Err(e) => {
                    checks.push(DiagnosticCheck {
                        label: "Matrix Login Endpoint".to_string(),
                        status: DiagnosticStatus::Fail,
                        detail: format!("Unreachable: {e}"),
                    });
                }
            }

            // Check 2: Well-known endpoint
            let wk_url = format!("{url}/.well-known/matrix/client");
            match Request::get(&wk_url)
                .header("Accept", "application/json")
                .send()
                .await
            {
                Ok(resp) if resp.status() < 400 => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        let has_homeserver = json.get("m.homeserver").is_some();
                        let has_auth_issuer = json
                            .get("org.matrix.msc2965.authentication")
                            .or_else(|| json.get("m.authentication"))
                            .is_some();
                        checks.push(DiagnosticCheck {
                            label: "Well-Known Discovery".to_string(),
                            status: DiagnosticStatus::Pass,
                            detail: format!(
                                "homeserver: {}, delegated auth: {}",
                                has_homeserver, has_auth_issuer
                            ),
                        });

                        if has_auth_issuer {
                            let auth_block = json
                                .get("org.matrix.msc2965.authentication")
                                .or_else(|| json.get("m.authentication"));
                            if let Some(block) = auth_block {
                                let issuer = block
                                    .get("issuer")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                let account = block.get("account").and_then(|v| v.as_str());
                                checks.push(DiagnosticCheck {
                                    label: "Delegated Auth (well-known)".to_string(),
                                    status: DiagnosticStatus::Pass,
                                    detail: format!(
                                        "Issuer: {issuer}{}",
                                        account
                                            .map(|a| format!(", Account: {a}"))
                                            .unwrap_or_default()
                                    ),
                                });
                            }
                        }
                    } else {
                        checks.push(DiagnosticCheck {
                            label: "Well-Known Discovery".to_string(),
                            status: DiagnosticStatus::Warn,
                            detail: "Response not valid JSON".to_string(),
                        });
                    }
                }
                Ok(resp) => {
                    checks.push(DiagnosticCheck {
                        label: "Well-Known Discovery".to_string(),
                        status: if resp.status() == 404 {
                            DiagnosticStatus::Warn
                        } else {
                            DiagnosticStatus::Fail
                        },
                        detail: format!("HTTP {}", resp.status()),
                    });
                }
                Err(e) => {
                    checks.push(DiagnosticCheck {
                        label: "Well-Known Discovery".to_string(),
                        status: DiagnosticStatus::Warn,
                        detail: format!("Not available: {e}"),
                    });
                }
            }

            // Check 3: Registration endpoint
            let reg_url = format!("{url}/_matrix/client/v3/register");
            match Request::post(&reg_url)
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .body("{}")
                .unwrap()
                .send()
                .await
            {
                Ok(resp) => {
                    // 401 with session = registration available, 403 = disabled
                    let status_code = resp.status();
                    if status_code == 401 {
                        checks.push(DiagnosticCheck {
                            label: "Registration Endpoint".to_string(),
                            status: DiagnosticStatus::Pass,
                            detail: "Available (UIA required)".to_string(),
                        });
                    } else if status_code == 403 {
                        checks.push(DiagnosticCheck {
                            label: "Registration Endpoint".to_string(),
                            status: DiagnosticStatus::Warn,
                            detail: "Registration disabled or requires token".to_string(),
                        });
                    } else {
                        checks.push(DiagnosticCheck {
                            label: "Registration Endpoint".to_string(),
                            status: DiagnosticStatus::Pass,
                            detail: format!("HTTP {status_code}"),
                        });
                    }
                }
                Err(e) => {
                    checks.push(DiagnosticCheck {
                        label: "Registration Endpoint".to_string(),
                        status: DiagnosticStatus::Fail,
                        detail: format!("Unreachable: {e}"),
                    });
                }
            }

            results.set(checks);
            running.set(false);
        });
    };

    let is_expanded = *expanded.read();
    let is_running = *running.read();
    let current_results = results.read().clone();

    rsx! {
        Card {
            CardHeader { class: "flex flex-row items-center justify-between space-y-0".to_string(),
                div {
                    CardTitle {
                        div { class: "flex items-center gap-2",
                            Icon { name: "code".to_string(), class: "h-5 w-5".to_string() }
                            {t("auth_status.dev_diagnostics_title")}
                        }
                    }
                    CardDescription { {t("auth_status.dev_diagnostics_desc")} }
                }
                Button {
                    variant: ButtonVariant::Outline,
                    onclick: move |_| {
                        let v = *expanded.read();
                        expanded.set(!v);
                    },
                    if is_expanded { "Collapse" } else { "Expand" }
                }
            }
            if is_expanded {
                CardContent {
                    div { class: "space-y-4",
                        Button {
                            disabled: is_running,
                            onclick: handle_run,
                            if is_running {
                                Spinner { class: "mr-2".to_string() }
                                "Running diagnostics..."
                            } else {
                                "Run Diagnostics"
                            }
                        }

                        if !current_results.is_empty() {
                            div { class: "space-y-2",
                                for check in current_results.iter() {
                                    {
                                        let (icon_name, icon_class) = match check.status {
                                            DiagnosticStatus::Pass => ("check-circle", "h-4 w-4 text-green-500"),
                                            DiagnosticStatus::Warn => ("alert-triangle", "h-4 w-4 text-yellow-500"),
                                            DiagnosticStatus::Fail => ("x-circle", "h-4 w-4 text-destructive"),
                                            DiagnosticStatus::Skipped => ("minus-circle", "h-4 w-4 text-muted-foreground"),
                                        };
                                        let label = check.label.clone();
                                        let detail = check.detail.clone();
                                        rsx! {
                                            div {
                                                key: "{label}",
                                                class: "flex items-start gap-3 rounded-md border p-3",
                                                Icon { name: icon_name.to_string(), class: icon_class.to_string() }
                                                div { class: "min-w-0 flex-1",
                                                    p { class: "text-sm font-medium", "{label}" }
                                                    p { class: "text-xs text-muted-foreground break-all", "{detail}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CapabilityCard(
    label: String,
    enabled: bool,
    icon: &'static str,
    #[props(default)] loading: bool,
) -> Element {
    rsx! {
        div { class: "rounded-md border p-4 text-center",
            Icon {
                name: icon.to_string(),
                class: if enabled {
                    "h-6 w-6 mx-auto mb-2 text-green-500".to_string()
                } else {
                    "h-6 w-6 mx-auto mb-2 text-muted-foreground".to_string()
                },
            }
            p { class: "text-sm font-medium", "{label}" }
            if loading {
                Badge { variant: BadgeVariant::Outline, "Checking…" }
            } else if enabled {
                Badge { variant: BadgeVariant::Success, "Available" }
            } else {
                Badge { variant: BadgeVariant::Outline, "Not Available" }
            }
        }
    }
}
