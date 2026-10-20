use dioxus::prelude::*;
use futures::stream::{self, StreamExt};

use crate::api::users;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::Spinner;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::i18n::t;
use crate::utils::storage;

const NOTICE_HISTORY_KEY: &str = "server_notice_history";
const MAX_HISTORY_ENTRIES: usize = 50;

/// Max in-flight server-notice sends during a broadcast. Bounds concurrency so
/// broadcasting to a large user base doesn't flood the homeserver.
const BROADCAST_CONCURRENCY: usize = 8;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct NoticeHistoryEntry {
    user_id: String,
    message: String,
    timestamp: String,
    event_id: String,
}

fn load_notice_history() -> Vec<NoticeHistoryEntry> {
    storage::get_item(NOTICE_HISTORY_KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn save_notice_history(entries: &[NoticeHistoryEntry]) {
    if let Ok(json) = serde_json::to_string(entries) {
        storage::set_item(NOTICE_HISTORY_KEY, &json);
    }
}

fn add_notice_to_history(user_id: &str, message: &str, event_id: &str) {
    let mut history = load_notice_history();
    let now = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    history.insert(
        0,
        NoticeHistoryEntry {
            user_id: user_id.to_string(),
            message: message.to_string(),
            timestamp: now,
            event_id: event_id.to_string(),
        },
    );
    if history.len() > MAX_HISTORY_ENTRIES {
        history.truncate(MAX_HISTORY_ENTRIES);
    }
    save_notice_history(&history);
}

fn clear_notice_history() {
    storage::remove_item(NOTICE_HISTORY_KEY);
}

#[component]
pub fn ServerNotices() -> Element {
    let mut user_id = use_signal(String::new);
    let mut message = use_signal(String::new);
    let mut sending = use_signal(|| false);
    let mut broadcast_mode = use_signal(|| false);
    let mut broadcast_progress = use_signal(|| Option::<(usize, usize)>::None);
    let mut history_revision = use_signal(|| 0u32);

    let is_sending = *sending.read();
    let is_broadcast = *broadcast_mode.read();
    let progress = *broadcast_progress.read();

    // Read history (re-read when history_revision changes)
    let _rev = *history_revision.read();
    let history = load_notice_history();

    let handle_send = move |_: MouseEvent| {
        let uid = user_id.read().clone();
        let msg = message.read().clone();
        let broadcast = *broadcast_mode.read();

        if msg.is_empty() {
            show_toast("Message cannot be empty", ToastVariant::Error);
            return;
        }
        if !broadcast && uid.is_empty() {
            show_toast("User ID cannot be empty", ToastVariant::Error);
            return;
        }

        sending.set(true);

        if broadcast {
            spawn(async move {
                // Fetch all users
                broadcast_progress.set(Some((0, 0)));
                let all_users = match users::get_users(1, 10000, "name", "asc", "").await {
                    Ok(data) => data.data,
                    Err(e) => {
                        show_toast(
                            &format!("Failed to fetch users: {}", e.message),
                            ToastVariant::Error,
                        );
                        sending.set(false);
                        broadcast_progress.set(None);
                        return;
                    }
                };

                let total = all_users.len();
                let mut success_count = 0usize;
                let mut fail_count = 0usize;
                let mut completed = 0usize;

                // Send with bounded concurrency instead of one-at-a-time.
                // Each task returns the user id, the message, and the result
                // so history is recorded on the main task as completions
                // stream in (keeps localStorage writes single-threaded).
                let msg_ref = &msg;
                let mut stream = stream::iter(all_users.iter())
                    .map(|user| async move {
                        let result = users::send_server_notice(&user.id, msg_ref).await;
                        (user.id.clone(), result)
                    })
                    .buffer_unordered(BROADCAST_CONCURRENCY);

                while let Some((user_id, result)) = stream.next().await {
                    completed += 1;
                    broadcast_progress.set(Some((completed, total)));
                    match result {
                        Ok(event_id) => {
                            add_notice_to_history(&user_id, &msg, &event_id);
                            success_count += 1;
                        }
                        Err(_) => fail_count += 1,
                    }
                }

                if fail_count == 0 {
                    show_toast(
                        &format!("Broadcast sent to {} users", success_count),
                        ToastVariant::Success,
                    );
                } else {
                    show_toast(
                        &format!(
                            "Broadcast: {success_count} sent, {fail_count} failed out of {total}"
                        ),
                        ToastVariant::Error,
                    );
                }
                message.set(String::new());
                sending.set(false);
                broadcast_progress.set(None);
                history_revision += 1;
            });
        } else {
            spawn(async move {
                match users::send_server_notice(&uid, &msg).await {
                    Ok(event_id) => {
                        add_notice_to_history(&uid, &msg, &event_id);
                        show_toast(
                            &format!("Server notice sent (event: {})", event_id),
                            ToastVariant::Success,
                        );
                        user_id.set(String::new());
                        message.set(String::new());
                    }
                    Err(e) => {
                        show_toast(
                            &format!("Failed to send notice: {}", e.message),
                            ToastVariant::Error,
                        );
                    }
                }
                sending.set(false);
                history_revision += 1;
            });
        }
    };

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("server_notices.title"),
                description: t("server_notices.subtitle"),
            }

            Card {
                CardHeader {
                    CardTitle { {t("server_notices.send_title")} }
                    CardDescription {
                        if is_broadcast {
                            {t("server_notices.broadcast_desc")}
                        } else {
                            {t("server_notices.single_desc")}
                        }
                    }
                }
                CardContent {
                    div { class: "space-y-4 max-w-2xl",
                        // Mode toggle
                        div { class: "server-notice-mode-toggle rounded-lg border border-input p-1 w-fit",
                            button {
                                class: if !is_broadcast {
                                    "px-3 py-1.5 text-sm font-medium rounded bg-primary text-primary-foreground touch-target"
                                } else {
                                    "px-3 py-1.5 text-sm font-medium rounded text-muted-foreground hover:text-foreground touch-target"
                                },
                                disabled: is_sending,
                                onclick: move |_| broadcast_mode.set(false),
                                {t("server_notices.single_user")}
                            }
                            button {
                                class: if is_broadcast {
                                    "px-3 py-1.5 text-sm font-medium rounded bg-primary text-primary-foreground touch-target"
                                } else {
                                    "px-3 py-1.5 text-sm font-medium rounded text-muted-foreground hover:text-foreground touch-target"
                                },
                                disabled: is_sending,
                                onclick: move |_| broadcast_mode.set(true),
                                {t("server_notices.broadcast")}
                            }
                        }

                        // User ID input (only in single user mode)
                        if !is_broadcast {
                            div { class: "space-y-2",
                                Label { r#for: "user_id".to_string(), {t("server_notices.user_id")} }
                                Input {
                                    placeholder: "@user:example.com".to_string(),
                                    value: user_id.read().clone(),
                                    oninput: move |evt: FormEvent| user_id.set(evt.value()),
                                    disabled: is_sending,
                                }
                            }
                        }

                        div { class: "space-y-2",
                            Label { r#for: "message".to_string(), {t("server_notices.message")} }
                            textarea {
                                class: "flex min-h-[120px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50 touch-target",
                                placeholder: t("server_notices.message_placeholder"),
                                value: message.read().clone(),
                                disabled: is_sending,
                                oninput: move |evt: FormEvent| message.set(evt.value()),
                            }
                        }

                        // Progress indicator for broadcast
                        if let Some((current, total)) = progress {
                            div { class: "flex items-center gap-2 text-sm text-muted-foreground",
                                Spinner { class: "h-4 w-4".to_string() }
                                "Sending to {current} of {total} users..."
                            }
                        }

                        div { class: "responsive-action-row",
                            Button {
                                disabled: is_sending,
                                onclick: handle_send,
                                if is_sending {
                                    Spinner { class: "mr-2".to_string() }
                                }
                                if is_broadcast {
                                    {t("server_notices.broadcast_notice")}
                                } else {
                                    {t("server_notices.send_notice")}
                                }
                            }
                        }
                    }
                }
            }

            // Notice History
            Card {
                CardHeader { class: "flex flex-row items-center justify-between space-y-0".to_string(),
                    div {
                        CardTitle { {t("server_notices.history_title")} }
                        CardDescription { {t("server_notices.history_desc")} }
                    }
                    if !history.is_empty() {
                        Button {
                            variant: ButtonVariant::Outline,
                            onclick: move |_| {
                                clear_notice_history();
                                history_revision += 1;
                                show_toast("History cleared", ToastVariant::Success);
                            },
                            Icon { name: "trash".to_string(), class: "h-4 w-4 mr-1".to_string() }
                            {t("server_notices.clear_history")}
                        }
                    }
                }
                CardContent {
                    if history.is_empty() {
                        p { class: "text-sm text-muted-foreground", {t("server_notices.no_notices")} }
                    } else {
                        div { class: "space-y-1",
                            // Table header
                            div { class: "grid grid-cols-[1fr_2fr_auto_auto] gap-4 px-3 py-2 text-xs font-medium text-muted-foreground border-b",
                                span { {t("server_notices.user")} }
                                span { {t("server_notices.message")} }
                                span { {t("server_notices.timestamp")} }
                                span { {t("server_notices.event_id")} }
                            }
                            for entry in history.iter() {
                                {
                                    let truncated_msg = if entry.message.chars().count() > 80 {
                                        format!("{}...", entry.message.chars().take(80).collect::<String>())
                                    } else {
                                        entry.message.clone()
                                    };
                                    let truncated_event_id = if entry.event_id.chars().count() > 20 {
                                        format!("{}...", entry.event_id.chars().take(20).collect::<String>())
                                    } else {
                                        entry.event_id.clone()
                                    };
                                    rsx! {
                                        div {
                                            key: "{entry.event_id}-{entry.timestamp}",
                                            class: "grid grid-cols-[1fr_2fr_auto_auto] gap-4 px-3 py-2 text-sm border-b last:border-0 hover:bg-muted/50",
                                            span { class: "font-mono text-xs truncate", "{entry.user_id}" }
                                            span { class: "truncate", "{truncated_msg}" }
                                            span { class: "text-xs text-muted-foreground whitespace-nowrap", "{entry.timestamp}" }
                                            span { class: "font-mono text-xs text-muted-foreground", title: "{entry.event_id}", "{truncated_event_id}" }
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
