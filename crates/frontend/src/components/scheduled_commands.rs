use dioxus::prelude::*;

use crate::api::palpo_admin;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::*;
use crate::components::ui::dialog::{ConfirmDialog, Modal};
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::{LoadingSkeleton, Spinner};
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::types::{RecurringCommand, ScheduledCommand};
use crate::utils::i18n::t;

fn command_args_input(args: &Option<serde_json::Value>) -> String {
    args.as_ref()
        .map(|value| match value {
            serde_json::Value::String(text) => text.clone(),
            _ => value.to_string(),
        })
        .unwrap_or_default()
}

fn command_args_value(input: &str) -> Option<serde_json::Value> {
    if input.is_empty() {
        None
    } else {
        Some(serde_json::json!(input))
    }
}

/// A command row flattened to the fields the shared list/dialog render. The
/// single `time` field maps to `ScheduledCommand::scheduled_at` or
/// `RecurringCommand::time` depending on the [`CommandKind`].
#[derive(Clone, PartialEq)]
struct CommandRow {
    id: String,
    command: String,
    args: Option<serde_json::Value>,
    /// scheduled_at (scheduled) or time (recurring).
    time: Option<String>,
    /// Carried through for ScheduledCommand round-trips.
    is_recurring: bool,
    /// Carried through for RecurringCommand round-trips.
    scheduled_at: Option<String>,
}

impl From<ScheduledCommand> for CommandRow {
    fn from(c: ScheduledCommand) -> Self {
        CommandRow {
            id: c.id,
            command: c.command,
            args: c.args,
            time: c.scheduled_at,
            is_recurring: c.is_recurring,
            scheduled_at: None,
        }
    }
}

impl From<RecurringCommand> for CommandRow {
    fn from(c: RecurringCommand) -> Self {
        CommandRow {
            id: c.id,
            command: c.command,
            args: c.args,
            time: c.time,
            is_recurring: false,
            scheduled_at: c.scheduled_at,
        }
    }
}

/// Selects which command flavor the shared list/dialog operate on. Dispatches
/// the API calls and supplies the type-specific i18n keys.
#[derive(Clone, Copy, PartialEq)]
enum CommandKind {
    Scheduled,
    Recurring,
}

impl CommandKind {
    async fn fetch(self, url: &str) -> Option<Vec<CommandRow>> {
        match self {
            CommandKind::Scheduled => palpo_admin::get_scheduled_commands(url)
                .await
                .ok()
                .map(|v| v.into_iter().map(CommandRow::from).collect()),
            CommandKind::Recurring => palpo_admin::get_recurring_commands(url)
                .await
                .ok()
                .map(|v| v.into_iter().map(CommandRow::from).collect()),
        }
    }

    async fn delete(self, url: &str, id: &str) -> Result<(), crate::utils::error::HttpError> {
        match self {
            CommandKind::Scheduled => palpo_admin::delete_scheduled_command(url, id).await,
            CommandKind::Recurring => palpo_admin::delete_recurring_command(url, id).await,
        }
    }

    /// Save (create or update) a row. Returns Ok on success.
    async fn save(
        self,
        url: &str,
        row: &CommandRow,
        is_editing: bool,
    ) -> Result<(), crate::utils::error::HttpError> {
        match self {
            CommandKind::Scheduled => {
                let sc = ScheduledCommand {
                    id: row.id.clone(),
                    command: row.command.clone(),
                    scheduled_at: row.time.clone(),
                    args: row.args.clone(),
                    is_recurring: row.is_recurring,
                };
                if is_editing {
                    palpo_admin::update_scheduled_command(url, &sc)
                        .await
                        .map(|_| ())
                } else {
                    palpo_admin::create_scheduled_command(url, &sc)
                        .await
                        .map(|_| ())
                }
            }
            CommandKind::Recurring => {
                let rc = RecurringCommand {
                    id: row.id.clone(),
                    command: row.command.clone(),
                    time: row.time.clone(),
                    args: row.args.clone(),
                    scheduled_at: row.scheduled_at.clone(),
                };
                if is_editing {
                    palpo_admin::update_recurring_command(url, &rc)
                        .await
                        .map(|_| ())
                } else {
                    palpo_admin::create_recurring_command(url, &rc)
                        .await
                        .map(|_| ())
                }
            }
        }
    }

    fn list_title(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.scheduled_title"),
            CommandKind::Recurring => t("commands.recurring_title"),
        }
    }

    fn empty_text(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.none_scheduled"),
            CommandKind::Recurring => t("commands.none_recurring"),
        }
    }

    fn time_column(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.col_scheduled_at"),
            CommandKind::Recurring => t("commands.col_time"),
        }
    }

    fn time_label(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.scheduled_at_label"),
            CommandKind::Recurring => t("commands.time_label"),
        }
    }

    fn time_placeholder(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.scheduled_at_placeholder"),
            CommandKind::Recurring => t("commands.time_placeholder"),
        }
    }

    fn delete_title(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.delete_scheduled_title"),
            CommandKind::Recurring => t("commands.delete_recurring_title"),
        }
    }

    fn delete_desc(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.delete_scheduled_desc"),
            CommandKind::Recurring => t("commands.delete_recurring_desc"),
        }
    }

    fn validation_msg(self) -> String {
        match self {
            CommandKind::Scheduled => t("commands.validation_scheduled"),
            CommandKind::Recurring => t("commands.validation_recurring"),
        }
    }

    fn dialog_title(self, is_editing: bool) -> String {
        match (self, is_editing) {
            (CommandKind::Scheduled, true) => t("commands.dialog_edit_scheduled"),
            (CommandKind::Scheduled, false) => t("commands.dialog_schedule"),
            (CommandKind::Recurring, true) => t("commands.dialog_edit_recurring"),
            (CommandKind::Recurring, false) => t("commands.dialog_create_recurring"),
        }
    }

    fn submit_label(self, is_editing: bool) -> String {
        if is_editing {
            t("commands.submit_save")
        } else {
            match self {
                CommandKind::Scheduled => t("commands.submit_schedule"),
                CommandKind::Recurring => t("commands.submit_create"),
            }
        }
    }

    fn save_toast(self, is_editing: bool) -> String {
        match (self, is_editing) {
            (CommandKind::Scheduled, true) => t("commands.toast_updated"),
            (CommandKind::Scheduled, false) => t("commands.toast_scheduled"),
            (CommandKind::Recurring, true) => t("commands.toast_recurring_updated"),
            (CommandKind::Recurring, false) => t("commands.toast_recurring_created"),
        }
    }
}

// ── Public entry points ──

#[component]
pub fn ScheduledCommandsList(palpo_admin_url: String) -> Element {
    rsx! {
        CommandsList { palpo_admin_url, kind: CommandKind::Scheduled }
    }
}

#[component]
pub fn RecurringCommandsList(palpo_admin_url: String) -> Element {
    rsx! {
        CommandsList { palpo_admin_url, kind: CommandKind::Recurring }
    }
}

// ── Shared list ──

#[component]
fn CommandsList(palpo_admin_url: String, kind: CommandKind) -> Element {
    let url = palpo_admin_url.clone();
    let url_for_resource = url.clone();
    let mut data = use_resource(move || {
        let u = url_for_resource.clone();
        async move { kind.fetch(&u).await }
    });
    let mut show_create = use_signal(|| false);
    let mut edit_command = use_signal(|| Option::<CommandRow>::None);
    let mut delete_id = use_signal(|| Option::<String>::None);

    rsx! {
        Card {
            CardHeader { class: "flex flex-row items-center justify-between".to_string(),
                CardTitle { class: "text-base".to_string(), {kind.list_title()} }
                Button {
                    size: ButtonSize::Sm,
                    onclick: move |_| {
                        edit_command.set(None);
                        show_create.set(true);
                    },
                    {t("commands.create")}
                }
            }
            CardContent {
                match &*data.read() {
                    Some(Some(commands)) => {
                        if commands.is_empty() {
                            rsx! { p { class: "text-sm text-muted-foreground", {kind.empty_text()} } }
                        } else {
                            rsx! {
                                Table {
                                    TableHeader {
                                        TableRow {
                                            TableHead { {t("commands.col_command")} }
                                            TableHead { {t("commands.col_arguments")} }
                                            TableHead { {kind.time_column()} }
                                            TableHead { class: "text-right".to_string(), {t("commands.col_actions")} }
                                        }
                                    }
                                    TableBody {
                                        for cmd in commands.iter() {
                                            {
                                                let name = cmd.command.clone();
                                                let args = cmd.args.as_ref().map(|a| a.to_string()).unwrap_or_else(|| "-".to_string());
                                                let when = cmd.time.clone().unwrap_or_else(|| "-".to_string());
                                                let editable_cmd = cmd.clone();
                                                let cmd_id = cmd.id.clone();
                                                rsx! {
                                                    TableRow {
                                                        TableCell { class: "font-medium".to_string(), "{name}" }
                                                        TableCell { class: "text-muted-foreground text-xs font-mono".to_string(), "{args}" }
                                                        TableCell { "{when}" }
                                                        TableCell { class: "text-right".to_string(),
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: ButtonSize::Sm,
                                                                onclick: move |_| {
                                                                    show_create.set(false);
                                                                    edit_command.set(Some(editable_cmd.clone()));
                                                                },
                                                                {t("commands.edit")}
                                                            }
                                                            Button {
                                                                variant: ButtonVariant::Ghost,
                                                                size: ButtonSize::Sm,
                                                                class: "text-destructive".to_string(),
                                                                onclick: move |_| delete_id.set(Some(cmd_id.clone())),
                                                                {t("commands.delete")}
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
                    },
                    Some(None) => rsx! {
                        div { class: "flex items-center justify-between rounded-md bg-destructive/10 p-3",
                            p { class: "text-sm text-destructive", {t("commands.load_failed")} }
                            button { class: "text-sm font-medium text-primary hover:underline", onclick: move |_| data.restart(), {t("commands.retry")} }
                        }
                    },
                    None => rsx! { LoadingSkeleton {} },
                }
            }
        }

        if *show_create.read() || edit_command.read().is_some() {
            CommandDialog {
                palpo_admin_url: url.clone(),
                kind,
                initial_command: edit_command.read().clone(),
                on_close: move |_| {
                    show_create.set(false);
                    edit_command.set(None);
                },
                on_saved: move |_| {
                    show_create.set(false);
                    edit_command.set(None);
                    data.restart();
                },
            }
        }

        ConfirmDialog {
            open: delete_id.read().is_some(),
            title: kind.delete_title(),
            description: kind.delete_desc(),
            confirm_text: t("commands.delete"),
            destructive: true,
            on_confirm: {
                let url = url.clone();
                move |_| {
                    let id = delete_id.read().clone();
                    let u = url.clone();
                    if let Some(id) = id {
                        spawn(async move {
                            match kind.delete(&u, &id).await {
                                Ok(_) => { show_toast(&t("commands.toast_deleted"), ToastVariant::Success); data.restart(); }
                                Err(e) => show_toast(&format!("{}: {}", t("common.failed"), e.message), ToastVariant::Error),
                            }
                            delete_id.set(None);
                        });
                    }
                }
            },
            on_cancel: move |_| delete_id.set(None),
        }
    }
}

// ── Shared dialog ──

#[component]
fn CommandDialog(
    palpo_admin_url: String,
    kind: CommandKind,
    initial_command: Option<CommandRow>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let initial_for_name = initial_command.clone();
    let mut command = use_signal(move || {
        initial_for_name
            .as_ref()
            .map(|cmd| cmd.command.clone())
            .unwrap_or_default()
    });
    let initial_for_time = initial_command.clone();
    let mut time = use_signal(move || {
        initial_for_time
            .as_ref()
            .and_then(|cmd| cmd.time.clone())
            .unwrap_or_default()
    });
    let initial_for_args = initial_command.clone();
    let mut args = use_signal(move || {
        initial_for_args
            .as_ref()
            .map(|cmd| command_args_input(&cmd.args))
            .unwrap_or_default()
    });
    let mut loading = use_signal(|| false);
    let is_editing = initial_command.is_some();

    let url = palpo_admin_url.clone();
    let handle_save = move |_: MouseEvent| {
        let u = url.clone();
        let cmd = command.read().clone();
        let when = time.read().clone();
        let a = args.read().clone();
        let existing = initial_command.clone();

        if cmd.is_empty() || when.is_empty() {
            show_toast(&kind.validation_msg(), ToastVariant::Error);
            return;
        }

        loading.set(true);
        spawn(async move {
            let row = CommandRow {
                id: existing.as_ref().map(|c| c.id.clone()).unwrap_or_default(),
                command: cmd,
                args: command_args_value(&a),
                time: Some(when),
                is_recurring: existing.as_ref().map(|c| c.is_recurring).unwrap_or(false),
                scheduled_at: existing.as_ref().and_then(|c| c.scheduled_at.clone()),
            };
            match kind.save(&u, &row, is_editing).await {
                Ok(_) => {
                    show_toast(&kind.save_toast(is_editing), ToastVariant::Success);
                    on_saved.call(());
                }
                Err(e) => {
                    show_toast(
                        &format!("{}: {}", t("common.failed"), e.message),
                        ToastVariant::Error,
                    );
                    loading.set(false);
                }
            }
        });
    };

    let is_loading = *loading.read();

    rsx! {
        Modal { open: true, on_close: move |_| on_close.call(()),
            h2 { class: "text-lg font-semibold mb-4", {kind.dialog_title(is_editing)} }
            div { class: "space-y-4",
                crate::components::ui::input::Field { class: "space-y-2",
                    Label { {t("commands.field_command")} }
                    Input { placeholder: t("commands.field_command_placeholder"), value: command.read().clone(), oninput: move |e: FormEvent| command.set(e.value()) }
                }
                crate::components::ui::input::Field { class: "space-y-2",
                    Label { {t("commands.field_arguments")} }
                    Input { placeholder: t("commands.field_arguments_placeholder"), value: args.read().clone(), oninput: move |e: FormEvent| args.set(e.value()) }
                }
                crate::components::ui::input::Field { class: "space-y-2",
                    Label { {kind.time_label()} }
                    Input { placeholder: kind.time_placeholder(), value: time.read().clone(), oninput: move |e: FormEvent| time.set(e.value()) }
                }
            }
            div { class: "responsive-action-row mt-4",
                Button { variant: ButtonVariant::Outline, onclick: move |_| on_close.call(()), {t("commands.cancel")} }
                Button { disabled: is_loading, onclick: handle_save, if is_loading { Spinner { class: "mr-2".to_string() } } {kind.submit_label(is_editing)} }
            }
        }
    }
}
