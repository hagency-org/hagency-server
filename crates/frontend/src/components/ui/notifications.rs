use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationSeverity {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationType {
    ReportFiled,
    UserRegistered,
    SystemInfo,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u64,
    pub message: String,
    pub severity: NotificationSeverity,
    pub timestamp: String,
    pub read: bool,
}

static NOTIFICATION_COUNTER: GlobalSignal<u64> = GlobalSignal::new(|| 0);
pub static NOTIFICATIONS: GlobalSignal<Vec<Notification>> = GlobalSignal::new(Vec::new);

pub fn add_typed_notification(
    message: &str,
    severity: NotificationSeverity,
    ntype: Option<NotificationType>,
) {
    // Check if this notification type is enabled in preferences
    if let Some(ref nt) = ntype
        && !crate::pages::notification_preferences::is_notification_type_enabled(nt)
    {
        return;
    }

    let id = {
        let mut counter = NOTIFICATION_COUNTER.write();
        *counter += 1;
        *counter
    };

    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let mut notifications = NOTIFICATIONS.write();
    notifications.insert(
        0,
        Notification {
            id,
            message: message.to_string(),
            severity,
            timestamp,
            read: false,
        },
    );

    // Keep at most 50 notifications
    if notifications.len() > 50 {
        notifications.truncate(50);
    }
}

pub fn mark_all_read() {
    let mut notifications = NOTIFICATIONS.write();
    for n in notifications.iter_mut() {
        n.read = true;
    }
}

pub fn clear_notifications() {
    let mut notifications = NOTIFICATIONS.write();
    notifications.clear();
}

pub fn unread_count() -> usize {
    let notifications = NOTIFICATIONS.read();
    notifications.iter().filter(|n| !n.read).count()
}
