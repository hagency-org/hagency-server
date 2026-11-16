use dioxus::prelude::*;

use crate::api::palpo_admin;
use crate::components::scheduled_commands::{RecurringCommandsList, ScheduledCommandsList};
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::Button;
use crate::components::ui::card::*;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::instance_config::get_palpo_admin_url;

#[component]
pub fn ServerActions() -> Element {
    let palpo_url = get_palpo_admin_url();

    let palpo_url_for_commands = palpo_url.clone();
    let mut commands_data = use_resource(move || {
        let url = palpo_url_for_commands.clone();
        async move {
            if let Some(url) = url {
                palpo_admin::get_server_commands(&url).await.ok()
            } else {
                None
            }
        }
    });

    let palpo_url_for_process = palpo_url.clone();
    let process_data = use_resource(move || {
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

    let is_locked = process_data
        .read()
        .as_ref()
        .and_then(|p| p.as_ref())
        .map(|p| p.command.is_some())
        .unwrap_or(false);

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: "Server Actions".to_string(),
                description: "Run maintenance commands on your server".to_string(),
            }

            // Show running command warning
            if is_locked {
                div { class: "rounded-md bg-yellow-500/10 border border-yellow-500/20 p-4",
                    div { class: "flex items-center gap-2",
                        div { class: "h-2 w-2 rounded-full bg-yellow-500 animate-pulse" }
                        p { class: "text-sm font-medium text-yellow-600 dark:text-yellow-400",
                            "A command is currently running. Please wait for it to complete."
                        }
                    }
                }
            }

            match &*commands_data.read() {
                Some(Some(commands)) => rsx! {
                    div { class: "grid gap-4 md:grid-cols-2",
                        for (key, command) in commands.iter() {
                            {
                                let cmd_name = key.clone();
                                let cmd_description = command.description.clone().unwrap_or_default();
                                let cmd_display_name = command.name.clone();
                                let requires_lock = command.with_lock;
                                let palpo_url_for_run = palpo_url.clone();

                                rsx! {
                                    Card {
                                        CardHeader {
                                            div { class: "flex items-center justify-between",
                                                CardTitle { class: "text-base".to_string(), "{cmd_display_name}" }
                                                if requires_lock {
                                                    Badge { variant: BadgeVariant::Secondary, "Requires Lock" }
                                                }
                                            }
                                            if !cmd_description.is_empty() {
                                                CardDescription { "{cmd_description}" }
                                            }
                                        }
                                        CardContent {
                                            Button {
                                                disabled: is_locked,
                                                onclick: move |_| {
                                                    let url = palpo_url_for_run.clone();
                                                    let name = cmd_name.clone();
                                                    spawn(async move {
                                                        if let Some(url) = url {
                                                            match palpo_admin::run_server_command(&url, &name, None).await {
                                                                Ok(_) => show_toast("Command started successfully", ToastVariant::Success),
                                                                Err(e) => { show_toast(&format!("Failed: {}", e.message), ToastVariant::Error);
                                                                    crate::components::ui::notifications::add_typed_notification(&format!("Maintenance command {name} failed: {}", e.message),crate::components::ui::notifications::NotificationSeverity::Warning,Some(crate::components::ui::notifications::NotificationType::JobFailure));
                                                                },
                                                            }
                                                        }
                                                    });
                                                },
                                                "Run"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Some(None) => rsx! {
                    div { class: "rounded-md bg-destructive/10 p-6 text-center",
                        p { class: "text-sm text-destructive mb-2", "Unable to fetch server commands." }
                        button {
                            class: "text-sm font-medium text-primary hover:underline",
                            onclick: move |_| commands_data.restart(),
                            "Retry"
                        }
                    }
                },
                None => rsx! { PageSkeleton {} },
            }

            // Scheduled commands
            if let Some(ref url) = palpo_url {
                ScheduledCommandsList { palpo_admin_url: url.clone() }
            }

            // Recurring commands
            if let Some(ref url) = palpo_url {
                RecurringCommandsList { palpo_admin_url: url.clone() }
            }
        }
    }
}
