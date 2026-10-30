use dioxus::prelude::*;

use crate::api::auth;
use crate::components::layout::AppLayout;
use crate::pages;
use crate::utils::i18n::t;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[route("/")]
    Landing {},

    #[route("/login")]
    LoginPage {},

    #[route("/oauth/callback?:code&:state&:error&:error_description")]
    OAuthCallback {
        code: Option<String>,
        state: Option<String>,
        error: Option<String>,
        error_description: Option<String>,
    },

    #[layout(AuthenticatedLayout)]
        #[route("/hagency/home")]
        Home {},

        #[route("/admin")]
        Dashboard {},

        #[route("/hagency/projects")]
        HagencyProjects {},
        #[route("/hagency/agents")]
        HagencyAgents {},

        #[route("/admin/users")]
        UserList {},
        #[route("/admin/users/create")]
        UserCreate {},
        #[route("/admin/users/known")]
        KnownUserList {},
        #[route("/admin/users/known/:user_id")]
        KnownUserShow { user_id: String },
        #[route("/admin/users/:user_id")]
        UserShow { user_id: String },

        #[route("/admin/rooms")]
        RoomList {},
        #[route("/admin/rooms/:room_id")]
        RoomShow { room_id: String },

        #[route("/admin/media")]
        MediaList {},

        #[route("/admin/reports")]
        ReportList {},
        #[route("/admin/reports/:report_id")]
        ReportShow { report_id: String },

        #[route("/admin/destinations")]
        DestinationList {},
        #[route("/admin/destinations/:destination_id")]
        DestinationShow { destination_id: String },

        #[route("/admin/registration-tokens")]
        RegistrationTokenList {},

        #[route("/admin/appservices")]
        AppserviceList {},

        #[route("/admin/auth-status")]
        AuthStatus {},

        #[route("/admin/server-status")]
        ServerStatus {},

        #[route("/admin/server-actions")]
        ServerActions {},

        #[route("/admin/server-notices")]
        ServerNotices {},

        #[route("/admin/server-notifications")]
        ServerNotifications {},

        #[route("/admin/settings/notifications")]
        NotificationPreferences {},

        #[route("/admin/pasion/accounts")]
        PasionAccounts {},
        #[route("/admin/pasion/accounts/:user_id")]
        PasionAccountShow { user_id: String },
        #[route("/admin/pasion/audit-log")]
        PasionAuditLog {},
        #[route("/admin/pasion/oauth2-sessions")]
        PasionOAuth2Sessions {},
        #[route("/admin/pasion/personal-sessions")]
        PasionPersonalSessions {},
        #[route("/admin/pasion/connector-health")]
        PasionConnectorHealth {},
        #[route("/admin/pasion/upstream-providers")]
        PasionUpstreamProviders {},
        #[route("/admin/pasion/upstream-links")]
        PasionUpstreamLinks {},
        #[route("/admin/pasion/notification-channels")]
        PasionNotificationChannels {},
        #[route("/admin/pasion/notification-templates")]
        PasionNotificationTemplates {},
    #[end_layout]

    #[route("/:..route")]
    NotFound { route: Vec<String> },
}

#[component]
pub fn AppRouter() -> Element {
    rsx! {
        Router::<Route> {}
    }
}

impl Route {
    /// Shared by direct navigation, OAuth return paths and permission tests.
    pub fn requires_admin(&self) -> bool {
        !matches!(
            self,
            Self::Landing {}
                | Self::Home {}
                | Self::HagencyProjects {}
                | Self::HagencyAgents {}
                | Self::LoginPage {}
                | Self::OAuthCallback { .. }
                | Self::NotFound { .. }
        )
    }
}

/// Session-scoped admin verdict, shared across the whole router.
///
///   Some(true)  = the servers accepted this user as an administrator
///   Some(false) = verified non-admin (forbidden)
///   None        = not yet probed (or the probe failed)
///
/// `verify_admin()` is an HTTP round-trip, so in-session navigation reuses
/// this verdict. It only ever comes from a successful server probe: nothing
/// persisted client side can grant access, and a failed probe never counts
/// as admin. It is cleared on logout and after a 401 (see
/// `reset_admin_cache`), and flipped to `false` when a later 403 turns out
/// to mean the admin flag was revoked (see `auth::handle_forbidden`).
static ADMIN_VERDICT: GlobalSignal<Option<bool>> = GlobalSignal::new(|| None);

/// Last admin-probe failure, shown with a retry button instead of letting
/// the user in.
static ADMIN_PROBE_ERROR: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);

/// Clear the cached admin verdict so the next navigation re-probes. Call this
/// when the session is invalidated (e.g. after a 401 or on logout).
pub fn reset_admin_cache() {
    *ADMIN_VERDICT.write() = None;
    *ADMIN_PROBE_ERROR.write() = None;
}

/// Record a verdict obtained from a server probe.
pub fn set_admin_verdict(is_admin: bool) {
    *ADMIN_VERDICT.write() = Some(is_admin);
    *ADMIN_PROBE_ERROR.write() = None;
}

pub fn set_access_error(message: String) {
    *ADMIN_VERDICT.write() = None;
    *ADMIN_PROBE_ERROR.write() = Some(message);
}

pub fn is_server_admin() -> bool {
    *ADMIN_VERDICT.read() == Some(true)
}

/// Only known application routes can be restored after an OAuth redirect.
pub fn restorable_route(path: &str, admin: bool) -> Option<Route> {
    let route: Route = path.parse().ok()?;
    if matches!(
        route,
        Route::Landing {}
            | Route::LoginPage {}
            | Route::OAuthCallback { .. }
            | Route::NotFound { .. }
    ) {
        return None;
    }
    if !admin && route.requires_admin() {
        return None;
    }
    Some(route)
}

#[cfg(test)]
mod return_route_tests {
    use super::*;
    #[test]
    fn oauth_return_does_not_allow_external_or_unauthorized_routes() {
        for path in [
            "https://evil.example/users",
            "//evil.example/users",
            "/login",
            "/oauth/callback",
            "/does-not-exist",
            "/",
        ] {
            assert!(restorable_route(path, true).is_none());
        }
        assert!(matches!(
            restorable_route("/admin/users", true),
            Some(Route::UserList {})
        ));
        assert!(restorable_route("/admin/users", false).is_none());
        assert!(matches!(
            restorable_route("/hagency/agents", false),
            Some(Route::HagencyAgents {})
        ));
    }

    #[test]
    fn member_routes_stay_in_place_and_admin_routes_require_verified_role() {
        for path in ["/hagency/home", "/hagency/projects", "/hagency/agents"] {
            let route = restorable_route(path, false).unwrap();
            assert!(!route.requires_admin());
            assert_eq!(route.to_string(), path);
        }
        for path in [
            "/admin",
            "/admin/users",
            "/admin/rooms",
            "/admin/pasion/accounts",
            "/admin/settings/notifications",
        ] {
            assert!(restorable_route(path, false).is_none());
            assert!(restorable_route(path, true).unwrap().requires_admin());
        }
        assert!(restorable_route("/users", true).is_none());
    }
}

#[component]
fn AuthenticatedLayout() -> Element {
    let nav = use_navigator();
    let route = use_route::<Route>();

    let router = dioxus::prelude::router();
    let mut probe = use_resource(move || {
        async move {
            // A reload retains only the HttpOnly session, never an OAuth bearer.
            // Revalidate that session before rendering owner management pages.
            *ADMIN_PROBE_ERROR.write() = None;
            match auth::verify_access_on_load().await {
                Ok(Some(flag)) => {
                    set_admin_verdict(flag);
                }
                Ok(None) => {} // Redirecting for the newly granted administrator role.
                Err(err) if err.status == 401 => {
                    crate::api::browser_auth::clear_session();
                    nav.replace(Route::LoginPage {});
                }
                // Never fall back to "admin": without a positive answer from the
                // servers the dashboard stays closed.
                Err(err) => set_access_error(err.message),
            }
        }
    });

    // Restore the bearer grant when navigating to an administrator page
    // after reloading with only the HttpOnly browser session.
    let mut authorization = use_resource(move || {
        let route = router.current::<Route>();
        let admin = *ADMIN_VERDICT.read() == Some(true);
        async move {
            if admin
                && route.requires_admin()
                && !auth::has_oauth_token()
                && let Err(error) = auth::start_login().await
            {
                *ADMIN_PROBE_ERROR.write() = Some(error.message);
            }
        }
    });

    let verdict = *ADMIN_VERDICT.read();
    let probe_error = ADMIN_PROBE_ERROR.read().clone();
    let member_page = !route.requires_admin();
    if verdict == Some(true) && !member_page && !auth::has_oauth_token() {
        return rsx! { div { class: "p-6", "Restoring Pasion authorization..."
            if let Some(message) = probe_error { p { role: "alert", "{message}" }
                button { onclick: move |_| { probe.restart(); authorization.restart(); }, "Retry" }
            }
        } };
    }
    match (verdict, probe_error) {
        (Some(true), _) => rsx! {
            AppLayout {
                Outlet::<Route> {}
            }
        },
        (Some(false), _) => {
            let member_page = !route.requires_admin();
            rsx! { AppLayout {
                if member_page { Outlet::<Route> {} } else { pages::not_authorized::NotAuthorizedPage {} }
            } }
        }
        (None, Some(message)) => rsx! {
            div { class: "flex min-h-screen items-center justify-center bg-background p-4",
                div { class: "w-full max-w-md space-y-4 text-center",
                    h1 { class: "text-2xl font-bold tracking-tight", "Unable to verify account access" }
                    p { class: "text-muted-foreground text-sm break-all", "{message}" }
                    crate::components::ui::button::Button {
                        class: "w-full".to_string(),
                        onclick: move |_| { probe.restart(); authorization.restart(); },
                        "Retry"
                    }
                    crate::components::ui::button::Button {
                        variant: crate::components::ui::button::ButtonVariant::Outline,
                        class: "w-full".to_string(),
                        onclick: move |_| {
                            auth::logout_and_redirect(nav);
                        },
                        {crate::utils::i18n::t("auth.sign_out")}
                    }
                }
            }
        },
        (None, None) => rsx! {
            div { class: "flex min-h-screen items-center justify-center",
                crate::components::ui::loading::Spinner { class: String::new() }
            }
        },
    }
}

#[component]
fn Landing() -> Element {
    rsx! { pages::login::LoginPage {} }
}

#[component]
fn LoginPage() -> Element {
    rsx! {
        pages::login::LoginPage {}
    }
}

#[component]
fn HagencyProjects() -> Element {
    rsx! { pages::hagency::projects::Projects {} }
}
#[component]
fn HagencyAgents() -> Element {
    rsx! { pages::hagency::agents::Agents {} }
}

#[component]
fn OAuthCallback(
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
) -> Element {
    rsx! {
        pages::oauth_callback::OAuthCallback {
            code: code,
            state: state,
            error: error,
            error_description: error_description,
        }
    }
}

#[component]
fn Home() -> Element {
    rsx! {
        div { class: "space-y-6",
            h1 { class: "text-2xl font-bold tracking-tight", "Hagency" }
            p { class: "text-muted-foreground", {t("home.description")} }
            div { class: "flex flex-wrap gap-4",
                Link { to: Route::HagencyProjects {}, class: "hg-link", {t("home.projects")} }
                Link { to: Route::HagencyAgents {}, class: "hg-link", {t("home.agents")} }
                a { href: "/auth/", class: "hg-link", {t("home.account")} }
                if is_server_admin() {
                    Link { to: Route::Dashboard {}, class: "hg-link", {t("home.admin")} }
                }
            }
        }
    }
}

#[component]
fn Dashboard() -> Element {
    rsx! {
        pages::dashboard::Dashboard {}
    }
}

#[component]
fn UserList() -> Element {
    rsx! {
        pages::users::list::UserList {}
    }
}

#[component]
fn UserCreate() -> Element {
    if crate::utils::storage::get_item("oauth_enabled").as_deref() == Some("true") {
        return rsx! { div { class:"hg-page",
            h1 { "Create a server account" }
            p { "Pasion manages accounts, passwords and administrator roles for this server." }
            Link { to:Route::PasionAccounts {},class:"hg-link","Create an account in Account management" }
        } };
    }
    rsx! {
        pages::users::create::UserCreate {}
    }
}

#[component]
fn UserShow(user_id: String) -> Element {
    rsx! {
        pages::users::show::UserShow { user_id }
    }
}

#[component]
fn KnownUserList() -> Element {
    rsx! {
        pages::users::known::KnownUserList {}
    }
}

#[component]
fn KnownUserShow(user_id: String) -> Element {
    rsx! {
        pages::users::known_show::KnownUserShow { user_id }
    }
}

#[component]
fn RoomList() -> Element {
    rsx! {
        pages::rooms::list::RoomList {}
    }
}

#[component]
fn RoomShow(room_id: String) -> Element {
    rsx! {
        pages::rooms::show::RoomShow { room_id }
    }
}

#[component]
fn MediaList() -> Element {
    rsx! {
        pages::media::MediaList {}
    }
}

#[component]
fn ReportList() -> Element {
    rsx! {
        pages::reports::ReportList {}
    }
}

#[component]
fn ReportShow(report_id: String) -> Element {
    rsx! {
        pages::reports::ReportShow { report_id }
    }
}

#[component]
fn DestinationList() -> Element {
    rsx! {
        pages::destinations::DestinationList {}
    }
}

#[component]
fn DestinationShow(destination_id: String) -> Element {
    rsx! {
        pages::destinations::DestinationShow { destination_id }
    }
}

#[component]
fn RegistrationTokenList() -> Element {
    rsx! {
        pages::registration_tokens::RegistrationTokenList {}
    }
}

#[component]
fn AppserviceList() -> Element {
    rsx! {
        pages::appservices::AppserviceList {}
    }
}

#[component]
fn AuthStatus() -> Element {
    rsx! {
        pages::auth_status::AuthStatusPage {}
    }
}

#[component]
fn ServerStatus() -> Element {
    rsx! {
        pages::server_status::ServerStatus {}
    }
}

#[component]
fn ServerActions() -> Element {
    rsx! {
        pages::server_actions::ServerActions {}
    }
}

#[component]
fn ServerNotices() -> Element {
    rsx! {
        pages::server_notices::ServerNotices {}
    }
}

#[component]
fn ServerNotifications() -> Element {
    rsx! {
        pages::server_notifications::ServerNotifications {}
    }
}

#[component]
fn NotificationPreferences() -> Element {
    rsx! {
        pages::notification_preferences::NotificationPreferencesPage {}
    }
}

#[component]
fn PasionAccounts() -> Element {
    rsx! { pages::pasion::accounts::PasionAccountsPage {} }
}

#[component]
fn PasionAccountShow(user_id: String) -> Element {
    rsx! { pages::pasion::account_show::PasionAccountShowPage { user_id } }
}

#[component]
fn PasionAuditLog() -> Element {
    rsx! { pages::pasion::audit_log::AuditLogPage {} }
}

#[component]
fn PasionOAuth2Sessions() -> Element {
    rsx! { pages::pasion::oauth2_sessions::OAuth2SessionsPage {} }
}

#[component]
fn PasionPersonalSessions() -> Element {
    rsx! { pages::pasion::personal_sessions::PersonalSessionsPage {} }
}

#[component]
fn PasionConnectorHealth() -> Element {
    rsx! { pages::pasion::connector_health::ConnectorHealthPage {} }
}

#[component]
fn PasionUpstreamProviders() -> Element {
    rsx! { pages::pasion::upstream_providers::UpstreamProvidersPage {} }
}

#[component]
fn PasionUpstreamLinks() -> Element {
    rsx! { pages::pasion::upstream_links::UpstreamLinksPage {} }
}

#[component]
fn PasionNotificationChannels() -> Element {
    rsx! { pages::pasion::notification_channels::NotificationChannelsPage {} }
}

#[component]
fn PasionNotificationTemplates() -> Element {
    rsx! { pages::pasion::notification_templates::NotificationTemplatesPage {} }
}

#[component]
fn NotFound(route: Vec<String>) -> Element {
    rsx! {
        div { class: "flex items-center justify-center min-h-screen",
            div { class: "text-center",
                h1 { class: "text-4xl font-bold mb-4", {t("not_found.title")} }
                p { class: "text-muted-foreground mb-4",
                    {format!("{}: /{}", t("not_found.message"), route.join("/"))}
                }
                Link { to: Route::Home {}, class: "text-primary hover:underline",
                    {t("not_found.go_dashboard")}
                }
            }
        }
    }
}
