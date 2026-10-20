use dioxus::prelude::*;

use crate::api::destinations;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::SearchInput;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::{BreadcrumbItem, Breadcrumbs, PageHeader};
use crate::components::ui::pagination::Pagination;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::router::Route;
use crate::utils::date::{format_duration_ms, format_timestamp};
use crate::utils::i18n::t;

const PAGE_SIZE: u64 = 25;

#[component]
pub fn DestinationList() -> Element {
    let mut page = use_signal(|| 1u64);
    let mut search = use_signal(String::new);
    let page_val = *page.read();
    let search_val = search.read().clone();

    let mut dest_data = use_resource(move || {
        let _search = search_val.clone();
        async move { destinations::get_destinations(page_val, PAGE_SIZE, "destination", "asc").await }
    });

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("destinations.title"),
                description: t("destinations.subtitle"),
            }

            SearchInput {
                placeholder: t("destinations.search"),
                value: search.read().clone(),
                oninput: move |evt: FormEvent| {
                    search.set(evt.value());
                    page.set(1);
                },
            }

            match &*dest_data.read() {
                Some(Ok(data)) => {
                    let search_term = search.read().to_lowercase();
                    let filtered: Vec<_> = if search_term.is_empty() {
                        data.data.clone()
                    } else {
                        data.data.iter().filter(|d| d.destination.destination.to_lowercase().contains(&search_term)).cloned().collect()
                    };
                    let total = if search_term.is_empty() { data.total } else { filtered.len() as u64 };

                    rsx! {
                        div { class: "rounded-md border",
                            Table {
                                TableHeader {
                                    TableRow {
                                        TableHead { {t("destinations.destination")} }
                                        TableHead { {t("destinations.status")} }
                                        TableHead { {t("destinations.last_retry")} }
                                        TableHead { {t("destinations.retry_interval")} }
                                        TableHead { class: "text-right".to_string(), {t("destinations.actions")} }
                                    }
                                }
                                TableBody {
                                    if filtered.is_empty() {
                                        TableRow {
                                            TableCell { class: "text-center text-muted-foreground py-8".to_string(), colspan: 99,
                                                {t("destinations.no_destinations_found")}
                                            }
                                        }
                                    } else {
                                        for dest in filtered.iter() {
                                            {
                                                let name = dest.destination.destination.clone();
                                                let has_failure = dest.destination.failure_ts.is_some() && dest.destination.failure_ts.unwrap_or(0) > 0;
                                                let retry_last = if dest.destination.retry_last_ts > 0 {
                                                    format_timestamp(dest.destination.retry_last_ts)
                                                } else {
                                                    "-".to_string()
                                                };
                                                let retry_interval = dest.destination.retry_interval;
                                                let dest_id = name.clone();

                                                rsx! {
                                                    TableRow {
                                                        TableCell {
                                                            div { class: "flex items-center gap-2",
                                                                Icon { name: "globe".to_string(), class: "h-4 w-4 text-muted-foreground".to_string() }
                                                                Link {
                                                                    to: Route::DestinationShow { destination_id: urlencoding::encode(&name).to_string() },
                                                                    class: "font-medium text-primary hover:underline",
                                                                    "{name}"
                                                                }
                                                            }
                                                        }
                                                        TableCell {
                                                            if has_failure {
                                                                Badge { variant: BadgeVariant::Destructive, {t("destinations.failed")} }
                                                            } else {
                                                                Badge { variant: BadgeVariant::Success, {t("destinations.ok")} }
                                                            }
                                                        }
                                                        TableCell { class: "text-muted-foreground".to_string(), "{retry_last}" }
                                                        TableCell {
                                                            if retry_interval > 0 {
                                                                "{format_duration_ms(retry_interval)}"
                                                            } else {
                                                                "-"
                                                            }
                                                        }
                                                        TableCell { class: "text-right".to_string(),
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: ButtonSize::Sm,
                                                                onclick: move |_| {
                                                                    let id = dest_id.clone();
                                                                    spawn(async move {
                                                                        match destinations::reset_destination_connection(&id).await {
                                                                            Ok(_) => {
                                                                                show_toast("Connection reset successfully", ToastVariant::Success);
                                                                                dest_data.restart();
                                                                            }
                                                                            Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                        }
                                                                    });
                                                                },
                                                                {t("destinations.reset_button")}
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

                        Pagination {
                            page: page_val,
                            total,
                            per_page: PAGE_SIZE,
                            on_page_change: move |p| page.set(p),
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| dest_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}

#[component]
pub fn DestinationShow(destination_id: String) -> Element {
    let decoded_id = urlencoding::decode(&destination_id)
        .map(|s| s.into_owned())
        .unwrap_or(destination_id.clone());

    let decoded_for_resource = decoded_id.clone();
    let decoded_for_reset = decoded_id.clone();

    let mut dest_data = use_resource(move || {
        let id = decoded_for_resource.clone();
        async move { destinations::get_destination(&id).await }
    });

    rsx! {
        div { class: "space-y-6",
            Breadcrumbs {
                items: vec![
                    BreadcrumbItem { label: t("destinations.title"), href: Some("/admin/destinations".to_string()) },
                    BreadcrumbItem { label: decoded_id.clone(), href: None },
                ],
            }

            match &*dest_data.read() {
                Some(Ok(dest)) => {
                    let name = dest.destination.destination.clone();
                    let has_failure = dest.destination.failure_ts.is_some() && dest.destination.failure_ts.unwrap_or(0) > 0;
                    let retry_last = if dest.destination.retry_last_ts > 0 {
                        format_timestamp(dest.destination.retry_last_ts)
                    } else {
                        "-".to_string()
                    };
                    let retry_interval = dest.destination.retry_interval;
                    let failure_ts = dest.destination.failure_ts
                        .filter(|ts| *ts > 0)
                        .map(format_timestamp)
                        .unwrap_or_else(|| "-".to_string());
                    let last_stream = dest.destination.last_successful_stream_ordering
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let dest_id_for_reset = decoded_for_reset.clone();

                    rsx! {
                        PageHeader {
                            title: name.clone(),
                            description: t("destinations.detail_description"),
                            Button {
                                variant: ButtonVariant::Outline,
                                onclick: move |_| {
                                    let id = dest_id_for_reset.clone();
                                    spawn(async move {
                                        match destinations::reset_destination_connection(&id).await {
                                            Ok(_) => {
                                                show_toast("Connection reset successfully", ToastVariant::Success);
                                                dest_data.restart();
                                            }
                                            Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                        }
                                    });
                                },
                                Icon { name: "refresh-cw".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                {t("destinations.reset_button")}
                            }
                        }

                        div { class: "grid gap-6 md:grid-cols-2",
                            Card {
                                CardHeader { CardTitle { {t("destinations.connection_info")} } }
                                CardContent {
                                    div { class: "space-y-4",
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("destinations.status")} }
                                            if has_failure {
                                                Badge { variant: BadgeVariant::Destructive, {t("destinations.failed")} }
                                            } else {
                                                Badge { variant: BadgeVariant::Success, {t("destinations.ok")} }
                                            }
                                        }
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("destinations.destination")} }
                                            span { class: "text-sm font-mono", "{name}" }
                                        }
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("destinations.last_successful_stream")} }
                                            span { class: "text-sm", "{last_stream}" }
                                        }
                                    }
                                }
                            }

                            Card {
                                CardHeader { CardTitle { {t("destinations.retry_info")} } }
                                CardContent {
                                    div { class: "space-y-4",
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("destinations.last_failure")} }
                                            span { class: "text-sm", "{failure_ts}" }
                                        }
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("destinations.last_retry")} }
                                            span { class: "text-sm", "{retry_last}" }
                                        }
                                        div { class: "flex items-center justify-between py-2",
                                            span { class: "text-sm font-medium text-muted-foreground", {t("destinations.retry_interval")} }
                                            span { class: "text-sm",
                                                if retry_interval > 0 {
                                                    "{format_duration_ms(retry_interval)}"
                                                } else {
                                                    "-"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| dest_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}
