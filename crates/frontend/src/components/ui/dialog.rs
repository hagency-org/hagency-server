use dioxus::prelude::*;
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::JsCast;

use super::button::{Button, ButtonVariant};

/// Width presets for [`Modal`]. Maps to the `max-w-*` Tailwind utility used by
/// the panel.
#[derive(Clone, Copy, PartialEq)]
pub enum ModalSize {
    Md,
    Lg,
    Xl2,
    Xl3,
}

impl ModalSize {
    fn max_width_class(self) -> &'static str {
        match self {
            ModalSize::Md => "max-w-md",
            ModalSize::Lg => "max-w-lg",
            ModalSize::Xl2 => "max-w-2xl",
            ModalSize::Xl3 => "max-w-3xl",
        }
    }
}

/// Generic modal shell: a full-screen centered overlay with a dimmed backdrop
/// and a centered panel. Clicking the backdrop invokes `on_close`; clicks
/// inside the panel do not propagate to the backdrop.
///
/// Renders nothing when `open` is false.
///
/// - `size` selects the panel's `max-w-*` width (default [`ModalSize::Lg`]).
/// - `bg` overrides the panel background class (default `"bg-background"`; pass
///   `"glass-panel"` for the frosted variant).
/// - `panel_class` appends extra classes to the panel (e.g.
///   `"max-h-[80vh] overflow-y-auto"`). Pass `"p-0 ..."` to drop the default
///   `p-6` padding when the children manage their own header/body/footer
///   padding.
/// - `default_padding` controls whether the panel gets the default `p-6`
///   padding (default `true`). Set `false` for panels that render their own
///   padded sections (e.g. a scrollable header/body/footer layout).
/// - `container_class` appends extra classes to the full-screen container
///   (e.g. `"items-start overflow-y-auto p-4 sm:items-center"` to let the whole
///   dialog scroll on short viewports).
/// - `children` is the panel's content (header, body, footer).
#[component]
pub fn Modal(
    open: bool,
    on_close: EventHandler<()>,
    #[props(default = "Dialog".to_string())] label: String,
    #[props(default = ModalSize::Lg)] size: ModalSize,
    #[props(default = "bg-background".to_string())] bg: String,
    #[props(default = String::new())] panel_class: String,
    #[props(default = true)] default_padding: bool,
    #[props(default = String::new())] container_class: String,
    children: Element,
) -> Element {
    let dialog_id = super::input::use_dom_id("dialog");
    let previous_focus = use_hook(|| Rc::new(RefCell::new(None::<web_sys::HtmlElement>)));
    let restore = previous_focus.clone();
    use_effect(use_reactive!(|open| {
        if !open && let Some(element) = restore.borrow_mut().take() {
            let _ = element.focus();
        }
    }));
    let restore = previous_focus.clone();
    use_drop(move || {
        if let Some(element) = restore.borrow_mut().take() {
            let _ = element.focus();
        }
    });
    if !open {
        return rsx! {};
    }

    let padding = if default_padding { "p-6" } else { "" };
    let panel_class = format!(
        "relative z-50 w-full {} rounded-lg border {} {} shadow-lg {}",
        size.max_width_class(),
        bg,
        padding,
        panel_class
    );
    let panel_class = panel_class.split_whitespace().collect::<Vec<_>>().join(" ");

    let container_class = format!(
        "fixed inset-0 z-50 flex items-center justify-center {}",
        container_class
    );
    let container_class = container_class.trim_end().to_string();

    rsx! { dialog {
        id: dialog_id.clone(),
        aria_label: label,
        style: "position:fixed;inset:0;width:100vw;height:100vh;max-width:none;max-height:none;margin:0;padding:0;border:0;background:transparent;",
        onmounted: move |_| {
            if let Some(document)=web_sys::window().and_then(|w|w.document()) {
                *previous_focus.borrow_mut()=document.active_element().and_then(|e|e.dyn_into().ok());
                if let Some(dialog)=document.get_element_by_id(&dialog_id).and_then(|e|e.dyn_into::<web_sys::HtmlDialogElement>().ok())
                    && dialog.show_modal().is_err() { on_close.call(()); }
            }
        },
        onkeydown: move |event: KeyboardEvent| { if event.key()==Key::Escape { event.prevent_default(); on_close.call(()); } },
        div { class: "{container_class}",
            // Backdrop
            div {
                class: "fixed inset-0 bg-black/80",
                onclick: move |_| on_close.call(()),
            }
            // Panel
            div {
                class: "{panel_class}",
                onclick: move |evt: MouseEvent| evt.stop_propagation(),
                {children}
            }
        }
    } }
}

#[component]
pub fn ConfirmDialog(
    open: bool,
    title: String,
    description: String,
    #[props(default = "Confirm".to_string())] confirm_text: String,
    #[props(default = "Cancel".to_string())] cancel_text: String,
    #[props(default = false)] destructive: bool,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        Modal {
            open,
            label: title.clone(),
            on_close: move |_| on_cancel.call(()),
            bg: "glass-panel".to_string(),
            div {
                div { class: "flex flex-col space-y-2 text-center sm:text-left",
                    h2 { id: "confirm-dialog-title", class: "text-lg font-semibold", "{title}" }
                    p { id: "confirm-dialog-description", class: "text-sm text-muted-foreground", "{description}" }
                }
                div { class: "flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2 mt-4",
                    Button {
                        variant: ButtonVariant::Outline,
                        autofocus: true,
                        onclick: move |_| on_cancel.call(()),
                        "{cancel_text}"
                    }
                    Button {
                        variant: if destructive { ButtonVariant::Destructive } else { ButtonVariant::Default },
                        onclick: move |_| on_confirm.call(()),
                        "{confirm_text}"
                    }
                }
            }
        }
    }
}
