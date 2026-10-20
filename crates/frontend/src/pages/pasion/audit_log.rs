use dioxus::prelude::*;

use crate::api::pasion;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::input::Input;
use crate::components::ui::loading::PageSkeleton;
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::table::*;
use crate::types::PasionAuditEntry;
use crate::utils::i18n::t;

const PAGE_LIMIT: u64 = 25;

#[component]
pub fn AuditLogPage() -> Element {
    let mut entries = use_signal(Vec::<PasionAuditEntry>::new);
    let mut loading = use_signal(|| false);
    let mut load_error = use_signal(|| Option::<String>::None);
    let mut expanded_id = use_signal(|| Option::<String>::None);
    let mut op_filter = use_signal(String::new);

    // Initial load
    use_effect(move || {
        loading.set(true);
        spawn(async move {
            match pasion::pasion_get_audit_feed(PAGE_LIMIT).await {
                Ok(resp) => {
                    entries.set(resp.data);
                    load_error.set(None);
                }
                Err(e) => load_error.set(Some(e.message)),
            }
            loading.set(false);
        });
    });

    let is_loading = *loading.read();

    let filtered_memo = use_memo(move || {
        let filter_val = op_filter.read();
        entries
            .read()
            .iter()
            .filter(|e| filter_val.is_empty() || e.operation.contains(filter_val.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    });
    let filtered = filtered_memo.read();

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("pasion.audit_log.title"),
                description: t("pasion.audit_log.description"),
                Button {
                    variant: ButtonVariant::Outline,
                    disabled: is_loading,
                    onclick: move |_| {
                        expanded_id.set(None);
                        loading.set(true);
                        spawn(async move {
                            match pasion::pasion_get_audit_feed(PAGE_LIMIT).await {
                                Ok(resp) => {
                                    entries.set(resp.data);
                                    load_error.set(None);
                                }
                                Err(e) => load_error.set(Some(e.message)),
                            }
                            loading.set(false);
                        });
                    },
                    {t("common.refresh")}
                }
            }

            // Filters
            div { class: "flex items-center gap-4 max-w-sm",
                Input {
                    placeholder: t("pasion.audit_log.filter_placeholder"),
                    value: op_filter.read().clone(),
                    oninput: move |evt: FormEvent| op_filter.set(evt.value()),
                }
            }

            if let Some(err) = load_error.read().clone() {
                ErrorBanner { message: err }
            } else if is_loading && filtered.is_empty() {
                PageSkeleton {}
            } else {
                div { class: "rounded-md border",
                    Table {
                        TableHeader {
                            TableRow {
                                TableHead { {t("pasion.audit_log.col_timestamp")} }
                                TableHead { {t("pasion.audit_log.col_operation")} }
                                TableHead { {t("pasion.audit_log.col_admin")} }
                                TableHead { {t("pasion.audit_log.col_resource")} }
                                TableHead { {t("pasion.audit_log.col_detail")} }
                            }
                        }
                        TableBody {
                            if filtered.is_empty() {
                                EmptyRow { colspan: 5, message: t("pasion.audit_log.empty") }
                            } else {
                                for entry in filtered.iter() {
                                    {
                                        let entry_id = entry.id.clone();
                                        let entry_id2 = entry_id.clone();
                                        let op = entry.operation.clone();
                                        let created = entry.created_at.clone();
                                        let admin = entry.admin_user_id.clone().unwrap_or_else(|| "-".to_string());
                                        let resource = match (&entry.resource_type, &entry.resource_id) {
                                            (Some(rt), Some(rid)) if !rt.is_empty() && !rid.is_empty() => format!("{rt}: {rid}"),
                                            (Some(rt), _) if !rt.is_empty() => rt.clone(),
                                            _ => "-".to_string(),
                                        };
                                        let has_detail = entry.details.is_some();
                                        let detail_json = entry.details.as_ref()
                                            .map(|d| serde_json::to_string_pretty(d).unwrap_or_default())
                                            .unwrap_or_default();
                                        let is_expanded = expanded_id.read().as_deref() == Some(&entry_id);

                                        rsx! {
                                            TableRow {
                                                key: "{entry_id}",
                                                TableCell {
                                                    span { class: "text-xs text-muted-foreground font-mono", "{created}" }
                                                }
                                                TableCell {
                                                    Badge { variant: BadgeVariant::Secondary, "{op}" }
                                                }
                                                TableCell {
                                                    span { class: "text-xs font-mono", "{admin}" }
                                                }
                                                TableCell {
                                                    span { class: "text-xs text-muted-foreground", "{resource}" }
                                                }
                                                TableCell {
                                                    if has_detail {
                                                        Button {
                                                            variant: ButtonVariant::Ghost,
                                                            size: ButtonSize::Sm,
                                                            onclick: move |_| {
                                                                if is_expanded {
                                                                    expanded_id.set(None);
                                                                } else {
                                                                    expanded_id.set(Some(entry_id2.clone()));
                                                                }
                                                            },
                                                            if is_expanded { {t("common.hide")} } else { {t("common.show")} }
                                                        }
                                                    }
                                                }
                                            }
                                            if is_expanded {
                                                TableRow {
                                                    key: "{entry_id}-detail",
                                                    td { colspan: 5,
                                                        pre { class: "text-xs bg-muted p-3 rounded overflow-auto max-h-48 font-mono",
                                                            "{detail_json}"
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
    }
}
