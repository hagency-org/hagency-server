//! A user the homeserver has seen, local or remote, and the rooms it has seen
//! them in.

use dioxus::prelude::*;

use crate::api::known_users;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::{BreadcrumbItem, Breadcrumbs};
use crate::components::ui::relative_time::RelativeTime;
use crate::components::ui::table::*;
use crate::router::Route;
use crate::utils::i18n::t;

fn membership_badge(membership: &str) -> BadgeVariant {
    match membership {
        "join" => BadgeVariant::Success,
        "invite" | "knock" => BadgeVariant::Secondary,
        "ban" => BadgeVariant::Destructive,
        _ => BadgeVariant::Outline,
    }
}

#[component]
pub fn KnownUserShow(user_id: String) -> Element {
    let decoded_user_id = urlencoding::decode(&user_id)
        .map(|s| s.into_owned())
        .unwrap_or(user_id.clone());

    let decoded_for_resource = decoded_user_id.clone();
    let mut user_data = use_resource(move || {
        let id = decoded_for_resource.clone();
        async move { known_users::get_known_user(&id).await }
    });

    rsx! {
        div { class: "space-y-6",
            Breadcrumbs {
                items: vec![
                    BreadcrumbItem { label: t("users.title"), href: Some("/admin/users".to_string()) },
                    BreadcrumbItem { label: t("users.tab_known"), href: Some("/admin/users/known".to_string()) },
                    BreadcrumbItem { label: decoded_user_id.clone(), href: None },
                ],
            }

            match &*user_data.read() {
                Some(Ok(user)) => {
                    let title = user.displayname.clone().unwrap_or_else(|| user.user_id.clone());
                    let encoded_id = urlencoding::encode(&user.user_id).to_string();
                    rsx! {
                        div { class: "flex items-start justify-between gap-4",
                            div { class: "space-y-1",
                                h1 { class: "text-2xl font-bold tracking-tight", "{title}" }
                                p { class: "text-sm text-muted-foreground", "{user.user_id}" }
                                div { class: "flex items-center gap-2",
                                    if user.is_local {
                                        Badge { variant: BadgeVariant::Default, {t("users.origin_local")} }
                                    } else {
                                        Badge { variant: BadgeVariant::Secondary, {t("users.origin_remote")} }
                                    }
                                    span { class: "text-sm text-muted-foreground", "{user.server_name}" }
                                }
                            }
                            if user.has_account {
                                Link {
                                    to: Route::UserShow { user_id: encoded_id },
                                    class: "text-sm font-medium text-primary hover:underline",
                                    {t("users.open_account")}
                                }
                            }
                        }

                        if !user.is_local {
                            div { class: "rounded-md border bg-muted/50 p-4",
                                p { class: "text-sm text-muted-foreground", {t("users.remote_note")} }
                            }
                        }

                        Card {
                            CardHeader {
                                CardTitle { {t("users.seen_in_rooms")} }
                                CardDescription { {t("users.seen_in_rooms_desc")} }
                            }
                            CardContent {
                                div { class: "rounded-md border",
                                  div { class: "overflow-x-auto -mx-4 sm:mx-0",
                                    Table {
                                        TableHeader {
                                            TableRow {
                                                TableHead { {t("users.room")} }
                                                TableHead { {t("users.membership")} }
                                                TableHead { {t("users.membership_sender")} }
                                                TableHead { {t("users.updated")} }
                                            }
                                        }
                                        TableBody {
                                            for room in user.rooms.iter() {
                                                {
                                                    let room_id = room.room_id.clone();
                                                    let label = room.name.clone().unwrap_or_else(|| room_id.clone());
                                                    let membership = room.membership.clone();
                                                    let variant = membership_badge(&membership);
                                                    let sender = room.sender.clone();
                                                    let ts = room.updated_ts;
                                                    rsx! {
                                                        TableRow {
                                                            key: "{room_id}",
                                                            TableCell {
                                                                Link {
                                                                    to: Route::RoomShow { room_id: urlencoding::encode(&room_id).to_string() },
                                                                    class: "font-medium text-primary hover:underline",
                                                                    "{label}"
                                                                }
                                                                if room.name.is_some() {
                                                                    p { class: "text-xs text-muted-foreground", "{room_id}" }
                                                                }
                                                            }
                                                            TableCell {
                                                                Badge { variant, "{membership}" }
                                                            }
                                                            TableCell { class: "text-sm".to_string(), "{sender}" }
                                                            TableCell { class: "text-muted-foreground".to_string(),
                                                                RelativeTime { ts_ms: ts }
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
                Some(Err(e)) => rsx! {
                    div { class: "rounded-md bg-destructive/10 p-4",
                        div { class: "flex items-center justify-between",
                            p { class: "text-sm text-destructive", "Error: {e.message}" }
                            button {
                                class: "text-sm font-medium text-primary hover:underline",
                                onclick: move |_| user_data.restart(),
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
