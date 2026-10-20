use dioxus::prelude::*;

use crate::components::ui::card::*;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::storage;

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationPreferences {
    pub report_filed: bool,
    pub user_registered: bool,
    pub federation_alert: bool,
    pub job_failure: bool,
    pub system_info: bool,
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            report_filed: true,
            user_registered: true,
            federation_alert: true,
            job_failure: true,
            system_info: true,
        }
    }
}

const STORAGE_KEY: &str = "notification_preferences";

pub static NOTIFICATION_PREFS: GlobalSignal<NotificationPreferences> =
    GlobalSignal::new(load_preferences);

fn load_preferences() -> NotificationPreferences {
    storage::get_item(STORAGE_KEY)
        .and_then(|s| serde_json::from_str::<PrefsJson>(&s).ok())
        .map(|p| NotificationPreferences {
            report_filed: p.report_filed,
            user_registered: p.user_registered,
            federation_alert: p.federation_alert,
            job_failure: p.job_failure,
            system_info: p.system_info,
        })
        .unwrap_or_default()
}

fn save_preferences(prefs: &NotificationPreferences) {
    let json = PrefsJson {
        report_filed: prefs.report_filed,
        user_registered: prefs.user_registered,
        federation_alert: prefs.federation_alert,
        job_failure: prefs.job_failure,
        system_info: prefs.system_info,
    };
    if let Ok(s) = serde_json::to_string(&json) {
        storage::set_item(STORAGE_KEY, &s);
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PrefsJson {
    report_filed: bool,
    user_registered: bool,
    federation_alert: bool,
    job_failure: bool,
    system_info: bool,
}

pub fn is_notification_type_enabled(
    ntype: &crate::components::ui::notifications::NotificationType,
) -> bool {
    let prefs = NOTIFICATION_PREFS.read();
    match ntype {
        crate::components::ui::notifications::NotificationType::ReportFiled => prefs.report_filed,
        crate::components::ui::notifications::NotificationType::UserRegistered => {
            prefs.user_registered
        }
        crate::components::ui::notifications::NotificationType::SystemInfo => prefs.system_info,
    }
}

struct ToggleItem {
    label: &'static str,
    description: &'static str,
    key: &'static str,
}

#[component]
pub fn NotificationPreferencesPage() -> Element {
    let mut prefs = use_signal(|| (*NOTIFICATION_PREFS.read()).clone());

    let items = [
        ToggleItem {
            label: "Report Filed",
            description: "Notifications when users file event reports",
            key: "report_filed",
        },
        ToggleItem {
            label: "User Registered",
            description: "Notifications when new users register",
            key: "user_registered",
        },
        ToggleItem {
            label: "Federation Alert",
            description: "Alerts about federation issues with remote servers",
            key: "federation_alert",
        },
        ToggleItem {
            label: "Job Failure",
            description: "Notifications when background jobs fail",
            key: "job_failure",
        },
        ToggleItem {
            label: "System Info",
            description: "General system information notifications",
            key: "system_info",
        },
    ];

    let get_value = |key: &str, p: &NotificationPreferences| -> bool {
        match key {
            "report_filed" => p.report_filed,
            "user_registered" => p.user_registered,
            "federation_alert" => p.federation_alert,
            "job_failure" => p.job_failure,
            "system_info" => p.system_info,
            _ => true,
        }
    };

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: "Notification Preferences".to_string(),
                description: "Configure which notification types you want to receive".to_string(),
            }

            Card {
                CardHeader {
                    CardTitle { "Notification Types" }
                    CardDescription { "Toggle individual notification types on or off" }
                }
                CardContent {
                    div { class: "space-y-4",
                        for item in items.iter() {
                            {
                                let key = item.key;
                                let current_prefs = prefs.read().clone();
                                let is_enabled = get_value(key, &current_prefs);
                                rsx! {
                                    div { class: "flex items-center justify-between py-3 border-b last:border-b-0",
                                        div {
                                            p { class: "text-sm font-medium", "{item.label}" }
                                            p { class: "text-xs text-muted-foreground", "{item.description}" }
                                        }
                                        button {
                                            class: if is_enabled {
                                                "relative inline-flex h-6 w-11 items-center rounded-full bg-primary transition-colors"
                                            } else {
                                                "relative inline-flex h-6 w-11 items-center rounded-full bg-muted transition-colors"
                                            },
                                            onclick: move |_| {
                                                let mut p = prefs.read().clone();
                                                match key {
                                                    "report_filed" => p.report_filed = !p.report_filed,
                                                    "user_registered" => p.user_registered = !p.user_registered,
                                                    "federation_alert" => p.federation_alert = !p.federation_alert,
                                                    "job_failure" => p.job_failure = !p.job_failure,
                                                    "system_info" => p.system_info = !p.system_info,
                                                    _ => {}
                                                }
                                                save_preferences(&p);
                                                *NOTIFICATION_PREFS.write() = p.clone();
                                                prefs.set(p);
                                                show_toast("Preferences saved", ToastVariant::Success);
                                            },
                                            span {
                                                class: if is_enabled {
                                                    "inline-block h-4 w-4 transform rounded-full bg-white transition-transform translate-x-6"
                                                } else {
                                                    "inline-block h-4 w-4 transform rounded-full bg-white transition-transform translate-x-1"
                                                },
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
