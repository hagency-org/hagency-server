use dioxus::prelude::*;

use crate::utils::storage;

#[derive(Debug, Clone, PartialEq)]
pub enum Theme {
    Light,
    Dark,
    System,
}

impl Theme {
    pub fn from_str(s: &str) -> Self {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

fn get_system_theme() -> &'static str {
    let window = web_sys::window().unwrap();
    let matches = window
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
        .map(|m| m.matches())
        .unwrap_or(false);
    if matches { "dark" } else { "light" }
}

pub fn get_resolved_theme() -> String {
    let stored = storage::get_item("theme").unwrap_or_else(|| "system".to_string());
    let theme = Theme::from_str(&stored);
    match theme {
        Theme::Light => "light".to_string(),
        Theme::Dark => "dark".to_string(),
        Theme::System => get_system_theme().to_string(),
    }
}

pub fn apply_theme() {
    let preference = storage::get_item("theme").unwrap_or_else(|| "system".into());
    let explicit = explicit_theme_class(&preference);
    if let Some(document) = web_sys::window().and_then(|w| w.document())
        && let Some(root) = document.document_element()
    {
        let _ = root.class_list().remove_2("light", "dark");
        if let Some(explicit) = explicit {
            let _ = root.class_list().add_1(explicit);
        }
    }
}

pub fn set_theme(theme: &str) {
    storage::set_item("theme", theme);
    apply_theme();
}

#[component]
pub fn ThemeSwitcher() -> Element {
    let mut current =
        use_signal(|| storage::get_item("theme").unwrap_or_else(|| "system".to_string()));

    rsx! {
        div { class: "flex items-center gap-1 rounded-lg border p-1",
            for (label, value) in [("Light", "light"), ("Dark", "dark"), ("System", "system")] {
                button {
                    class: {
                        let is_active = *current.read() == value;
                        if is_active {
                            "px-2 py-1 text-xs rounded bg-primary text-primary-foreground"
                        } else {
                            "px-2 py-1 text-xs rounded hover:bg-muted"
                        }
                    },
                    onclick: move |_| {
                        set_theme(value);
                        current.set(value.to_string());
                    },
                    "{label}"
                }
            }
        }
    }
}

fn explicit_theme_class(preference: &str) -> Option<&'static str> {
    match Theme::from_str(preference) {
        Theme::Light => Some("light"),
        Theme::Dark => Some("dark"),
        Theme::System => None,
    }
}

/// CSS follows system colours; this listener keeps theme-toggle labels in sync too.
pub fn use_resolved_dark_mode() -> Signal<bool> {
    use wasm_bindgen::{JsCast, closure::Closure};
    struct Listener {
        media: web_sys::MediaQueryList,
        callback: Closure<dyn FnMut()>,
    }
    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = self.media.remove_event_listener_with_callback(
                "change",
                self.callback.as_ref().unchecked_ref(),
            );
        }
    }
    let mut dark = use_signal(|| get_resolved_theme() == "dark");
    use_hook(move || {
        web_sys::window()
            .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
            .map(|media| {
                let callback = Closure::wrap(Box::new(move || {
                    if explicit_theme_class(&storage::get_item("theme").unwrap_or_default())
                        .is_none()
                    {
                        dark.set(get_resolved_theme() == "dark");
                    }
                }) as Box<dyn FnMut()>);
                let _ = media
                    .add_event_listener_with_callback("change", callback.as_ref().unchecked_ref());
                std::rc::Rc::new(Listener { media, callback })
            })
    });
    dark
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_mode_does_not_pin_a_css_colour() {
        assert_eq!(explicit_theme_class("system"), None);
        assert_eq!(explicit_theme_class(""), None);
        assert_eq!(explicit_theme_class("dark"), Some("dark"));
        assert_eq!(explicit_theme_class("light"), Some("light"));
    }
}
