use dioxus::prelude::*;

use crate::api::rooms;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::dialog::{ConfirmDialog, Modal, ModalSize};
use crate::components::ui::icons::Icon;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::{LoadingSkeleton, PageSkeleton, Spinner};
use crate::components::ui::notifications::{
    NotificationSeverity, NotificationType, add_typed_notification,
};
use crate::components::ui::page_header::{BreadcrumbItem, Breadcrumbs, PageHeader};
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::router::Route;
use crate::utils::i18n::t;

#[derive(Debug, Clone, PartialEq)]
enum Tab {
    Overview,
    Members,
    Messages,
    State,
    Hierarchy,
    ForwardExtremities,
}

#[component]
pub fn RoomShow(room_id: String) -> Element {
    let nav = use_navigator();

    let decoded_room_id = urlencoding::decode(&room_id)
        .map(|s| s.into_owned())
        .unwrap_or(room_id.clone());

    let decoded_for_resource = decoded_room_id.clone();
    let decoded_for_state = decoded_room_id.clone();
    let mut room_data = use_resource(move || {
        let id = decoded_for_resource.clone();
        async move { rooms::get_room(&id).await }
    });

    let mut state_for_aliases = use_resource(move || {
        let id = decoded_for_state.clone();
        async move { rooms::get_room_state(&id).await }
    });

    let mut active_tab = use_signal(|| Tab::Overview);
    let mut show_delete_dialog = use_signal(|| false);
    let mut show_purge_dialog = use_signal(|| false);
    let mut purge_date = use_signal(|| String::new());
    let mut purge_loading = use_signal(|| false);
    let mut blocked_override = use_signal(|| Option::<bool>::None);
    let mut block_loading = use_signal(|| false);

    // Room edit state
    let mut editing_room = use_signal(|| false);
    let mut edit_name = use_signal(|| String::new());
    let mut edit_topic = use_signal(|| String::new());
    let mut edit_join_rules = use_signal(|| String::new());
    let mut edit_history_vis = use_signal(|| String::new());
    let mut saving_room = use_signal(|| false);

    // Alias management
    let mut new_alias = use_signal(|| String::new());

    rsx! {
        div { class: "space-y-6",
            Breadcrumbs {
                items: vec![
                    BreadcrumbItem { label: t("rooms.title"), href: Some("/admin/rooms".to_string()) },
                    BreadcrumbItem { label: decoded_room_id.clone(), href: None },
                ],
            }

            match &*room_data.read() {
                Some(Ok(room)) => {
                    let name = room.room.name.clone().unwrap_or_else(|| room.id.clone());
                    let alias = room.alias.clone().unwrap_or_else(|| "-".to_string());
                    let topic = room.room.topic.clone().unwrap_or_else(|| "-".to_string());
                    let members = room.members;
                    let is_public = room.room.public;
                    let join_rules = room.room.join_rules.clone().unwrap_or_else(|| "-".to_string());
                    let history_visibility = room.room.history_visibility.clone().unwrap_or_else(|| "-".to_string());
                    let is_space = room.room.room_type.as_deref() == Some("m.space");
                    let room_id_str = room.id.clone();
                    let room_id_for_delete = room_id_str.clone();
                    let room_id_for_tabs = room_id_str.clone();
                    let room_id_for_block = room_id_str.clone();
                    let room_id_for_purge = room_id_str.clone();
                    let current_tab = active_tab.read().clone();
                    let blocked =
                        (*blocked_override.read()).unwrap_or(room.room.is_blocked_effective());
                    let is_block_loading = *block_loading.read();
                    let is_purge_loading = *purge_loading.read();

                    // Build tabs list dynamically
                    let mut tabs: Vec<(Tab, String)> = vec![
                        (Tab::Overview, t("rooms.overview")),
                        (Tab::Members, t("rooms.members")),
                        (Tab::Messages, t("rooms.messages")),
                        (Tab::State, t("rooms.state_events")),
                        (Tab::ForwardExtremities, t("rooms.forward_extremities")),
                    ];
                    if is_space {
                        tabs.push((Tab::Hierarchy, t("rooms.hierarchy")));
                    }

                    let name_for_edit = room.room.name.clone().unwrap_or_default();
                    let topic_for_edit = topic.clone();
                    let jr_for_edit = join_rules.clone();
                    let hv_for_edit = history_visibility.clone();

                    rsx! {
                        PageHeader {
                            title: name,
                            description: room_id_str.clone(),
                            Button {
                                variant: ButtonVariant::Outline,
                                onclick: {
                                    let n = name_for_edit.clone();
                                    let tp = topic_for_edit.clone();
                                    let jr = jr_for_edit.clone();
                                    let hv = hv_for_edit.clone();
                                    move |_| {
                                        if !*editing_room.read() {
                                            edit_name.set(n.clone());
                                            edit_topic.set(if tp == "-" { String::new() } else { tp.clone() });
                                            edit_join_rules.set(jr.clone());
                                            edit_history_vis.set(hv.clone());
                                        }
                                        let v = *editing_room.read();
                                        editing_room.set(!v);
                                    }
                                },
                                Icon { name: "edit".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                {t("rooms.edit_room")}
                            }
                            Button {
                                variant: ButtonVariant::Outline,
                                onclick: move |_| show_purge_dialog.set(true),
                                Icon { name: "clock".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                {t("rooms.purge_history")}
                            }
                            Button {
                                variant: if blocked { ButtonVariant::Secondary } else { ButtonVariant::Outline },
                                disabled: is_block_loading,
                                onclick: {
                                    let rid = room_id_for_block.clone();
                                    move |_| {
                                        let rid = rid.clone();
                                        let new_blocked = !blocked;
                                        block_loading.set(true);
                                        spawn(async move {
                                            match rooms::block_room(&rid, new_blocked).await {
                                                Ok(_) => {
                                                    blocked_override.set(Some(new_blocked));
                                                    let msg = if new_blocked { "Room blocked" } else { "Room unblocked" };
                                                    show_toast(msg, ToastVariant::Success);
                                                }
                                                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                            }
                                            block_loading.set(false);
                                        });
                                    }
                                },
                                Icon { name: "shield".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                if blocked { {t("rooms.unblock")} } else { {t("rooms.block")} }
                            }
                            Button {
                                variant: ButtonVariant::Destructive,
                                onclick: move |_| show_delete_dialog.set(true),
                                Icon { name: "trash".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                {t("rooms.delete")}
                            }
                        }

                        // Tab navigation
                        div { class: "flex border-b",
                            {tabs.iter().map(|(tab, label)| {
                                let is_active = current_tab == *tab;
                                let tab_val = tab.clone();
                                rsx! {
                                    button {
                                        key: "{label}",
                                        class: if is_active {
                                            "px-4 py-2 text-sm font-medium border-b-2 border-primary text-primary"
                                        } else {
                                            "px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground"
                                        },
                                        onclick: move |_| active_tab.set(tab_val.clone()),
                                        "{label}"
                                    }
                                }
                            })}
                        }

                        match current_tab {
                            Tab::Overview => {
                                // Extract aliases from state events
                                let mut alias_list: Vec<String> = Vec::new();
                                if let Some(Ok(events)) = &*state_for_aliases.read() {
                                    for event in events.iter() {
                                        if event.event_type == "m.room.canonical_alias" {
                                            // Extract alt_aliases from canonical alias event
                                            if let Some(alts) = event.content.get("alt_aliases") {
                                                if let Some(arr) = alts.as_array() {
                                                    for v in arr {
                                                        if let Some(s) = v.as_str() {
                                                            alias_list.push(s.to_string());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if event.event_type == "m.room.aliases" {
                                            if let Some(aliases) = event.content.get("aliases") {
                                                if let Some(arr) = aliases.as_array() {
                                                    for v in arr {
                                                        if let Some(s) = v.as_str() {
                                                            if !alias_list.contains(&s.to_string()) {
                                                                alias_list.push(s.to_string());
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                let canonical_alias_display = alias.clone();
                                let room_id_for_edit = room_id_str.clone();
                                let room_id_for_alias = room_id_str.clone();
                                let is_editing_room = *editing_room.read();
                                let is_saving_room = *saving_room.read();

                                rsx! {
                                // Edit Room Form
                                if is_editing_room {
                                    Card {
                                        CardHeader { CardTitle { {t("rooms.edit_room")} } }
                                        CardContent {
                                            div { class: "space-y-4 max-w-xl",
                                                div { class: "space-y-2",
                                                    Label { {t("rooms.name")} }
                                                    Input {
                                                        placeholder: t("rooms.name"),
                                                        value: edit_name.read().clone(),
                                                        oninput: move |evt: FormEvent| edit_name.set(evt.value()),
                                                        disabled: is_saving_room,
                                                    }
                                                }
                                                div { class: "space-y-2",
                                                    Label { {t("rooms.topic")} }
                                                    Input {
                                                        placeholder: t("rooms.topic"),
                                                        value: edit_topic.read().clone(),
                                                        oninput: move |evt: FormEvent| edit_topic.set(evt.value()),
                                                        disabled: is_saving_room,
                                                    }
                                                }
                                                div { class: "space-y-2",
                                                    Label { {t("rooms.join_rules")} }
                                                    select {
                                                        class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm",
                                                        value: edit_join_rules.read().clone(),
                                                        disabled: is_saving_room,
                                                        onchange: move |evt: FormEvent| edit_join_rules.set(evt.value()),
                                                        option { value: "public", "public" }
                                                        option { value: "invite", "invite" }
                                                        option { value: "knock", "knock" }
                                                        option { value: "restricted", "restricted" }
                                                    }
                                                }
                                                div { class: "space-y-2",
                                                    Label { {t("rooms.history_visibility")} }
                                                    select {
                                                        class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm",
                                                        value: edit_history_vis.read().clone(),
                                                        disabled: is_saving_room,
                                                        onchange: move |evt: FormEvent| edit_history_vis.set(evt.value()),
                                                        option { value: "shared", "shared" }
                                                        option { value: "invited", "invited" }
                                                        option { value: "joined", "joined" }
                                                        option { value: "world_readable", "world_readable" }
                                                    }
                                                }
                                            }
                                        }
                                        CardFooter { class: "flex justify-end gap-2".to_string(),
                                            Button {
                                                variant: ButtonVariant::Outline,
                                                disabled: is_saving_room,
                                                onclick: move |_| editing_room.set(false),
                                                {t("common.cancel")}
                                            }
                                            Button {
                                                disabled: is_saving_room,
                                                onclick: {
                                                    let rid = room_id_for_edit.clone();
                                                    move |_| {
                                                        let rid = rid.clone();
                                                        let name = edit_name.read().clone();
                                                        let topic = edit_topic.read().clone();
                                                        let jr = edit_join_rules.read().clone();
                                                        let hv = edit_history_vis.read().clone();
                                                        saving_room.set(true);
                                                        spawn(async move {
                                                            let mut errors = Vec::new();
                                                            if rooms::set_room_name(&rid, &name).await.is_err() {
                                                                errors.push("name");
                                                            }
                                                            if rooms::set_room_topic(&rid, &topic).await.is_err() {
                                                                errors.push("topic");
                                                            }
                                                            if rooms::set_room_join_rules(&rid, &jr).await.is_err() {
                                                                errors.push("join_rules");
                                                            }
                                                            if rooms::set_room_history_visibility(&rid, &hv).await.is_err() {
                                                                errors.push("history_visibility");
                                                            }
                                                            saving_room.set(false);
                                                            if errors.is_empty() {
                                                                show_toast("Room updated", ToastVariant::Success);
                                                                editing_room.set(false);
                                                                room_data.restart();
                                                            } else {
                                                                show_toast(&format!("Failed to update: {}", errors.join(", ")), ToastVariant::Error);
                                                            }
                                                        });
                                                    }
                                                },
                                                if is_saving_room {
                                                    Spinner { class: "mr-2".to_string() }
                                                }
                                                {t("rooms.save_changes")}
                                            }
                                        }
                                    }
                                }

                                div { class: "grid gap-6 md:grid-cols-2",
                                    Card {
                                        CardHeader { CardTitle { {t("rooms.room_information")} } }
                                        CardContent {
                                            div { class: "space-y-4",
                                                InfoRow { label: t("rooms.room_id"), value: room_id_str.clone() }
                                                InfoRow { label: t("rooms.canonical_alias"), value: alias }
                                                InfoRow { label: t("rooms.topic"), value: topic }
                                                if is_space {
                                                    div { class: "flex items-center justify-between py-2",
                                                        span { class: "text-sm font-medium text-muted-foreground", {t("rooms.room_type")} }
                                                        Badge { variant: BadgeVariant::Secondary, {t("rooms.space")} }
                                                    }
                                                }
                                                if room.is_encrypted {
                                                    div { class: "flex items-center justify-between py-2",
                                                        span { class: "text-sm font-medium text-muted-foreground", {t("rooms.encrypted")} }
                                                        Badge { variant: BadgeVariant::Success,
                                                            Icon { name: "lock".to_string(), class: "h-3 w-3 mr-1".to_string() }
                                                            "Yes"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    Card {
                                        CardHeader { CardTitle { {t("rooms.room_settings")} } }
                                        CardContent {
                                            div { class: "space-y-4",
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("rooms.members")} }
                                                    div { class: "flex items-center gap-1",
                                                        Icon { name: "users".to_string(), class: "h-4 w-4 text-muted-foreground".to_string() }
                                                        span { class: "text-sm font-medium", "{members}" }
                                                    }
                                                }
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("rooms.visibility")} }
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
                                                // Directory visibility toggle
                                                {
                                                    let rid_for_dir = room_id_str.clone();
                                                    rsx! {
                                                        RoomDirectoryToggle { room_id: rid_for_dir }
                                                    }
                                                }
                                                InfoRow { label: t("rooms.join_rules"), value: join_rules }
                                                InfoRow { label: t("rooms.history_visibility"), value: history_visibility }
                                                div { class: "flex items-center justify-between py-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("rooms.blocked")} }
                                                    if blocked {
                                                        Badge { variant: BadgeVariant::Destructive, "Yes" }
                                                    } else {
                                                        span { class: "text-sm", "No" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                // Aliases section
                                Card {
                                    CardHeader { CardTitle { {t("rooms.aliases")} } }
                                    CardContent {
                                        div { class: "space-y-4",
                                            div { class: "flex items-center justify-between py-2",
                                                span { class: "text-sm font-medium text-muted-foreground", {t("rooms.canonical_alias")} }
                                                span { class: "text-sm font-mono max-w-[70%] text-right break-all", "{canonical_alias_display}" }
                                            }
                                            if !alias_list.is_empty() {
                                                div { class: "space-y-2",
                                                    span { class: "text-sm font-medium text-muted-foreground", {t("rooms.other_aliases")} }
                                                    div { class: "space-y-1",
                                                        for a in alias_list.iter() {
                                                            {
                                                                let alias_for_delete = a.clone();
                                                                rsx! {
                                                                    div {
                                                                        key: "{a}",
                                                                        class: "flex items-center justify-between py-1",
                                                                        div { class: "flex items-center",
                                                                            Icon { name: "hash".to_string(), class: "h-3 w-3 text-muted-foreground mr-2".to_string() }
                                                                            span { class: "text-sm font-mono", "{a}" }
                                                                        }
                                                                        Button {
                                                                            variant: ButtonVariant::Ghost,
                                                                            size: crate::components::ui::button::ButtonSize::Sm,
                                                                            onclick: move |_| {
                                                                                let alias = alias_for_delete.clone();
                                                                                spawn(async move {
                                                                                    match rooms::delete_room_alias(&alias).await {
                                                                                        Ok(_) => {
                                                                                            show_toast("Alias removed", ToastVariant::Success);
                                                                                            state_for_aliases.restart();
                                                                                        }
                                                                                        Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                                    }
                                                                                });
                                                                            },
                                                                            Icon { name: "trash".to_string(), class: "h-3 w-3 text-destructive".to_string() }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                p { class: "text-sm text-muted-foreground", {t("rooms.no_aliases")} }
                                            }

                                            // Add alias form
                                            div { class: "flex items-center gap-2 mt-4 pt-4 border-t",
                                                input {
                                                    class: "flex h-10 flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm font-mono ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2",
                                                    placeholder: "#alias:example.com",
                                                    value: new_alias.read().clone(),
                                                    oninput: move |evt: FormEvent| new_alias.set(evt.value()),
                                                }
                                                Button {
                                                    variant: ButtonVariant::Outline,
                                                    onclick: {
                                                        let rid = room_id_for_alias.clone();
                                                        move |_| {
                                                            let rid = rid.clone();
                                                            let alias = new_alias.read().clone();
                                                            if alias.is_empty() { return; }
                                                            spawn(async move {
                                                                match rooms::put_room_alias(&alias, &rid).await {
                                                                    Ok(_) => {
                                                                        show_toast("Alias added", ToastVariant::Success);
                                                                        new_alias.set(String::new());
                                                                        state_for_aliases.restart();
                                                                    }
                                                                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                }
                                                            });
                                                        }
                                                    },
                                                    Icon { name: "plus".to_string(), class: "h-4 w-4 mr-1".to_string() }
                                                    {t("rooms.add_alias")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }},
                            Tab::Members => rsx! {
                                RoomMembers { room_id: room_id_for_tabs.clone() }
                            },
                            Tab::Messages => rsx! {
                                RoomMessages { room_id: room_id_for_tabs.clone() }
                                EventLookup {}
                            },
                            Tab::State => rsx! {
                                RoomStateEvents { room_id: room_id_for_tabs.clone() }
                            },
                            Tab::ForwardExtremities => rsx! {
                                RoomForwardExtremities { room_id: room_id_for_tabs.clone() }
                            },
                            Tab::Hierarchy => rsx! {
                                RoomHierarchy { room_id: room_id_for_tabs.clone() }
                            },
                        }

                        // Purge History dialog
                        Modal {
                            open: *show_purge_dialog.read(),
                            on_close: move |_| show_purge_dialog.set(false),
                            size: ModalSize::Md,
                                    h2 { class: "text-lg font-semibold mb-2", {t("rooms.purge_title")} }
                                    p { class: "text-sm text-muted-foreground mb-4",
                                        {t("rooms.purge_description")}
                                    }
                                    div { class: "space-y-4",
                                        div { class: "space-y-2",
                                            label { class: "text-sm font-medium", {t("rooms.purge_before")} }
                                            input {
                                                r#type: "date",
                                                class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm",
                                                value: purge_date.read().clone(),
                                                oninput: move |evt: FormEvent| purge_date.set(evt.value()),
                                            }
                                        }
                                        div { class: "rounded-md bg-destructive/10 p-3",
                                            p { class: "text-sm text-destructive font-medium",
                                                {t("rooms.purge_warning")}
                                            }
                                        }
                                        div { class: "responsive-action-row",
                                            Button {
                                                variant: ButtonVariant::Outline,
                                                onclick: move |_| show_purge_dialog.set(false),
                                                {t("common.cancel")}
                                            }
                                            Button {
                                                variant: ButtonVariant::Destructive,
                                                disabled: purge_date.read().is_empty() || is_purge_loading,
                                                onclick: {
                                                    let rid = room_id_for_purge.clone();
                                                    move |_| {
                                                        let rid = rid.clone();
                                                        let date_str = purge_date.read().clone();
                                                        if date_str.is_empty() {
                                                            return;
                                                        }
                                                        let ts_ms = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
                                                            .ok()
                                                            .and_then(|d| d.and_hms_opt(0, 0, 0))
                                                            .and_then(|dt| dt.and_utc().timestamp_millis().try_into().ok())
                                                            .unwrap_or(0u64);
                                                        if ts_ms == 0 {
                                                            show_toast("Invalid date", ToastVariant::Error);
                                                            return;
                                                        }
                                                        purge_loading.set(true);
                                                        spawn(async move {
                                                            match rooms::purge_history(&rid, ts_ms).await {
                                                                Ok(_) => {
                                                                    show_toast("History purge initiated", ToastVariant::Success);
                                                                    show_purge_dialog.set(false);
                                                                    purge_date.set(String::new());
                                                                }
                                                                Err(e) => show_toast(&format!("Failed to purge: {}", e.message), ToastVariant::Error),
                                                            }
                                                            purge_loading.set(false);
                                                        });
                                                    }
                                                },
                                                if is_purge_loading {
                                                    Spinner { class: "mr-2".to_string() }
                                                }
                                                {t("rooms.purge_history")}
                                            }
                                        }
                                    }
                        }

                        // Delete dialog
                        ConfirmDialog {
                            open: *show_delete_dialog.read(),
                            title: t("rooms.delete"),
                            description: t("rooms.delete_confirm"),
                            confirm_text: t("rooms.delete"),
                            destructive: true,
                            on_confirm: move |_| {
                                let rid = room_id_for_delete.clone();
                                show_delete_dialog.set(false);
                                spawn(async move {
                                    match rooms::delete_room(&rid, false).await {
                                        Ok(_) => {
                                            show_toast("Room deleted", ToastVariant::Success);
                                            add_typed_notification(
                                                &format!("Room deleted: {rid}"),
                                                NotificationSeverity::Info,
                                                Some(NotificationType::SystemInfo),
                                            );
                                            nav.push(Route::RoomList {});
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
                    div { class: "rounded-md bg-destructive/10 p-4 text-sm text-destructive",
                        "Error loading room: {e.message}"
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }
    }
}

/// Room members list with moderation actions
#[component]
fn RoomMembers(room_id: String) -> Element {
    let rid = room_id.clone();
    let rid_for_invite = room_id.clone();
    let mut invite_user_id = use_signal(|| String::new());

    let mut members_data = use_resource(move || {
        let id = rid.clone();
        async move { rooms::get_room_members(&id, 1, 50).await }
    });

    rsx! {
        Card {
            CardHeader {
                CardTitle { {t("rooms.members")} }
            }
            CardContent {
                // Invite user input
                div { class: "flex items-center gap-2 mb-4",
                    input {
                        class: "flex h-10 flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2",
                        placeholder: t("rooms.invite_user_id"),
                        value: invite_user_id.read().clone(),
                        oninput: move |evt: FormEvent| invite_user_id.set(evt.value()),
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        onclick: {
                            let rid = rid_for_invite.clone();
                            move |_| {
                                let rid = rid.clone();
                                let uid = invite_user_id.read().clone();
                                if uid.is_empty() {
                                    return;
                                }
                                spawn(async move {
                                    match rooms::invite_user(&rid, &uid).await {
                                        Ok(_) => {
                                            show_toast("User invited", ToastVariant::Success);
                                            invite_user_id.set(String::new());
                                            members_data.restart();
                                        }
                                        Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                    }
                                });
                            }
                        },
                        Icon { name: "plus".to_string(), class: "h-4 w-4 mr-1".to_string() }
                        {t("rooms.invite")}
                    }
                }

                match &*members_data.read() {
                    Some(Ok(resp)) => rsx! {
                        if resp.data.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("rooms.no_members")} }
                        } else {
                            p { class: "text-sm text-muted-foreground mb-3", "{resp.total} members" }
                            div { class: "space-y-1",
                                for member in resp.data.iter() {
                                    {
                                        let member_id = member.clone();
                                        let rid_for_kick = room_id.clone();
                                        let rid_for_ban = room_id.clone();
                                        let rid_for_promote = room_id.clone();
                                        let mid_for_kick = member_id.clone();
                                        let mid_for_ban = member_id.clone();
                                        let mid_for_promote = member_id.clone();

                                        rsx! {
                                            div {
                                                key: "{member_id}",
                                                class: "flex items-center justify-between py-2 border-b last:border-0",
                                                div { class: "flex items-center min-w-0 flex-1",
                                                    Icon { name: "user".to_string(), class: "h-4 w-4 text-muted-foreground mr-2 shrink-0".to_string() }
                                                    Link {
                                                        to: Route::UserShow { user_id: urlencoding::encode(&member_id).to_string() },
                                                        class: "text-sm font-mono truncate text-primary hover:underline",
                                                        "{member_id}"
                                                    }
                                                }
                                                div { class: "flex items-center gap-1 shrink-0",
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: crate::components::ui::button::ButtonSize::Sm,
                                                        onclick: move |_| {
                                                            let rid = rid_for_promote.clone();
                                                            let mid = mid_for_promote.clone();
                                                            spawn(async move {
                                                                match rooms::make_room_admin(&rid, &mid).await {
                                                                    Ok(_) => show_toast("User promoted to admin", ToastVariant::Success),
                                                                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                }
                                                            });
                                                        },
                                                        {t("rooms.promote")}
                                                    }
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: crate::components::ui::button::ButtonSize::Sm,
                                                        onclick: move |_| {
                                                            let rid = rid_for_kick.clone();
                                                            let mid = mid_for_kick.clone();
                                                            spawn(async move {
                                                                match rooms::kick_user(&rid, &mid, "Kicked by admin").await {
                                                                    Ok(_) => {
                                                                        show_toast("User kicked", ToastVariant::Success);
                                                                        members_data.restart();
                                                                    }
                                                                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                }
                                                            });
                                                        },
                                                        {t("rooms.kick")}
                                                    }
                                                    Button {
                                                        variant: ButtonVariant::Ghost,
                                                        size: crate::components::ui::button::ButtonSize::Sm,
                                                        class: "text-destructive hover:text-destructive".to_string(),
                                                        onclick: move |_| {
                                                            let rid = rid_for_ban.clone();
                                                            let mid = mid_for_ban.clone();
                                                            spawn(async move {
                                                                match rooms::ban_user(&rid, &mid, "Banned by admin").await {
                                                                    Ok(_) => {
                                                                        show_toast("User banned", ToastVariant::Success);
                                                                        members_data.restart();
                                                                    }
                                                                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                                                }
                                                            });
                                                        },
                                                        {t("reports.ban_user")}
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
                        p { class: "text-sm text-destructive", "Error: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

/// Room messages viewer
#[component]
fn RoomMessages(room_id: String) -> Element {
    let rid = room_id.clone();

    let messages_data = use_resource(move || {
        let id = rid.clone();
        async move { rooms::get_room_messages(&id, 50).await }
    });

    rsx! {
        Card {
            CardHeader { CardTitle { {t("rooms.messages")} } }
            CardContent {
                match &*messages_data.read() {
                    Some(Ok(messages)) => rsx! {
                        if messages.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("rooms.no_messages")} }
                        } else {
                            div { class: "space-y-1",
                                for msg in messages.iter() {
                                    {
                                        let body = msg.content.body.clone().unwrap_or_else(|| {
                                            format!("[{}]", msg.event_type)
                                        });
                                        let ts = msg.origin_server_ts;
                                        let time_str = if ts > 0 {
                                            chrono::DateTime::from_timestamp_millis(ts as i64)
                                                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                                                .unwrap_or_else(|| "-".to_string())
                                        } else {
                                            "-".to_string()
                                        };
                                        let event_id = msg.event_id.clone();
                                        rsx! {
                                            div {
                                                key: "{event_id}",
                                                class: "flex flex-col gap-1 rounded-md border px-4 py-3",
                                                div { class: "flex items-center justify-between",
                                                    span { class: "text-sm font-medium text-primary", "{msg.sender}" }
                                                    span { class: "text-xs text-muted-foreground", "{time_str}" }
                                                }
                                                p { class: "text-sm", "{body}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        p { class: "text-sm text-destructive", "Error: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

/// Room state events viewer
#[component]
fn RoomStateEvents(room_id: String) -> Element {
    let rid = room_id.clone();

    let state_data = use_resource(move || {
        let id = rid.clone();
        async move { rooms::get_room_state(&id).await }
    });

    rsx! {
        Card {
            CardHeader { CardTitle { {t("rooms.state_events")} } }
            CardContent {
                match &*state_data.read() {
                    Some(Ok(events)) => rsx! {
                        if events.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("rooms.no_state_events")} }
                        } else {
                            div { class: "space-y-3",
                                for event in events.iter() {
                                    details { class: "border rounded-md p-3",
                                        summary { class: "text-sm font-medium cursor-pointer",
                                            span { class: "font-mono text-primary", "{event.event_type}" }
                                            span { class: "text-muted-foreground ml-2 text-xs", "by {event.sender}" }
                                        }
                                        pre {
                                            class: "text-xs font-mono bg-muted p-3 rounded mt-2 overflow-auto max-h-64",
                                            {serde_json::to_string_pretty(&event.content).unwrap_or_else(|_| "{}".to_string())}
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        p { class: "text-sm text-destructive", "Error: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

/// Room hierarchy viewer (for spaces)
#[component]
fn RoomHierarchy(room_id: String) -> Element {
    let rid = room_id.clone();

    let hierarchy_data = use_resource(move || {
        let id = rid.clone();
        async move { rooms::get_room_hierarchy(&id).await }
    });

    rsx! {
        Card {
            CardHeader {
                CardTitle { {t("rooms.space_hierarchy")} }
                CardDescription { {t("rooms.space_hierarchy_desc")} }
            }
            CardContent {
                match &*hierarchy_data.read() {
                    Some(Ok(rooms)) => rsx! {
                        if rooms.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("rooms.no_children")} }
                        } else {
                            div { class: "space-y-1",
                                for (idx, child) in rooms.iter().enumerate() {
                                    {
                                        let child_name = child.name.clone().unwrap_or_else(|| child.room_id.clone());
                                        let child_topic = child.topic.clone().unwrap_or_default();
                                        let child_members = child.num_joined_members;
                                        let child_room_id = child.room_id.clone();
                                        let is_sub_space = child.room_type.as_deref() == Some("m.space");
                                        // First item is the space itself; indent child rooms
                                        let indent = if idx == 0 { "" } else { "pl-6" };
                                        rsx! {
                                            div {
                                                key: "{child_room_id}",
                                                class: "flex items-center justify-between py-2 border-b last:border-0 {indent}",
                                                div { class: "flex items-center gap-2 min-w-0 flex-1",
                                                    if is_sub_space {
                                                        Icon { name: "layers".to_string(), class: "h-4 w-4 text-muted-foreground shrink-0".to_string() }
                                                    } else {
                                                        Icon { name: "message-square".to_string(), class: "h-4 w-4 text-muted-foreground shrink-0".to_string() }
                                                    }
                                                    div { class: "min-w-0",
                                                        div { class: "text-sm font-medium truncate", "{child_name}" }
                                                        if !child_topic.is_empty() {
                                                            p { class: "text-xs text-muted-foreground truncate max-w-md", "{child_topic}" }
                                                        }
                                                        p { class: "text-xs text-muted-foreground font-mono", "{child_room_id}" }
                                                    }
                                                }
                                                div { class: "flex items-center gap-1 shrink-0 ml-4",
                                                    Icon { name: "users".to_string(), class: "h-3 w-3 text-muted-foreground".to_string() }
                                                    span { class: "text-xs text-muted-foreground", "{child_members}" }
                                                    if is_sub_space {
                                                        Badge { variant: BadgeVariant::Secondary, {t("rooms.space")} }
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
                        p { class: "text-sm text-destructive", "Error loading hierarchy: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

/// Room forward extremities viewer
#[component]
fn RoomForwardExtremities(room_id: String) -> Element {
    let rid = room_id.clone();

    let extremities_data = use_resource(move || {
        let id = rid.clone();
        async move { rooms::get_forward_extremities(&id).await }
    });

    rsx! {
        Card {
            CardHeader {
                CardTitle { {t("rooms.forward_extremities")} }
                CardDescription { {t("rooms.forward_extremities_desc")} }
            }
            CardContent {
                match &*extremities_data.read() {
                    Some(Ok(resp)) => rsx! {
                        p { class: "text-sm text-muted-foreground mb-3",
                            {format!("{}: {}", t("rooms.count"), resp.count)}
                        }
                        if resp.results.is_empty() {
                            p { class: "text-sm text-muted-foreground", {t("rooms.no_forward_extremities")} }
                        } else {
                            if resp.count > 1 {
                                div { class: "rounded-md bg-warning/10 p-3 mb-4",
                                    p { class: "text-sm text-warning font-medium",
                                        {t("rooms.forward_extremities_warning")}
                                    }
                                }
                            }
                            div { class: "space-y-2",
                                for ext in resp.results.iter() {
                                    {
                                        let event_id = ext.event_id.clone();
                                        let ts = ext.received_ts;
                                        let time_str = if ts > 0 {
                                            chrono::DateTime::from_timestamp_millis(ts)
                                                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                                                .unwrap_or_else(|| "-".to_string())
                                        } else {
                                            "-".to_string()
                                        };
                                        rsx! {
                                            div {
                                                key: "{event_id}",
                                                class: "border rounded-md p-3 space-y-1",
                                                div { class: "flex items-center justify-between",
                                                    span { class: "text-sm font-mono text-primary truncate max-w-[70%]", "{event_id}" }
                                                    span { class: "text-xs text-muted-foreground", "{time_str}" }
                                                }
                                                div { class: "flex items-center gap-4 text-xs text-muted-foreground",
                                                    span { {format!("Depth: {}", ext.depth)} }
                                                    if let Some(sg) = ext.state_group {
                                                        span { {format!("State Group: {}", sg)} }
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
                        p { class: "text-sm text-destructive", "Error: {e.message}" }
                    },
                    None => rsx! {
                        LoadingSkeleton {}
                    },
                }
            }
        }
    }
}

/// Room directory visibility toggle
#[component]
fn RoomDirectoryToggle(room_id: String) -> Element {
    let rid = room_id.clone();
    let rid_for_toggle = room_id.clone();
    let mut dir_loading = use_signal(|| false);

    let mut visibility_data = use_resource(move || {
        let id = rid.clone();
        async move { rooms::get_room_directory_visibility(&id).await }
    });

    let is_loading = *dir_loading.read();
    let is_published = match &*visibility_data.read() {
        Some(Ok(v)) => *v,
        _ => false,
    };

    rsx! {
        div { class: "flex items-center justify-between py-2",
            span { class: "text-sm font-medium text-muted-foreground", {t("rooms.directory_listing")} }
            div { class: "flex items-center gap-2",
                if is_published {
                    Badge {
                        variant: BadgeVariant::Success,
                        {t("rooms.published")}
                    }
                } else {
                    Badge {
                        variant: BadgeVariant::Outline,
                        {t("rooms.unpublished")}
                    }
                }
                Button {
                    variant: ButtonVariant::Ghost,
                    size: crate::components::ui::button::ButtonSize::Sm,
                    disabled: is_loading,
                    onclick: {
                        let rid = rid_for_toggle.clone();
                        move |_| {
                            let rid = rid.clone();
                            let new_vis = !is_published;
                            dir_loading.set(true);
                            spawn(async move {
                                match rooms::set_room_directory_visibility(&rid, new_vis).await {
                                    Ok(_) => {
                                        let msg = if new_vis { "Room published to directory" } else { "Room unpublished from directory" };
                                        show_toast(msg, ToastVariant::Success);
                                        visibility_data.restart();
                                    }
                                    Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
                                }
                                dir_loading.set(false);
                            });
                        }
                    },
                    if is_loading {
                        Spinner { class: "h-4 w-4".to_string() }
                    } else if is_published {
                        {t("rooms.unpublish")}
                    } else {
                        {t("rooms.publish")}
                    }
                }
            }
        }
    }
}

/// Event lookup by ID
#[component]
fn EventLookup() -> Element {
    let mut event_id_input = use_signal(|| String::new());
    let mut event_result = use_signal(|| None::<Result<serde_json::Value, String>>);
    let mut loading = use_signal(|| false);

    let is_loading = *loading.read();

    rsx! {
        Card {
            CardHeader {
                CardTitle { {t("rooms.event_lookup")} }
                CardDescription { {t("rooms.event_lookup_desc")} }
            }
            CardContent {
                div { class: "space-y-4",
                    div { class: "flex items-center gap-2",
                        input {
                            class: "flex h-10 flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm font-mono ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2",
                            placeholder: "$event_id",
                            value: event_id_input.read().clone(),
                            oninput: move |evt: FormEvent| event_id_input.set(evt.value()),
                        }
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: event_id_input.read().is_empty() || is_loading,
                            onclick: move |_| {
                                let eid = event_id_input.read().clone();
                                if eid.is_empty() { return; }
                                loading.set(true);
                                spawn(async move {
                                    match rooms::fetch_event(&eid).await {
                                        Ok(event) => event_result.set(Some(Ok(event))),
                                        Err(e) => event_result.set(Some(Err(e.message))),
                                    }
                                    loading.set(false);
                                });
                            },
                            if is_loading {
                                Spinner { class: "mr-2".to_string() }
                            }
                            Icon { name: "search".to_string(), class: "h-4 w-4 mr-1".to_string() }
                            {t("rooms.lookup")}
                        }
                    }

                    match &*event_result.read() {
                        Some(Ok(event)) => rsx! {
                            pre {
                                class: "text-xs font-mono bg-muted p-3 rounded overflow-auto max-h-96",
                                {serde_json::to_string_pretty(event).unwrap_or_else(|_| "{}".to_string())}
                            }
                        },
                        Some(Err(e)) => rsx! {
                            div { class: "rounded-md bg-destructive/10 p-3",
                                p { class: "text-sm text-destructive", "Error: {e}" }
                            }
                        },
                        None => rsx! {},
                    }
                }
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
