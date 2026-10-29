use gloo_storage::{LocalStorage, SessionStorage, Storage};

#[path = "session_state.rs"]
mod session_state;
pub use session_state::{ActorCheck, Recovery, SessionState, Snapshot, Tokens};

thread_local! {
    static SESSION_SECRETS: SessionState = SessionState::default();
}

pub fn session_state() -> SessionState {
    SESSION_SECRETS.with(Clone::clone)
}
pub fn session_generation() -> u64 {
    session_state().generation()
}

fn session_changed() {
    crate::utils::cache::clear_cache();
    crate::api::browser_auth::clear_session();
    for key in [
        "user_id",
        "user_display_name",
        "user_avatar_url",
        "is_admin",
    ] {
        LocalStorage::delete(key);
        SessionStorage::delete(key);
    }
    crate::router::reset_admin_cache();
}

fn session_only(key: &str) -> bool {
    matches!(
        key,
        "access_token" | "refresh_token" | "access_token_expires_at"
    )
}

pub fn get_item(key: &str) -> Option<String> {
    if session_only(key) {
        LocalStorage::delete(key);
        SessionStorage::delete(key);
        return session_state().get(key);
    }
    LocalStorage::get::<String>(key).ok()
}

pub fn set_item(key: &str, value: &str) {
    if session_only(key) {
        session_state().set(key, value);
        if key == "access_token" {
            session_changed();
        }
        LocalStorage::delete(key);
        SessionStorage::delete(key);
    } else {
        let _ = LocalStorage::set(key, value.to_string());
    }
}

pub fn remove_item(key: &str) {
    LocalStorage::delete(key);
    SessionStorage::delete(key);
    session_state().remove(key);
    if key == "access_token" {
        session_changed();
    }
}

/// Retire in-flight refreshes before awaiting any logout network operation.
pub fn retire_session() {
    session_state().clear();
    // Logout in this tab must not erase another tab's shared account metadata.
    crate::utils::cache::clear_cache();
    crate::api::browser_auth::clear_session();
    crate::router::reset_admin_cache();
}

// Nonsecret operation references are kept per tab for idempotent retries.
pub fn get_session_item(key: &str) -> Option<String> {
    SessionStorage::get(key).ok()
}
pub fn set_session_item(key: &str, value: &str) {
    let _ = SessionStorage::set(key, value);
}
pub fn remove_session_item(key: &str) {
    SessionStorage::delete(key);
}
