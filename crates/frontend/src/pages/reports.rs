use dioxus::prelude::*;

use crate::api::{reports, rooms};
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::dialog::ConfirmDialog;
use crate::components::ui::empty_state::EmptyState;
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::icons::Icon;
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::notifications::{
    NotificationSeverity, NotificationType, add_typed_notification,
};
use crate::components::ui::page_header::{BreadcrumbItem, Breadcrumbs, PageHeader};
use crate::components::ui::pagination::Pagination;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::router::Route;
use crate::utils::date::format_timestamp_short;
use crate::utils::i18n::t;

#[derive(Debug, Clone, PartialEq)]
enum ReportStatus {
    New,
    InReview,
    Resolved,
}

impl ReportStatus {
    fn as_api_value(&self) -> &'static str {
        match self {
            ReportStatus::New => "new",
            ReportStatus::InReview => "in_review",
            ReportStatus::Resolved => "resolved",
        }
    }

    fn display(&self) -> String {
        match self {
            ReportStatus::New => t("reports.new"),
            ReportStatus::InReview => t("reports.in_review"),
            ReportStatus::Resolved => t("reports.resolved"),
        }
    }

    fn from_api_value(s: &str) -> Self {
        match s {
            "in_review" | "In Review" => ReportStatus::InReview,
            "resolved" | "Resolved" => ReportStatus::Resolved,
            _ => ReportStatus::New,
        }
    }

    fn badge_variant(&self) -> BadgeVariant {
        match self {
            ReportStatus::New => BadgeVariant::Default,
            ReportStatus::InReview => BadgeVariant::Secondary,
            ReportStatus::Resolved => BadgeVariant::Success,
        }
    }
}

const PAGE_SIZE: u64 = 25;

#[component]
pub fn ReportList() -> Element {
    let mut page = use_signal(|| 1u64);
    let mut notified = use_signal(|| false);
    let page_val = *page.read();

    let mut reports_data = use_resource(move || async move {
        notified.set(false);
        reports::get_reports_cached(page_val, PAGE_SIZE, "received_ts", "desc").await
    });

    // Notify when new reports are loaded
    if !*notified.read()
        && let Some(Ok(data)) = &*reports_data.read()
        && !data.data.is_empty()
        && page_val == 1
    {
        add_typed_notification(
            &format!("{} event report(s) pending review", data.total),
            NotificationSeverity::Warning,
            Some(NotificationType::ReportFiled),
        );
        notified.set(true);
    }

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("reports.title"),
                description: t("reports.subtitle"),
            }

            match &*reports_data.read() {
                Some(Ok(data)) => rsx! {
                    div { class: "rounded-md border",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { {t("reports.id")} }
                                    TableHead { {t("reports.status")} }
                                    TableHead { {t("reports.reporter")} }
                                    TableHead { {t("reports.room")} }
                                    TableHead { {t("reports.reason")} }
                                    TableHead { {t("reports.received")} }
                                }
                            }
                            TableBody {
                                if data.data.is_empty() {
                                    TableRow {
                                        TableCell { class: "p-0".to_string(), colspan: 99,
                                            EmptyState {
                                                icon: "flag".to_string(),
                                                title: t("reports.no_reports"),
                                                description: t("reports.no_reports_description"),
                                            }
                                        }
                                    }
                                } else {
                                    for report in data.data.iter() {
                                        {
                                            let id = report.id;
                                            let user_id = report.user_id.clone();
                                            let room_id = report.room_id.clone();
                                            let reason = report.reason.clone().unwrap_or_else(|| "-".to_string());
                                            let received = format_timestamp_short(report.received_ts);
                                            let status = ReportStatus::from_api_value(&report.status);
                                            let status_label = status.display();
                                            let status_variant = status.badge_variant();

                                            rsx! {
                                                TableRow {
                                                    key: "{id}",
                                                    TableCell {
                                                        Link {
                                                            to: Route::ReportShow { report_id: id.to_string() },
                                                            class: "font-medium text-primary hover:underline",
                                                            "#{id}"
                                                        }
                                                    }
                                                    TableCell {
                                                        Badge { variant: status_variant, "{status_label}" }
                                                    }
                                                    TableCell { class: "max-w-[200px] truncate".to_string(),
                                                        Link {
                                                            to: Route::UserShow { user_id: urlencoding::encode(&user_id).to_string() },
                                                            class: "text-primary hover:underline",
                                                            "{user_id}"
                                                        }
                                                    }
                                                    TableCell { class: "max-w-[200px] truncate".to_string(),
                                                        Link {
                                                            to: Route::RoomShow { room_id: urlencoding::encode(&room_id).to_string() },
                                                            class: "text-primary hover:underline",
                                                            "{room_id}"
                                                        }
                                                    }
                                                    TableCell { class: "max-w-[300px] truncate".to_string(), "{reason}" }
                                                    TableCell { class: "text-muted-foreground".to_string(), "{received}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    Pagination {
                        page: page_val,
                        total: data.total,
                        per_page: PAGE_SIZE,
                        on_page_change: move |p| page.set(p),
                    }
                },
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| reports_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}

#[component]
pub fn ReportShow(report_id: String) -> Element {
    let nav = use_navigator();

    let report_id_parsed: u64 = report_id.parse().unwrap_or(0);

    let mut report_data =
        use_resource(move || async move { reports::get_report(report_id_parsed).await });

    let mut show_delete_dialog = use_signal(|| false);
    let mut redact_loading = use_signal(|| false);
    let mut ban_loading = use_signal(|| false);
    let mut block_loading = use_signal(|| false);
    let mut current_status = use_signal(|| Option::<ReportStatus>::None);
    let mut status_loading = use_signal(|| false);

    rsx! {
        div { class: "space-y-6",
            Breadcrumbs {
                items: vec![
                    BreadcrumbItem { label: t("reports.title"), href: Some("/admin/reports".to_string()) },
                    BreadcrumbItem { label: format!("Report #{report_id}"), href: None },
                ],
            }

            match &*report_data.read() {
                Some(Ok(report)) => {
                    let id = report.id;
                    let user_id = report.user_id.clone();
                    let room_id = report.room_id.clone();
                    let event_id = report.event_id.clone();
                    let reason = report.reason.clone().unwrap_or_else(|| "-".to_string());
                    let score = report.score.map(|s| s.to_string()).unwrap_or_else(|| "-".to_string());
                    let sender = report.sender.clone().unwrap_or_else(|| "-".to_string());
                    let received = format_timestamp_short(report.received_ts);
                    let event_json = report.event_json.clone();

                    let room_id_for_redact = room_id.clone();
                    let event_id_for_redact = event_id.clone();
                    let room_id_for_ban = room_id.clone();
                    let room_id_for_block = room_id.clone();
                    let sender_for_ban = sender.clone();
                    let is_redact_loading = *redact_loading.read();
                    let is_ban_loading = *ban_loading.read();
                    let is_block_loading = *block_loading.read();
                    let effective_status = current_status
                        .read()
                        .clone()
                        .unwrap_or_else(|| ReportStatus::from_api_value(&report.status));
                    let is_status_loading = *status_loading.read();
                    let status_value = effective_status.as_api_value().to_string();

                    rsx! {
                        PageHeader {
                            title: format!("Report #{id}"),
                            description: format!("Received {received}"),
                            div { class: "page-header-inline-control",
                                label { class: "text-sm font-medium text-muted-foreground", "Status:" }
                                select {
                                    class: "rounded-md border bg-background px-3 py-1.5 text-sm touch-target",
                                    disabled: is_status_loading,
                                    value: "{status_value}",
                                    onchange: move |evt: Event<FormData>| {
                                        let new_status = ReportStatus::from_api_value(&evt.value());
                                        let status_for_api = new_status.as_api_value().to_string();
                                        status_loading.set(true);
                                        spawn(async move {
                                            match reports::update_report_status(id, &status_for_api).await {
                                                Ok(updated) => {
                                                    current_status.set(Some(ReportStatus::from_api_value(&updated.status)));
                                                    report_data.restart();
                                                    show_toast("Status updated", ToastVariant::Success);
                                                }
                                                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                            }
                                            status_loading.set(false);
                                        });
                                    },
                                    option { value: "new", {t("reports.new")} }
                                    option { value: "in_review", {t("reports.in_review")} }
                                    option { value: "resolved", {t("reports.resolved")} }
                                }
                                if is_status_loading {
                                    Spinner { class: "ml-2".to_string() }
                                }
                            }
                        }

                        div { class: "grid gap-6 md:grid-cols-2",
                            Card {
                                CardHeader { CardTitle { {t("reports.report_details")} } }
                                CardContent {
                                    div { class: "space-y-4",
                                        // Reporter with link
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("reports.reporter_user_id")} }
                                            Link {
                                                to: Route::UserShow { user_id: urlencoding::encode(&user_id).to_string() },
                                                class: "text-sm text-primary hover:underline max-w-[60%] text-right break-all",
                                                "{user_id}"
                                            }
                                        }
                                        // Room with link
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("reports.room_id")} }
                                            Link {
                                                to: Route::RoomShow { room_id: urlencoding::encode(&room_id).to_string() },
                                                class: "text-sm text-primary hover:underline max-w-[60%] text-right break-all",
                                                "{room_id}"
                                            }
                                        }
                                        InfoRow { label: t("reports.event_id"), value: event_id }
                                        // Sender with link
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("reports.sender")} }
                                            if sender != "-" {
                                                Link {
                                                    to: Route::UserShow { user_id: urlencoding::encode(&sender).to_string() },
                                                    class: "text-sm text-primary hover:underline max-w-[60%] text-right break-all",
                                                    "{sender}"
                                                }
                                            } else {
                                                span { class: "text-sm", "-" }
                                            }
                                        }
                                        InfoRow { label: t("reports.score"), value: score }
                                        InfoRow { label: t("reports.received"), value: received }
                                    }
                                }
                            }

                            Card {
                                CardHeader { CardTitle { {t("reports.reason")} } }
                                CardContent {
                                    p { class: "text-sm whitespace-pre-wrap", "{reason}" }
                                }
                            }
                        }

                        Card {
                            CardHeader { CardTitle { {t("reports.event_content")} } }
                            CardContent {
                                match event_json {
                                    Some(ref json) => rsx! {
                                        pre {
                                            class: "text-xs font-mono bg-muted p-3 rounded overflow-auto max-h-96",
                                            {serde_json::to_string_pretty(json).unwrap_or_else(|_| "{}".to_string())}
                                        }
                                    },
                                    None => rsx! {
                                        p { class: "text-sm text-muted-foreground", {t("reports.no_event_content")} }
                                    },
                                }
                            }
                        }

                        // Moderation Actions
                        Card {
                            CardHeader {
                                CardTitle { {t("reports.moderation_actions")} }
                            }
                            CardContent {
                                div { class: "flex flex-wrap gap-2",
                                    Button {
                                        variant: ButtonVariant::Outline,
                                        disabled: is_redact_loading || event_id_for_redact.is_empty(),
                                        onclick: {
                                            let rid = room_id_for_redact.clone();
                                            let eid = event_id_for_redact.clone();
                                            move |_| {
                                                let rid = rid.clone();
                                                let eid = eid.clone();
                                                redact_loading.set(true);
                                                let txn_id = format!("redact_{}", js_sys::Date::now() as u64);
                                                spawn(async move {
                                                    match rooms::redact_event(&rid, &eid, &txn_id, "Removed by admin").await {
                                                        Ok(_) => show_toast("Event redacted successfully", ToastVariant::Success),
                                                        Err(e) => show_toast(&format!("Failed to redact: {}", e.message), ToastVariant::Error),
                                                    }
                                                    redact_loading.set(false);
                                                });
                                            }
                                        },
                                        if is_redact_loading { Spinner { class: "mr-1".to_string() } }
                                        Icon { name: "x".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                        {t("reports.redact")}
                                    }
                                    Button {
                                        variant: ButtonVariant::Outline,
                                        disabled: is_ban_loading || sender_for_ban == "-",
                                        onclick: {
                                            let rid = room_id_for_ban.clone();
                                            let uid = sender_for_ban.clone();
                                            move |_| {
                                                let rid = rid.clone();
                                                let uid = uid.clone();
                                                ban_loading.set(true);
                                                spawn(async move {
                                                    match rooms::ban_user(&rid, &uid, "Banned by admin following report").await {
                                                        Ok(_) => show_toast("User banned from room", ToastVariant::Success),
                                                        Err(e) => show_toast(&format!("Failed to ban: {}", e.message), ToastVariant::Error),
                                                    }
                                                    ban_loading.set(false);
                                                });
                                            }
                                        },
                                        if is_ban_loading { Spinner { class: "mr-1".to_string() } }
                                        Icon { name: "shield".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                        {t("reports.ban_user")}
                                    }
                                    Button {
                                        variant: ButtonVariant::Outline,
                                        disabled: is_block_loading || room_id_for_block.is_empty(),
                                        onclick: {
                                            let rid = room_id_for_block.clone();
                                            move |_| {
                                                let rid = rid.clone();
                                                block_loading.set(true);
                                                spawn(async move {
                                                    match rooms::block_room(&rid, true).await {
                                                        Ok(_) => show_toast("Room blocked successfully", ToastVariant::Success),
                                                        Err(e) => show_toast(&format!("Failed to block room: {}", e.message), ToastVariant::Error),
                                                    }
                                                    block_loading.set(false);
                                                });
                                            }
                                        },
                                        if is_block_loading { Spinner { class: "mr-1".to_string() } }
                                        Icon { name: "shield".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                        {t("reports.block_room")}
                                    }
                                    Button {
                                        variant: ButtonVariant::Destructive,
                                        onclick: move |_| show_delete_dialog.set(true),
                                        Icon { name: "trash".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                        {t("reports.delete_report")}
                                    }
                                }
                            }
                        }

                        ConfirmDialog {
                            open: *show_delete_dialog.read(),
                            title: t("reports.delete_report"),
                            description: format!("Are you sure you want to delete report #{id}? This action cannot be undone."),
                            confirm_text: t("reports.delete_report"),
                            destructive: true,
                            on_confirm: move |_| {
                                show_delete_dialog.set(false);
                                spawn(async move {
                                    match reports::delete_report(id).await {
                                        Ok(_) => {
                                            show_toast("Report deleted", ToastVariant::Success);
                                            nav.push(Route::ReportList {});
                                        }
                                        Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                    }
                                });
                            },
                            on_cancel: move |_| show_delete_dialog.set(false),
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| report_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}

#[component]
fn InfoRow(label: String, value: String) -> Element {
    rsx! {
        div { class: "flex items-center justify-between py-2",
            span { class: "text-sm font-medium text-muted-foreground", "{label}" }
            span { class: "text-sm max-w-[60%] text-right break-all", "{value}" }
        }
    }
}
