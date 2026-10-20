use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ButtonVariant {
    #[default]
    Default,
    Destructive,
    Outline,
    Secondary,
    Ghost,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ButtonSize {
    #[default]
    Default,
    Sm,
    Lg,
}

#[component]
pub fn Button(
    #[props(default)] variant: ButtonVariant,
    #[props(default)] size: ButtonSize,
    #[props(default)] class: String,
    #[props(default)] disabled: bool,
    #[props(default)] autofocus: bool,
    #[props(default = "button".to_string())] r#type: String,
    #[props(default)] onclick: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    let variant_class = match variant {
        ButtonVariant::Default => "btn-gradient",
        ButtonVariant::Destructive => {
            "bg-destructive text-destructive-foreground hover:bg-destructive/90"
        }
        ButtonVariant::Outline => "btn-outline",
        ButtonVariant::Secondary => "btn-secondary",
        ButtonVariant::Ghost => "hover:bg-accent hover:text-accent-foreground",
    };

    let size_class = match size {
        ButtonSize::Default => "h-10 px-4 py-2",
        ButtonSize::Sm => "h-9 px-3",
        ButtonSize::Lg => "h-11 px-8",
    };

    let disabled_class = if disabled {
        "opacity-50 pointer-events-none"
    } else {
        ""
    };

    rsx! {
        button {
            r#type,
            class: "inline-flex items-center justify-center whitespace-nowrap rounded-lg text-sm font-semibold ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 touch-target {variant_class} {size_class} {disabled_class} {class}",
            disabled,
            autofocus,
            onclick: move |evt| onclick.call(evt),
            {children}
        }
    }
}
