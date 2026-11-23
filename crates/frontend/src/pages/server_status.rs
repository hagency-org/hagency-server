use dioxus::prelude::*;

use crate::api::palpo_admin;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::PageHeader;
use crate::utils::instance_config::get_palpo_admin_url;

#[component]
pub fn ServerStatus() -> Element {
    let palpo_url = get_palpo_admin_url();

    let palpo_url_for_resource = palpo_url.clone();
    let mut status_data = use_resource(move || {
        let url = palpo_url_for_resource.clone();
        async move {
            if let Some(url) = url {
                palpo_admin::get_server_status(&url).await.ok()
            } else {
                None
            }
        }
    });

    let palpo_url_for_process = palpo_url.clone();
    let mut process_data = use_resource(move || {
        let url = palpo_url_for_process.clone();
        async move {
            if let Some(url) = url {
                palpo_admin::get_server_running_process(&url).await.ok()
            } else {
                None
            }
        }
    });

    if palpo_url.is_none() {
        return rsx! {
            div { class: "text-center p-8",
                p { class: "text-muted-foreground", "Palpo admin is not configured." }
            }
        };
    }

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: "Server Status".to_string(),
                description: "Monitor your server's health".to_string(),
                Button {
                    variant: ButtonVariant::Outline,
                    onclick: move |_| { status_data.restart(); process_data.restart(); },
                    "Refresh"
                }
            }

            match &*status_data.read() {
                Some(Some(status)) => {
                    let overall_ok = status.ok;
                    let in_maintenance = status.maintenance;
                    let host = status.host.clone().unwrap_or_default();

                    rsx! {
                        // Overall status
                        div { class: "flex items-center gap-4",
                            if overall_ok {
                                Badge { variant: BadgeVariant::Success, class: "text-base px-4 py-1".to_string(), "Healthy" }
                            } else {
                                Badge { variant: BadgeVariant::Destructive, class: "text-base px-4 py-1".to_string(), "Issues Detected" }
                            }
                            if in_maintenance {
                                Badge { variant: BadgeVariant::Secondary, class: "text-base px-4 py-1".to_string(), "Maintenance Mode" }
                            }
                            if !host.is_empty() {
                                span { class: "text-sm text-muted-foreground", "Host: {host}" }
                            }
                        }

                        // Running process
                        if let Some(Some(process)) = &*process_data.read() {
                            if let Some(ref command) = process.command {
                                Card {
                                    CardHeader {
                                        CardTitle { class: "text-base".to_string(), "Currently Running" }
                                    }
                                    CardContent {
                                        div { class: "flex items-center gap-2",
                                            div { class: "h-2 w-2 rounded-full bg-yellow-500 animate-pulse" }
                                            span { class: "font-medium", "{command}" }
                                            if let Some(ref locked_at) = process.locked_at {
                                                span { class: "text-sm text-muted-foreground", "started {locked_at}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Component results
                        div { class: "grid gap-4 md:grid-cols-2 lg:grid-cols-3",
                            for component in status.results.iter() {
                                Card {
                                    CardContent { class: "p-4".to_string(),
                                        div { class: "flex items-center justify-between",
                                            div { class: "space-y-1",
                                                if let Some(ref label) = component.label {
                                                    p { class: "font-medium", "{label}" }
                                                }
                                                if let Some(ref category) = component.category {
                                                    p { class: "text-xs text-muted-foreground", "{category}" }
                                                }
                                                if !component.ok {
                                                    if let Some(ref reason) = component.reason {
                                                        p { class: "text-xs text-destructive mt-1", "{reason}" }
                                                    }
                                                }
                                            }
                                            if component.ok {
                                                Badge { variant: BadgeVariant::Success, "OK" }
                                            } else {
                                                Badge { variant: BadgeVariant::Destructive, "Error" }
                                            }
                                        }
                                        if !component.ok {
                                            if let Some(ref help) = component.help {
                                                a {
                                                    href: "{help}",
                                                    target: "_blank",
                                                    class: "text-xs text-primary hover:underline mt-2 block",
                                                    "Help"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Some(None) => rsx! {
                    div { class: "text-center p-8",
                        p { class: "text-muted-foreground", "Unable to fetch server status." }
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}
