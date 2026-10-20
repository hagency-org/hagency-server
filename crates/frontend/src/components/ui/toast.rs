use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum ToastVariant {
    Default,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToastAction {
    pub label: String,
    pub href: String,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub variant: ToastVariant,
    pub action: Option<ToastAction>,
}

static TOAST_COUNTER: GlobalSignal<u64> = GlobalSignal::new(|| 0);
pub static TOASTS: GlobalSignal<Vec<Toast>> = GlobalSignal::new(Vec::new);

const MAX_TOASTS: usize = 5;

pub fn show_toast(message: &str, variant: ToastVariant) {
    show_toast_inner(message, variant, None);
}

fn show_toast_inner(message: &str, variant: ToastVariant, action: Option<ToastAction>) {
    let id = {
        let mut counter = TOAST_COUNTER.write();
        *counter += 1;
        *counter
    };

    let timeout_ms = match variant {
        ToastVariant::Error => 5000,
        _ => 3000,
    };

    // If there's an action, give more time to click it
    let timeout_ms = if action.is_some() {
        timeout_ms.max(5000)
    } else {
        timeout_ms
    };

    {
        let mut toasts = TOASTS.write();
        toasts.push(Toast {
            id,
            message: message.to_string(),
            variant,
            action,
        });
        // Keep only the latest MAX_TOASTS
        if toasts.len() > MAX_TOASTS {
            let drain_count = toasts.len() - MAX_TOASTS;
            toasts.drain(0..drain_count);
        }
    }

    spawn(async move {
        gloo_timers::future::TimeoutFuture::new(timeout_ms).await;
        dismiss_toast(id);
    });
}

fn dismiss_toast(id: u64) {
    let mut toasts = TOASTS.write();
    toasts.retain(|t| t.id != id);
}

#[component]
pub fn Toaster() -> Element {
    let toasts = TOASTS.read();

    if toasts.is_empty() {
        return rsx! {};
    }

    rsx! {
        div {
            class: "fixed bottom-4 right-4 z-50 flex flex-col gap-2",
            aria_live: "polite",
            aria_atomic: "false",
            for toast in toasts.iter() {
                {
                    let toast_id = toast.id;
                    let bg_class = match toast.variant {
                        ToastVariant::Default => "bg-background border",
                        ToastVariant::Success => "bg-green-500 text-white",
                        ToastVariant::Error => "bg-destructive text-destructive-foreground",
                    };
                    let action = toast.action.clone();

                    rsx! {
                        div {
                            key: "{toast_id}",
                            role: if matches!(toast.variant, ToastVariant::Error) { "alert" } else { "status" },
                            class: "flex items-center gap-2 rounded-lg px-4 py-3 shadow-lg toast-enter {bg_class}",
                            p { class: "text-sm font-medium flex-1", "{toast.message}" }
                            if let Some(action) = action {
                                a {
                                    href: "{action.href}",
                                    class: "ml-2 text-sm font-medium underline underline-offset-2 hover:opacity-80 whitespace-nowrap",
                                    "{action.label}"
                                }
                            }
                            button {
                                r#type: "button",
                                aria_label: "Dismiss notification",
                                class: "ml-2 text-sm opacity-70 hover:opacity-100 font-bold leading-none",
                                onclick: move |_| dismiss_toast(toast_id),
                                "\u{00D7}"
                            }
                        }
                    }
                }
            }
        }
    }
}
