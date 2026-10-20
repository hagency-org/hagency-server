use dioxus::prelude::*;

use crate::api::pasion;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::components::ui::error_banner::ErrorBanner;
use crate::components::ui::loading::{PageSkeleton, Spinner};
use crate::components::ui::page_header::PageHeader;
use crate::components::ui::table::*;
use crate::components::ui::toast::{ToastVariant, show_toast};
use crate::utils::i18n::t;

#[component]
pub fn NotificationTemplatesPage() -> Element {
    let mut templates_data =
        use_resource(|| async { pasion::pasion_get_notification_templates().await });

    // Publish dialog state
    let mut show_publish = use_signal(|| false);
    let mut publish_template_key = use_signal(String::new);
    let mut publish_locale = use_signal(|| "en".to_string());
    let mut publish_channel = use_signal(|| "email".to_string());
    let mut publish_subject_template = use_signal(String::new);
    let mut publish_body_template = use_signal(String::new);
    let mut publishing = use_signal(|| false);

    let handle_publish = move |_: MouseEvent| {
        let template_key = publish_template_key.read().clone();
        let locale = publish_locale.read().clone();
        let channel = publish_channel.read().clone();
        let subject_template = publish_subject_template.read().clone();
        let body_template = publish_body_template.read().clone();

        if template_key.trim().is_empty() {
            show_toast("template_key is required", ToastVariant::Error);
            return;
        }
        if body_template.trim().is_empty() {
            show_toast("body_template is required", ToastVariant::Error);
            return;
        }

        let mut data = serde_json::json!({
            "template_key": template_key.trim(),
            "locale": locale,
            "channel": channel,
            "body_template": body_template,
        });
        if !subject_template.is_empty() {
            data["subject_template"] = serde_json::Value::String(subject_template);
        }

        publishing.set(true);
        spawn(async move {
            match pasion::pasion_publish_template(data).await {
                Ok(_) => {
                    show_toast("Template published", ToastVariant::Success);
                    show_publish.set(false);
                    publish_template_key.set(String::new());
                    publish_subject_template.set(String::new());
                    publish_body_template.set(String::new());
                    templates_data.restart();
                }
                Err(e) => show_toast(&format!("Failed: {}", e.message), ToastVariant::Error),
            }
            publishing.set(false);
        });
    };

    let is_publishing = *publishing.read();

    rsx! {
        div { class: "space-y-6",
            PageHeader {
                title: t("pasion.notification_templates.title"),
                description: t("pasion.notification_templates.description"),
                Button {
                    onclick: move |_| {
                        publish_template_key.set(String::new());
                        publish_locale.set("en".to_string());
                        publish_channel.set("email".to_string());
                        publish_subject_template.set(String::new());
                        publish_body_template.set(String::new());
                        show_publish.set(true);
                    },
                    "Publish Template"
                }
            }

            match &*templates_data.read() {
                Some(Ok(templates)) => rsx! {
                    div { class: "rounded-md border",
                        Table {
                            TableHeader {
                                TableRow {
                                    TableHead { "Template Key" }
                                    TableHead { "Description" }
                                    TableHead { class: "text-right".to_string(), "Actions" }
                                }
                            }
                            TableBody {
                                if templates.is_empty() {
                                    EmptyRow { colspan: 3, message: t("pasion.notification_templates.empty") }
                                } else {
                                    for template in templates.iter() {
                                        {
                                            let key = template.key.clone();
                                            let key_for_publish = key.clone();
                                            let description = template.description.clone();

                                            rsx! {
                                                TableRow { key: "{key}",
                                                    TableCell {
                                                        span { class: "font-medium text-sm font-mono", "{key}" }
                                                    }
                                                    TableCell {
                                                        span { class: "text-sm text-muted-foreground", "{description}" }
                                                    }
                                                    TableCell { class: "text-right".to_string(),
                                                        Button {
                                                            variant: ButtonVariant::Ghost,
                                                            size: ButtonSize::Sm,
                                                            onclick: move |_| {
                                                                publish_template_key.set(key_for_publish.clone());
                                                                show_publish.set(true);
                                                            },
                                                            "Publish"
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
                Some(Err(e)) => rsx! {
                    ErrorBanner {
                        message: e.message.clone(),
                        on_retry: move |_| templates_data.restart(),
                    }
                },
                None => rsx! { PageSkeleton {} },
            }
        }

        // Publish dialog
        Modal {
            open: *show_publish.read(),
            on_close: move |_| {
                if !is_publishing { show_publish.set(false); }
            },
                    h2 { class: "text-lg font-semibold mb-4", "Publish Notification Template" }
                    div { class: "space-y-4",
                        div { class: "space-y-2",
                            label { class: "text-sm font-medium leading-none", "Template Key" }
                            input {
                                class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm",
                                placeholder: "verification",
                                value: publish_template_key.read().clone(),
                                oninput: move |evt: FormEvent| publish_template_key.set(evt.value()),
                                disabled: is_publishing,
                            }
                        }
                        div { class: "grid grid-cols-2 gap-4",
                            div { class: "space-y-2",
                                label { class: "text-sm font-medium leading-none", "Locale" }
                                input {
                                    class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm",
                                    placeholder: "en",
                                    value: publish_locale.read().clone(),
                                    oninput: move |evt: FormEvent| publish_locale.set(evt.value()),
                                    disabled: is_publishing,
                                }
                            }
                            div { class: "space-y-2",
                                label { class: "text-sm font-medium leading-none", "Channel" }
                                select {
                                    class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm",
                                    disabled: is_publishing,
                                    onchange: move |evt: FormEvent| publish_channel.set(evt.value()),
                                    option { value: "email", "Email" }
                                    option { value: "sms", "SMS" }
                                }
                            }
                        }
                        div { class: "space-y-2",
                            label { class: "text-sm font-medium leading-none", "Subject Template (optional)" }
                            input {
                                class: "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm",
                                placeholder: "Email subject",
                                value: publish_subject_template.read().clone(),
                                oninput: move |evt: FormEvent| publish_subject_template.set(evt.value()),
                                disabled: is_publishing,
                            }
                        }
                        div { class: "space-y-2",
                            label { class: "text-sm font-medium leading-none", "Body Template" }
                            textarea {
                                class: "flex min-h-[120px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring resize-y font-mono",
                                placeholder: "Template body content...",
                                disabled: is_publishing,
                                oninput: move |evt: FormEvent| publish_body_template.set(evt.value()),
                                "{publish_body_template.read().clone()}"
                            }
                        }
                    }
                    div { class: "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2 mt-6",
                        Button {
                            variant: ButtonVariant::Outline,
                            disabled: is_publishing,
                            onclick: move |_| show_publish.set(false),
                            "Cancel"
                        }
                        Button {
                            disabled: is_publishing,
                            onclick: handle_publish,
                            if is_publishing {
                                Spinner { class: "mr-2".to_string() }
                            }
                            "Publish"
                        }
                    }
        }
    }
}
