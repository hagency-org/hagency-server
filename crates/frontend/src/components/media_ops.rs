use dioxus::prelude::*;

use crate::api::media;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::dialog::Modal;
use crate::components::ui::input::{Input, Label};
use crate::components::ui::loading::Spinner;
use crate::components::ui::toast::{ToastVariant, show_toast};

#[component]
pub fn DeleteMediaDialog(
    open: bool,
    on_close: EventHandler<()>,
    #[props(default)] on_success: Option<EventHandler<()>>,
) -> Element {
    let mut before_ts = use_signal(String::new);
    let mut size_gt = use_signal(String::new);
    let mut keep_profiles = use_signal(|| true);
    let mut loading = use_signal(|| false);

    let handle_delete = move |_: MouseEvent| {
        let ts_str = before_ts.read().clone();
        let size_str = size_gt.read().clone();
        let keep = *keep_profiles.read();

        let ts: u64 = ts_str.parse().unwrap_or(0);
        let size: u64 = size_str.parse().unwrap_or(0);

        loading.set(true);
        spawn(async move {
            match media::delete_local_media(ts, size, keep).await {
                Ok(result) => {
                    show_toast(
                        &format!("Deleted {} media files", result.total),
                        ToastVariant::Success,
                    );
                    loading.set(false);
                    if let Some(ref cb) = on_success {
                        cb.call(());
                    }
                    on_close.call(());
                }
                Err(e) => {
                    show_toast(&format!("Failed: {}", e.message), ToastVariant::Error);
                    loading.set(false);
                }
            }
        });
    };

    let is_loading = *loading.read();

    rsx! {
        Modal { open, on_close: move |_| on_close.call(()),
                h2 { class: "text-lg font-semibold mb-4", "Delete Local Media" }
                div { class: "space-y-4",
                    div { class: "space-y-2",
                        Label { "Delete media before (timestamp in ms)" }
                        Input {
                            r#type: "number".to_string(),
                            placeholder: "Timestamp in milliseconds".to_string(),
                            value: before_ts.read().clone(),
                            oninput: move |evt: FormEvent| before_ts.set(evt.value()),
                        }
                    }
                    div { class: "space-y-2",
                        Label { "Only delete media larger than (bytes)" }
                        Input {
                            r#type: "number".to_string(),
                            placeholder: "0".to_string(),
                            value: size_gt.read().clone(),
                            oninput: move |evt: FormEvent| size_gt.set(evt.value()),
                        }
                    }
                    div { class: "flex items-center space-x-2",
                        input {
                            r#type: "checkbox",
                            id: "keep_profiles",
                            class: "h-4 w-4 rounded border-input",
                            checked: *keep_profiles.read(),
                            onchange: move |_| {
                                let current = *keep_profiles.read();
                                keep_profiles.set(!current);
                            },
                        }
                        Label { r#for: "keep_profiles".to_string(), "Keep profile images" }
                    }
                }
                div { class: "flex justify-end gap-2 mt-4",
                    Button { variant: ButtonVariant::Outline, onclick: move |_| on_close.call(()), "Cancel" }
                    Button {
                        variant: ButtonVariant::Destructive,
                        disabled: is_loading,
                        onclick: handle_delete,
                        if is_loading { Spinner { class: "mr-2".to_string() } }
                        "Delete Media"
                    }
                }
        }
    }
}

#[component]
pub fn PurgeRemoteMediaDialog(
    open: bool,
    on_close: EventHandler<()>,
    #[props(default)] on_success: Option<EventHandler<()>>,
) -> Element {
    let mut before_ts = use_signal(String::new);
    let mut loading = use_signal(|| false);

    let handle_purge = move |_: MouseEvent| {
        let ts_str = before_ts.read().clone();
        let ts: u64 = ts_str.parse().unwrap_or(0);

        loading.set(true);
        spawn(async move {
            match media::purge_remote_media(ts).await {
                Ok(result) => {
                    show_toast(
                        &format!("Purged {} media files", result.total),
                        ToastVariant::Success,
                    );
                    loading.set(false);
                    if let Some(ref cb) = on_success {
                        cb.call(());
                    }
                    on_close.call(());
                }
                Err(e) => {
                    show_toast(&format!("Failed: {}", e.message), ToastVariant::Error);
                    loading.set(false);
                }
            }
        });
    };

    let is_loading = *loading.read();

    rsx! {
        Modal { open, on_close: move |_| on_close.call(()),
                h2 { class: "text-lg font-semibold mb-4", "Purge Remote Media" }
                div { class: "space-y-4",
                    div { class: "space-y-2",
                        Label { "Purge remote media cached before (timestamp in ms)" }
                        Input {
                            r#type: "number".to_string(),
                            placeholder: "Timestamp in milliseconds".to_string(),
                            value: before_ts.read().clone(),
                            oninput: move |evt: FormEvent| before_ts.set(evt.value()),
                        }
                    }
                }
                div { class: "flex justify-end gap-2 mt-4",
                    Button { variant: ButtonVariant::Outline, onclick: move |_| on_close.call(()), "Cancel" }
                    Button {
                        variant: ButtonVariant::Destructive,
                        disabled: is_loading,
                        onclick: handle_purge,
                        if is_loading { Spinner { class: "mr-2".to_string() } }
                        "Purge Remote Media"
                    }
                }
        }
    }
}
