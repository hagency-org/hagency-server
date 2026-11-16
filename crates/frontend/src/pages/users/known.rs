//! Every user the homeserver has seen in its rooms, local or remote.

use dioxus::dioxus_core::Task;
use dioxus::prelude::*;

use crate::api::known_users::{self, Origin};
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::empty_state::EmptyState;
use crate::components::ui::input::SearchInput;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::pagination::Pagination;
use crate::components::ui::relative_time::RelativeTime;
use crate::components::ui::table::*;
use crate::pages::users::tabs::{UsersTab, UsersTabs};
use crate::router::Route;
use crate::utils::i18n::t;

const PAGE_SIZE_OPTIONS: &[u64] = &[10, 25, 50, 100];
const DEFAULT_PAGE_SIZE: u64 = 25;

#[component]
pub fn KnownUserList() -> Element {
    let mut search_input = use_signal(String::new);
    let mut search = use_signal(String::new);
    let mut debounce_task = use_signal(|| Option::<Task>::None);
    let mut server_filter = use_signal(String::new);
    let mut origin = use_signal(|| Origin::All);
    let mut page = use_signal(|| 1u64);
    let mut per_page = use_signal(|| DEFAULT_PAGE_SIZE);
    let mut sort_by = use_signal(|| "user_id".to_string());
    let mut sort_desc = use_signal(|| false);

    let page_val = *page.read();
    let per_page_val = *per_page.read();

    let mut users_data = use_resource(move || {
        let search = search.read().clone();
        let server = server_filter.read().clone();
        let origin = *origin.read();
        let order_by = sort_by.read().clone();
        let desc = *sort_desc.read();
        let page = *page.read();
        let per_page = *per_page.read();
        async move {
            known_users::get_known_users(page, per_page, &search, &server, origin, &order_by, desc)
                .await
        }
    });

    // Clicking a sortable header sorts by it, or flips the direction if it is
    // already the sort column. Counts and times start with the largest first.
    let mut sort_on = move |column: &str| {
        if *sort_by.read() == column {
            let desc = *sort_desc.read();
            sort_desc.set(!desc);
        } else {
            sort_by.set(column.to_string());
            sort_desc.set(column != "user_id" && column != "server_name");
        }
        page.set(1);
    };
    let indicator = move |column: &str| -> &'static str {
        if *sort_by.read() != column {
            " \u{2195}"
        } else if *sort_desc.read() {
            " \u{2193}"
        } else {
            " \u{2191}"
        }
    };

    let current_server = server_filter.read().clone();
    let current_origin = *origin.read();

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("users.title"),
                description: t("users.known_subtitle"),
            }
            UsersTabs { active: UsersTab::Known }

            div { class: "flex items-center gap-4",
                div { class: "flex-1",
                    SearchInput {
                        placeholder: t("users.search"),
                        value: search_input.read().clone(),
                        oninput: move |evt: FormEvent| {
                            let value = evt.value();
                            search_input.set(value.clone());
                            if let Some(task) = debounce_task.write().take() {
                                task.cancel();
                            }
                            let task = spawn(async move {
                                gloo_timers::future::TimeoutFuture::new(300).await;
                                search.set(value);
                                page.set(1);
                            });
                            debounce_task.set(Some(task));
                        },
                    }
                }
                crate::components::ui::input::Select {
                    aria_label: "User origin",
                    class: "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm",
                    onchange: move |evt: FormEvent| {
                        origin.set(match evt.value().as_str() {
                            "local" => Origin::Local,
                            "remote" => Origin::Remote,
                            _ => Origin::All,
                        });
                        page.set(1);
                    },
                    option { value: "all", selected: current_origin == Origin::All, {t("users.origin_all")} }
                    option { value: "local", selected: current_origin == Origin::Local, {t("users.origin_local")} }
                    option { value: "remote", selected: current_origin == Origin::Remote, {t("users.origin_remote")} }
                }
                crate::components::ui::input::Field { class: "flex items-center gap-2",
                    crate::components::ui::input::Label { class: "text-sm text-muted-foreground whitespace-nowrap", {t("common.rows")} }
                    crate::components::ui::input::Select {
                        class: "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm",
                        value: "{per_page_val}",
                        onchange: move |evt: FormEvent| {
                            if let Ok(val) = evt.value().parse::<u64>() {
                                per_page.set(val);
                                page.set(1);
                            }
                        },
                        for size in PAGE_SIZE_OPTIONS.iter() {
                            option { value: "{size}", selected: *size == per_page_val, "{size}" }
                        }
                    }
                }
            }

            if !current_server.is_empty() {
                div { class: "flex items-center gap-3 rounded-md border bg-muted/50 px-4 py-2",
                    span { class: "text-sm", {t("users.server")} ": " span { class: "font-medium", "{current_server}" } }
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        onclick: move |_| {
                            server_filter.set(String::new());
                            page.set(1);
                        },
                        {t("common.clear_selection")}
                    }
                }
            }

            match &*users_data.read() {
                Some(Ok(data)) => rsx! {
                    div { class: "rounded-md border",
                      div { class: "overflow-x-auto max-h-[600px] overflow-y-auto -mx-4 sm:mx-0",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead {
                                        button {
                                            class: "flex items-center gap-1 hover:text-foreground",
                                            onclick: move |_| sort_on("user_id"),
                                            {t("users.user_id")}
                                            {indicator("user_id")}
                                        }
                                    }
                                    TableHead { {t("users.display_name")} }
                                    TableHead {
                                        button {
                                            class: "flex items-center gap-1 hover:text-foreground",
                                            onclick: move |_| sort_on("server_name"),
                                            {t("users.server")}
                                            {indicator("server_name")}
                                        }
                                    }
                                    TableHead {
                                        button {
                                            class: "flex items-center gap-1 hover:text-foreground",
                                            onclick: move |_| sort_on("joined_rooms"),
                                            {t("users.joined_rooms")}
                                            {indicator("joined_rooms")}
                                        }
                                    }
                                    TableHead {
                                        button {
                                            class: "flex items-center gap-1 hover:text-foreground",
                                            onclick: move |_| sort_on("total_rooms"),
                                            {t("users.all_rooms")}
                                            {indicator("total_rooms")}
                                        }
                                    }
                                    TableHead {
                                        button {
                                            class: "flex items-center gap-1 hover:text-foreground",
                                            onclick: move |_| sort_on("last_membership_at"),
                                            {t("users.last_membership_change")}
                                            {indicator("last_membership_at")}
                                        }
                                    }
                                }
                            }
                            TableBody {
                                if data.data.is_empty() {
                                    TableRow {
                                        TableCell { class: "p-0".to_string(), colspan: 99,
                                            EmptyState {
                                                icon: "users".to_string(),
                                                title: t("users.no_users"),
                                                description: t("users.known_empty"),
                                            }
                                        }
                                    }
                                } else {
                                    for user in data.data.iter() {
                                        {
                                            let user_id = user.user_id.clone();
                                            let server = user.server_name.clone();
                                            let server_for_filter = server.clone();
                                            let display_name = user.displayname.clone().unwrap_or_else(|| "-".to_string());
                                            let is_local = user.is_local;
                                            let joined = user.joined_rooms;
                                            let total = user.total_rooms;
                                            let banned = user.banned_rooms;
                                            let last_ts = user.last_membership_ts;
                                            rsx! {
                                                TableRow {
                                                    key: "{user_id}",
                                                    TableCell {
                                                        Link {
                                                            to: Route::KnownUserShow { user_id: urlencoding::encode(&user_id).to_string() },
                                                            class: "font-medium text-primary hover:underline",
                                                            "{user_id}"
                                                        }
                                                    }
                                                    TableCell { "{display_name}" }
                                                    TableCell {
                                                        div { class: "flex items-center gap-2",
                                                            if is_local {
                                                                Badge { variant: BadgeVariant::Default, {t("users.origin_local")} }
                                                            } else {
                                                                Badge { variant: BadgeVariant::Secondary, {t("users.origin_remote")} }
                                                            }
                                                            button {
                                                                class: "text-sm hover:underline",
                                                                title: t("users.filter_by_server"),
                                                                onclick: move |_| {
                                                                    server_filter.set(server_for_filter.clone());
                                                                    page.set(1);
                                                                },
                                                                "{server}"
                                                            }
                                                        }
                                                    }
                                                    TableCell { "{joined}" }
                                                    TableCell {
                                                        div { class: "flex items-center gap-2",
                                                            "{total}"
                                                            if banned > 0 {
                                                                Badge { variant: BadgeVariant::Destructive, "{banned} " {t("users.banned")} }
                                                            }
                                                        }
                                                    }
                                                    TableCell { class: "text-muted-foreground".to_string(),
                                                        RelativeTime { ts_ms: last_ts }
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
                        total: data.total,
                        per_page: per_page_val,
                        on_page_change: move |p| page.set(p),
                    }
                },
                Some(Err(e)) => rsx! {
                    div { class: "rounded-md bg-destructive/10 p-4",
                        div { class: "flex items-center justify-between",
                            p { class: "text-sm text-destructive", "Error: {e.message}" }
                            button {
                                class: "text-sm font-medium text-primary hover:underline",
                                onclick: move |_| users_data.restart(),
                                {t("common.retry")}
                            }
                        }
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}
