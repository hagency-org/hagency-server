use dioxus::prelude::*;

use crate::api::media;
use crate::components::media_ops::{DeleteMediaDialog, PurgeRemoteMediaDialog};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::SearchInput;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::pagination::Pagination;
use crate::components::ui::table::*;
use crate::router::Route;
use crate::utils::date::format_bytes;
use crate::utils::i18n::t;

const PAGE_SIZE: u64 = 25;

#[component]
pub fn MediaList() -> Element {
    let nav = use_navigator();
    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 1u64);
    let mut show_delete_dialog = use_signal(|| false);
    let mut show_purge_dialog = use_signal(|| false);

    let search_val = search.read().clone();
    let page_val = *page.read();

    let mut media_data = use_resource(move || {
        let search = search_val.clone();
        async move {
            media::get_user_media_statistics_cached(
                page_val,
                PAGE_SIZE,
                "media_length",
                "desc",
                &search,
            )
            .await
        }
    });

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("media.title"),
                description: t("media.subtitle"),
                div { class: "flex gap-2",
                    Button {
                        variant: ButtonVariant::Outline,
                        onclick: move |_| show_delete_dialog.set(true),
                        {t("media.delete_local")}
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        onclick: move |_| show_purge_dialog.set(true),
                        {t("media.purge_remote")}
                    }
                }
            }

            SearchInput {
                placeholder: t("media.search"),
                value: search.read().clone(),
                oninput: move |evt: FormEvent| {
                    search.set(evt.value());
                    page.set(1);
                },
            }

            match &*media_data.read() {
                Some(Ok(data)) => rsx! {
                    div { class: "rounded-md border",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { {t("media.user_id")} }
                                    TableHead { {t("media.display_name")} }
                                    TableHead { {t("media.media_count")} }
                                    TableHead { {t("media.total_size")} }
                                    TableHead { class: "text-right".to_string(), {t("media.actions")} }
                                }
                            }
                            TableBody {
                                if data.data.is_empty() {
                                    TableRow {
                                        TableCell { class: "text-center text-muted-foreground py-8".to_string(), colspan: 99,
                                            {t("media.no_media")}
                                        }
                                    }
                                } else {
                                    for stat in data.data.iter() {
                                        {
                                            let user_id = stat.statistic.user_id.clone();
                                            let user_id_for_action = user_id.clone();
                                            let display_name = stat.statistic.displayname.clone().unwrap_or_else(|| "-".to_string());
                                            let media_count = stat.statistic.media_count;
                                            let media_length = format_bytes(stat.statistic.media_length);

                                            rsx! {
                                                TableRow {
                                                    TableCell { class: "font-medium".to_string(),
                                                        Link {
                                                            to: Route::UserShow { user_id: urlencoding::encode(&user_id).to_string() },
                                                            class: "text-primary hover:underline",
                                                            "{user_id}"
                                                        }
                                                    }
                                                    TableCell { "{display_name}" }
                                                    TableCell { "{media_count}" }
                                                    TableCell { "{media_length}" }
                                                    TableCell { class: "text-right".to_string(),
                                                        div { class: "flex items-center justify-end gap-1",
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: ButtonSize::Sm,
                                                                onclick: move |_| {
                                                                    nav.push(Route::UserShow {
                                                                        user_id: urlencoding::encode(&user_id_for_action).to_string(),
                                                                    });
                                                                },
                                                                Icon { name: "user".to_string(), class: "h-4 w-4".to_string() }
                                                                "User"
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
                        on_retry: move |_| media_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        DeleteMediaDialog {
            open: *show_delete_dialog.read(),
            on_close: move |_| show_delete_dialog.set(false),
            on_success: move |_| media_data.restart(),
        }
        PurgeRemoteMediaDialog {
            open: *show_purge_dialog.read(),
            on_close: move |_| show_purge_dialog.set(false),
            on_success: move |_| media_data.restart(),
        }
    }
}
