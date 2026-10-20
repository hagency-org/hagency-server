use dioxus::prelude::*;

use crate::api::users;
use crate::components::ui::button::Button;
use crate::components::ui::dialog::Modal;
use crate::components::ui::loading::Spinner;
use crate::components::ui::toast::{ToastVariant, show_toast};

#[component]
pub fn ServerNoticeDialog(open: bool, user_id: String, on_close: EventHandler<()>) -> Element {
    let mut message = use_signal(String::new);
    let mut sending = use_signal(|| false);

    let uid = user_id.clone();
    let handle_send = move |_: MouseEvent| {
        let msg = message.read().clone();
        let user = uid.clone();

        if msg.is_empty() {
            show_toast("Message cannot be empty", ToastVariant::Error);
            return;
        }

        sending.set(true);

        spawn(async move {
            match users::send_server_notice(&user, &msg).await {
                Ok(_) => {
                    show_toast("Server notice sent", ToastVariant::Success);
                    message.set(String::new());
                    sending.set(false);
                    on_close.call(());
                }
                Err(e) => {
                    show_toast(&format!("Failed: {}", e.message), ToastVariant::Error);
                    sending.set(false);
                }
            }
        });
    };

    let is_sending = *sending.read();

    rsx! {
        Modal { open, on_close: move |_| on_close.call(()),
                h2 { class: "text-lg font-semibold mb-2", "Send Server Notice" }
                p { class: "text-sm text-muted-foreground mb-4", "Send a notice to {user_id}" }
                textarea {
                    class: "flex min-h-[100px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm mb-4",
                    placeholder: "Type your message...",
                    value: message.read().clone(),
                    oninput: move |evt: FormEvent| message.set(evt.value()),
                }
                div { class: "flex justify-end gap-2",
                    Button {
                        variant: crate::components::ui::button::ButtonVariant::Outline,
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    Button {
                        disabled: is_sending,
                        onclick: handle_send,
                        if is_sending {
                            Spinner { class: "mr-2".to_string() }
                        }
                        "Send"
                    }
                }
        }
    }
}
