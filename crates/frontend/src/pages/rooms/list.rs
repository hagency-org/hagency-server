use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use std::collections::HashSet;

use crate::api::rooms;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::components::ui::empty_state::EmptyState;
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label, SearchInput};
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::pagination::Pagination;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::router::Route;
use crate::utils::i18n::t;

const PAGE_SIZE_OPTIONS: &[u64] = &[10, 25, 50, 100];
const DEFAULT_PAGE_SIZE: u64 = 25;

#[derive(Debug, Clone, PartialEq)]
enum SortOption {
    Name,
    Members,
    StateEvents,
}

impl SortOption {
    fn order_by(&self) -> &'static str {
        match self {
            SortOption::Name => "name",
            SortOption::Members => "joined_members",
            SortOption::StateEvents => "state_events",
        }
    }

    fn default_dir(&self) -> &'static str {
        match self {
            SortOption::Name => "asc",
            SortOption::Members => "desc",
            SortOption::StateEvents => "desc",
        }
    }

    fn from_value(v: &str) -> Self {
        match v {
            "members" => SortOption::Members,
            "state_events" => SortOption::StateEvents,
            _ => SortOption::Name,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum TypeFilter {
    All,
    Public,
    Private,
    Encrypted,
}

impl TypeFilter {
    fn label(&self) -> String {
        match self {
            TypeFilter::All => t("rooms.all"),
            TypeFilter::Public => t("rooms.public"),
            TypeFilter::Private => t("rooms.private"),
            TypeFilter::Encrypted => t("rooms.encrypted"),
        }
    }
}

fn session_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.session_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
}

fn session_set(key: &str, value: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
        let _ = storage.set_item(key, value);
    }
}

#[component]
pub fn RoomList() -> Element {
    let initial_page = session_get("rooms_page")
        .and_then(|p| p.parse::<u64>().ok())
        .unwrap_or(1);
    let initial_search = session_get("rooms_search").unwrap_or_default();

    let initial_search2 = initial_search.clone();
    let mut search_input = use_signal(move || initial_search.clone());
    let mut search = use_signal(move || initial_search2.clone());
    let mut page = use_signal(move || initial_page);
    let mut per_page = use_signal(|| DEFAULT_PAGE_SIZE);
    let mut debounce_task = use_signal(|| Option::<Task>::None);
    let mut sort_option = use_signal(|| SortOption::Name);
    let mut type_filter = use_signal(|| TypeFilter::All);

    // Bulk selection state
    let mut selected_rooms = use_signal(HashSet::<String>::new);
    let mut bulk_running = use_signal(|| false);

    // Create room dialog state
    let mut show_create_dialog = use_signal(|| false);
    let mut create_room_name = use_signal(String::new);
    let mut create_room_topic = use_signal(String::new);
    let mut create_room_public = use_signal(|| true);
    let mut creating_room = use_signal(|| false);

    // Persist page and search to sessionStorage
    {
        let page_sig = page;
        use_effect(move || {
            let p = *page_sig.read();
            session_set("rooms_page", &p.to_string());
        });
    }
    {
        let search_sig = search;
        use_effect(move || {
            let s = search_sig.read().clone();
            session_set("rooms_search", &s);
        });
    }

    let search_val = search.read().clone();
    let page_val = *page.read();
    let per_page_val = *per_page.read();
    let current_sort = sort_option.read().clone();

    let order_by = current_sort.order_by();
    let dir = current_sort.default_dir();

    let mut rooms_data = use_resource(move || {
        let search = search_val.clone();
        async move { rooms::get_rooms_cached(page_val, per_page_val, order_by, dir, &search).await }
    });

    let is_creating = *creating_room.read();
    let dialog_open = *show_create_dialog.read();

    let handle_create_room = move |_: MouseEvent| {
        let name = create_room_name.read().clone();
        let topic = create_room_topic.read().clone();
        let public = *create_room_public.read();

        if name.is_empty() {
            show_toast("Room name is required", ToastVariant::Error);
            return;
        }

        creating_room.set(true);
        spawn(async move {
            match rooms::create_room(&name, &topic, public).await {
                Ok(room_id) => {
                    show_toast(&format!("Room created: {room_id}"), ToastVariant::Success);
                    show_create_dialog.set(false);
                    create_room_name.set(String::new());
                    create_room_topic.set(String::new());
                    create_room_public.set(true);
                    rooms_data.restart();
                }
                Err(e) => {
                    show_toast(
                        &format!("Failed to create room: {}", e.message),
                        ToastVariant::Error,
                    );
                }
            }
            creating_room.set(false);
        });
    };

    let is_bulk_running = *bulk_running.read();
    let selected_count = selected_rooms.read().len();

    let handle_bulk_delete = move |_: MouseEvent| {
        if *bulk_running.read() {
            return;
        }
        let ids: Vec<String> = selected_rooms.read().iter().cloned().collect();
        if ids.is_empty() {
            return;
        }
        bulk_running.set(true);
        spawn(async move {
            let total = ids.len();
            let mut success_count = 0usize;
            let mut fail_count = 0usize;
            for rid in &ids {
                match rooms::delete_room(rid, false).await {
                    Ok(_) => success_count += 1,
                    Err(_) => fail_count += 1,
                }
            }
            if fail_count == 0 {
                show_toast(
                    &format!("Deleted {} rooms", success_count),
                    ToastVariant::Success,
                );
            } else {
                show_toast(
                    &format!("Deleted {success_count} of {total} rooms ({fail_count} failed)"),
                    ToastVariant::Error,
                );
            }
            selected_rooms.set(HashSet::new());
            bulk_running.set(false);
            rooms_data.restart();
        });
    };

    let handle_bulk_block = move |_: MouseEvent| {
        if *bulk_running.read() {
            return;
        }
        let ids: Vec<String> = selected_rooms.read().iter().cloned().collect();
        if ids.is_empty() {
            return;
        }
        bulk_running.set(true);
        spawn(async move {
            let total = ids.len();
            let mut success_count = 0usize;
            let mut fail_count = 0usize;
            for rid in &ids {
                match rooms::block_room(rid, true).await {
                    Ok(_) => success_count += 1,
                    Err(_) => fail_count += 1,
                }
            }
            if fail_count == 0 {
                show_toast(
                    &format!("Blocked {} rooms", success_count),
                    ToastVariant::Success,
                );
            } else {
                show_toast(
                    &format!("Blocked {success_count} of {total} rooms ({fail_count} failed)"),
                    ToastVariant::Error,
                );
            }
            selected_rooms.set(HashSet::new());
            bulk_running.set(false);
            rooms_data.restart();
        });
    };

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("rooms.title"),
                description: t("rooms.subtitle"),
                Button {
                    onclick: move |_| show_create_dialog.set(true),
                    Icon { name: "plus".to_string(), class: "h-4 w-4 mr-2".to_string() }
                    {t("rooms.create")}
                }
            }

            div { class: "flex items-center gap-4",
                div { class: "flex-1",
                    SearchInput {
                        placeholder: t("rooms.search_placeholder"),
                        value: search_input.read().clone(),
                        oninput: move |evt: FormEvent| {
                            let value = evt.value();
                            search_input.set(value.clone());
                            // Cancel previous debounce timer
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
                div { class: "flex items-center gap-2",
                    label { class: "text-sm text-muted-foreground whitespace-nowrap", {t("rooms.sort")} }
                    select {
                        class: "flex h-10 rounded-md border border-input bg-background px-3 py-2 text-sm",
                        value: current_sort.order_by(),
                        onchange: move |evt: FormEvent| {
                            sort_option.set(SortOption::from_value(&evt.value()));
                            page.set(1);
                        },
                        option { value: "name", selected: current_sort == SortOption::Name, {t("rooms.name")} }
                        option { value: "members", selected: current_sort == SortOption::Members, {t("rooms.members")} }
                        option { value: "state_events", selected: current_sort == SortOption::StateEvents, {t("rooms.created")} }
                    }
                }
                div { class: "flex items-center gap-2",
                    label { class: "text-sm text-muted-foreground whitespace-nowrap", {t("common.rows")} }
                    select {
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

            // Type filter chips
            {
                let current_filter = type_filter.read().clone();
                let filters = [TypeFilter::All, TypeFilter::Public, TypeFilter::Private, TypeFilter::Encrypted];
                rsx! {
                    div { class: "flex items-center gap-2",
                        for filter in filters.iter() {
                            {
                                let is_active = current_filter == *filter;
                                let filter_val = filter.clone();
                                let label = filter.label();
                                rsx! {
                                    button {
                                        key: "{label}",
                                        class: if is_active {
                                            "inline-flex items-center rounded-full px-3 py-1 text-xs font-medium bg-primary text-primary-foreground"
                                        } else {
                                            "inline-flex items-center rounded-full px-3 py-1 text-xs font-medium border border-input bg-background text-foreground hover:bg-accent hover:text-accent-foreground"
                                        },
                                        onclick: move |_| type_filter.set(filter_val.clone()),
                                        "{label}"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Bulk action bar
            if selected_count > 0 {
                div { class: "flex items-center gap-3 rounded-md border bg-muted/50 px-4 py-2",
                    span { class: "text-sm font-medium", "{selected_count} {t(\"common.selected\")}" }
                    Button {
                        variant: ButtonVariant::Destructive,
                        size: ButtonSize::Sm,
                        disabled: is_bulk_running,
                        onclick: handle_bulk_delete,
                        {t("rooms.delete_selected")}
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        size: ButtonSize::Sm,
                        disabled: is_bulk_running,
                        onclick: handle_bulk_block,
                        {t("rooms.block_selected")}
                    }
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Sm,
                        disabled: is_bulk_running,
                        onclick: move |_| {
                            selected_rooms.set(HashSet::new());
                        },
                        {t("common.clear_selection")}
                    }
                }
            }

            match &*rooms_data.read() {
                Some(Ok(data)) => {
                    let active_filter = type_filter.read().clone();
                    let filtered_rooms: Vec<_> = data.data.iter().filter(|room| {
                        match active_filter {
                            TypeFilter::All => true,
                            TypeFilter::Public => room.room.public,
                            TypeFilter::Private => !room.room.public,
                            TypeFilter::Encrypted => room.is_encrypted,
                        }
                    }).collect();

                    let all_ids: Vec<String> = filtered_rooms.iter().map(|r| r.id.clone()).collect();
                    let current_selected = selected_rooms.read().clone();
                    let all_selected = !all_ids.is_empty() && all_ids.iter().all(|id| current_selected.contains(id));

                    rsx! {
                    div { class: "rounded-md border",
                      div { class: "overflow-x-auto max-h-[600px] overflow-y-auto -mx-4 sm:mx-0",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { class: "w-10".to_string(),
                                        input {
                                            r#type: "checkbox",
                                            class: "h-4 w-4 rounded border-gray-300",
                                            checked: all_selected,
                                            onchange: {
                                                let all_ids = all_ids.clone();
                                                move |_| {
                                                    let mut set = selected_rooms.write();
                                                    let currently_all = all_ids.iter().all(|id| set.contains(id));
                                                    if currently_all {
                                                        for id in &all_ids {
                                                            set.remove(id);
                                                        }
                                                    } else {
                                                        for id in &all_ids {
                                                            set.insert(id.clone());
                                                        }
                                                    }
                                                }
                                            },
                                        }
                                    }
                                    TableHead { {t("rooms.name")} }
                                    TableHead { {t("rooms.alias")} }
                                    TableHead { {t("rooms.members")} }
                                    TableHead { {t("rooms.visibility")} }
                                    TableHead { {t("rooms.join_rules")} }
                                }
                            }
                            TableBody {
                                if filtered_rooms.is_empty() {
                                    TableRow {
                                        TableCell { class: "p-0".to_string(), colspan: 99,
                                            EmptyState {
                                                icon: "message-square".to_string(),
                                                title: t("rooms.no_rooms"),
                                                description: t("rooms.no_rooms_description"),
                                            }
                                        }
                                    }
                                } else {
                                    for room in filtered_rooms.iter() {
                                        {
                                            let room_id = room.id.clone();
                                            let room_id_for_checkbox = room.id.clone();
                                            let name = room.room.name.clone().unwrap_or_else(|| room_id.clone());
                                            let alias = room.alias.clone().unwrap_or_else(|| "-".to_string());
                                            let members = room.members;
                                            let is_public = room.room.public;
                                            let join_rules = room.room.join_rules.clone().unwrap_or_else(|| "-".to_string());
                                            let is_checked = selected_rooms.read().contains(&room_id);

                                            rsx! {
                                                TableRow {
                                                    key: "{room_id}",
                                                    TableCell { class: "w-10".to_string(),
                                                        input {
                                                            r#type: "checkbox",
                                                            class: "h-4 w-4 rounded border-gray-300",
                                                            checked: is_checked,
                                                            onchange: move |_| {
                                                                let mut set = selected_rooms.write();
                                                                if set.contains(&room_id_for_checkbox) {
                                                                    set.remove(&room_id_for_checkbox);
                                                                } else {
                                                                    set.insert(room_id_for_checkbox.clone());
                                                                }
                                                            },
                                                        }
                                                    }
                                                    TableCell {
                                                        Link {
                                                            to: Route::RoomShow { room_id: urlencoding::encode(&room_id).to_string() },
                                                            class: "font-medium text-primary hover:underline",
                                                            "{name}"
                                                        }
                                                    }
                                                    TableCell { class: "text-muted-foreground".to_string(), "{alias}" }
                                                    TableCell {
                                                        div { class: "flex items-center gap-1",
                                                            Icon { name: "users".to_string(), class: "h-3 w-3 text-muted-foreground".to_string() }
                                                            "{members}"
                                                        }
                                                    }
                                                    TableCell {
                                                        if is_public {
                                                            Badge {
                                                                variant: BadgeVariant::Secondary,
                                                                Icon { name: "globe".to_string(), class: "h-3 w-3 mr-1".to_string() }
                                                                {t("rooms.public")}
                                                            }
                                                        } else {
                                                            Badge {
                                                                variant: BadgeVariant::Outline,
                                                                Icon { name: "lock".to_string(), class: "h-3 w-3 mr-1".to_string() }
                                                                {t("rooms.private")}
                                                            }
                                                        }
                                                    }
                                                    TableCell { class: "text-muted-foreground".to_string(), "{join_rules}" }
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
                }},
                Some(Err(e)) => rsx! {
                    div { class: "rounded-md bg-destructive/10 p-4",
                        div { class: "flex items-center justify-between",
                            p { class: "text-sm text-destructive", "Error: {e.message}" }
                            button {
                                class: "text-sm font-medium text-primary hover:underline",
                                onclick: move |_| rooms_data.restart(),
                                {t("common.retry")}
                            }
                        }
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        // Create Room Dialog
        Modal {
            open: dialog_open,
            on_close: move |_| {
                if !is_creating {
                    show_create_dialog.set(false);
                }
            },
                    div { class: "flex flex-col space-y-2 text-center sm:text-left",
                        h2 { class: "text-lg font-semibold", {t("rooms.create")} }
                        p { class: "text-sm text-muted-foreground", {t("rooms.create_description")} }
                    }
                    div { class: "space-y-4 mt-4",
                        div { class: "space-y-2",
                            Label { r#for: "room_name".to_string(), {t("rooms.room_name")} }
                            Input {
                                placeholder: "My Room".to_string(),
                                value: create_room_name.read().clone(),
                                oninput: move |evt: FormEvent| create_room_name.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { r#for: "room_topic".to_string(), {t("rooms.topic")} }
                            Input {
                                placeholder: t("rooms.topic_placeholder"),
                                value: create_room_topic.read().clone(),
                                oninput: move |evt: FormEvent| create_room_topic.set(evt.value()),
                                disabled: is_creating,
                            }
                        }
                        div { class: "space-y-2",
                            Label { {t("rooms.visibility")} }
                            div { class: "flex items-center gap-4",
                                label { class: "flex items-center gap-2 text-sm cursor-pointer",
                                    input {
                                        r#type: "radio",
                                        name: "visibility",
                                        value: "public",
                                        checked: *create_room_public.read(),
                                        onchange: move |_| create_room_public.set(true),
                                        disabled: is_creating,
                                    }
                                    {t("rooms.public")}
                                }
                                label { class: "flex items-center gap-2 text-sm cursor-pointer",
                                    input {
                                        r#type: "radio",
                                        name: "visibility",
                                        value: "private",
                                        checked: !*create_room_public.read(),
                                        onchange: move |_| create_room_public.set(false),
                                        disabled: is_creating,
                                    }
                                    {t("rooms.private")}
                                }
                            }
                        }
                    }
                    div { class: "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2 mt-6",
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: is_creating,
                            onclick: move |_| show_create_dialog.set(false),
                            {t("common.cancel")}
                        }
                        Button {
                            disabled: is_creating,
                            onclick: handle_create_room,
                            if is_creating {
                                Spinner { class: "mr-2".to_string() }
                            }
                            {t("rooms.create_button")}
                        }
                    }
        }
    }
}
