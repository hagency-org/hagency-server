use std::collections::HashMap;

use dioxus::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Language {
    En,
    ZhCn,
}

impl Language {
    pub fn code(&self) -> &'static str {
        match self {
            Language::En => "en",
            Language::ZhCn => "zh-CN",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Language::En => "English",
            Language::ZhCn => "\u{4e2d}\u{6587}",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "en" => Some(Language::En),
            "zh-CN" | "zh_CN" | "zh" => Some(Language::ZhCn),
            _ => None,
        }
    }

    pub fn all() -> &'static [Language] {
        &[Language::En, Language::ZhCn]
    }
}

pub struct I18n {
    translations: HashMap<Language, HashMap<String, String>>,
}

impl I18n {
    pub fn new() -> Self {
        let mut translations = HashMap::new();
        translations.insert(Language::En, Self::load_en());
        translations.insert(Language::ZhCn, Self::load_zh_cn());
        Self { translations }
    }

    pub fn t(&self, key: &str, lang: Language) -> String {
        self.translations
            .get(&lang)
            .and_then(|m| m.get(key).cloned())
            .or_else(|| {
                self.translations
                    .get(&Language::En)
                    .and_then(|m| m.get(key).cloned())
            })
            .unwrap_or_else(|| key.to_string())
    }

    pub fn t_with(&self, key: &str, lang: Language, params: &[(&str, &str)]) -> String {
        let mut result = self.t(key, lang);
        for (k, v) in params {
            result = result.replace(&format!("{{{k}}}"), v);
        }
        result
    }

    fn load_en() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("welcome.title".into(), "Welcome to Hagency".into());
        m.insert(
            "welcome.description".into(),
            "Sign in to work with your agents, projects and conversations.".into(),
        );
        m.insert("welcome.sign_in".into(), "Sign in".into());
        m.insert("welcome.signing_in".into(), "Connecting…".into());
        m.insert("welcome.continue".into(), "Open your workspace".into());
        m.insert(
            "welcome.signed_in".into(),
            "You’re signed in. Your workspace is ready.".into(),
        );
        m.insert(
            "welcome.sign_in_hint".into(),
            "Use your account to continue.".into(),
        );
        m.insert("welcome.loading".into(), "Loading sign-in…".into());
        m.insert(
            "welcome.unavailable".into(),
            "Sign-in is unavailable on this server.".into(),
        );
        m.insert("welcome.create_account".into(), "Create an account".into());
        m.insert("welcome.account_links".into(), "Account options".into());
        m.insert(
            "welcome.caption".into(),
            "A shared place for your projects and agents.".into(),
        );
        m.insert("welcome.theme".into(), "Switch appearance".into());

        // Auth
        m.insert("auth.welcome".into(), "Welcome to {name}".into());
        m.insert("auth.base_url".into(), "Homeserver URL".into());
        m.insert("auth.credentials".into(), "Credentials".into());
        m.insert("auth.access_token".into(), "Access Token".into());
        m.insert("auth.username".into(), "Username".into());
        m.insert("auth.password".into(), "Password".into());
        m.insert("auth.sign_in".into(), "Sign In".into());
        m.insert("auth.sign_out".into(), "Sign Out".into());
        m.insert("auth.sso_sign_in".into(), "Sign in with SSO".into());
        m.insert("auth.server_version".into(), "Server version:".into());
        m.insert("auth.supports_specs".into(), "Supports specs:".into());
        m.insert(
            "auth.fields_required".into(),
            "Please enter your username and password.".into(),
        );
        m.insert(
            "auth.invalid_credentials".into(),
            "Invalid username or password.".into(),
        );
        m.insert(
            "auth.rate_limited".into(),
            "Too many attempts. Please try again later.".into(),
        );
        m.insert(
            "auth.account_deactivated".into(),
            "This account has been deactivated.".into(),
        );
        m.insert(
            "auth.account_locked".into(),
            "This account has been locked.".into(),
        );
        m.insert(
            "auth.username_placeholder".into(),
            "Username or email".into(),
        );
        m.insert(
            "auth.oauth_hint".into(),
            "Sign in with your account to access your projects and agents.".into(),
        );
        m.insert("auth.processing".into(), "Signing in...".into());
        m.insert("auth.try_again".into(), "Try again".into());
        m.insert(
            "auth.protocol_error".into(),
            "URL must start with http:// or https://".into(),
        );
        m.insert("auth.url_error".into(), "Not a valid homeserver URL".into());

        // Navigation

        m.insert("home.title".into(), "Home".into());
        m.insert(
            "home.description".into(),
            "Manage your projects, agents and account.".into(),
        );
        m.insert("home.projects".into(), "Projects".into());
        m.insert("home.agents".into(), "My agents".into());
        m.insert("home.account".into(), "Account center".into());
        m.insert("home.admin".into(), "Server administration".into());
        m.insert("nav.dashboard".into(), "Dashboard".into());
        m.insert("nav.users".into(), "Users".into());
        m.insert("nav.rooms".into(), "Rooms".into());
        m.insert("nav.media".into(), "Media".into());
        m.insert("nav.reports".into(), "Reports".into());
        m.insert("nav.federation".into(), "Federation".into());
        m.insert(
            "nav.registration_tokens".into(),
            "Registration Tokens".into(),
        );
        m.insert("nav.server_status".into(), "Server Status".into());
        m.insert("nav.server_actions".into(), "Server Actions".into());
        m.insert("nav.notifications".into(), "Notifications".into());
        m.insert("nav.server_notices".into(), "Server Notices".into());
        m.insert("nav.management".into(), "Management".into());
        m.insert("nav.section_identity".into(), "Identity".into());
        m.insert("nav.section_moderation".into(), "Moderation".into());
        m.insert("nav.section_infrastructure".into(), "Infrastructure".into());
        m.insert("nav.section_server_ops".into(), "Server Ops".into());
        m.insert("nav.section_pasion".into(), "Identity Provider".into());
        m.insert("nav.pasion_accounts".into(), "Local Accounts".into());
        m.insert("nav.audit_log".into(), "Audit Log".into());
        m.insert("nav.oauth2_sessions".into(), "OAuth2 Sessions".into());
        m.insert("nav.personal_tokens".into(), "Personal Tokens".into());
        m.insert("nav.upstream_providers".into(), "Upstream Providers".into());
        m.insert("nav.upstream_links".into(), "Upstream Links".into());
        m.insert("nav.connector_health".into(), "Connector Health".into());
        m.insert(
            "nav.notification_templates".into(),
            "Notification Templates".into(),
        );
        m.insert(
            "nav.notification_channels".into(),
            "Notification Channels".into(),
        );
        m.insert(
            "nav.notification_prefs".into(),
            "Notification Preferences".into(),
        );
        m.insert("nav.auth_status".into(), "Auth Status".into());
        m.insert("nav.appservices".into(), "Appservices".into());
        m.insert("nav.logout".into(), "Logout".into());

        // Auth Status page
        m.insert("auth_status.title".into(), "Authentication Status".into());
        m.insert(
            "auth_status.subtitle".into(),
            "Delegated authentication capabilities and login flow status".into(),
        );
        m.insert("auth_status.server_info".into(), "Server Connection".into());
        m.insert("auth_status.base_url".into(), "Base URL".into());
        m.insert("auth_status.server_version".into(), "Server Version".into());
        m.insert(
            "auth_status.auth_capabilities".into(),
            "Authentication Capabilities".into(),
        );
        m.insert(
            "auth_status.auth_capabilities_desc".into(),
            "Login methods supported by this homeserver based on /_matrix/client/v3/login".into(),
        );
        m.insert("auth_status.login_flows".into(), "Login Flows".into());
        m.insert(
            "auth_status.no_password_warning".into(),
            "Password login is not available on this server. Users must authenticate via SSO or access token.".into(),
        );
        m.insert(
            "auth_status.sso_only_hint".into(),
            "This server is configured for SSO-only authentication. Admin access requires an access token or SSO session.".into(),
        );
        m.insert(
            "auth_status.diagnostics_title".into(),
            "Delegated Auth Diagnostics".into(),
        );
        m.insert(
            "auth_status.diagnostics_desc".into(),
            "Checks for OIDC/MAS issuer discovery, endpoint availability, DCR support, and scope configuration".into(),
        );
        m.insert(
            "auth_status.dev_diagnostics_title".into(),
            "Dev Diagnostics".into(),
        );
        m.insert(
            "auth_status.dev_diagnostics_desc".into(),
            "Probe MAS/Pasion endpoints for registration, consent, and well-known discovery debugging".into(),
        );
        m.insert("nav.palpo_admin".into(), "Palpo Admin".into());
        m.insert("nav.server".into(), "Server".into());

        // Header
        m.insert("header.switch_light".into(), "Switch to light mode".into());
        m.insert("header.switch_dark".into(), "Switch to dark mode".into());
        m.insert("header.notifications".into(), "Notifications".into());
        m.insert("header.no_notifications".into(), "No notifications".into());
        m.insert("header.mark_all_read".into(), "Mark all as read".into());
        m.insert("header.clear_all".into(), "Clear all".into());
        m.insert("header.close".into(), "Close".into());

        // Dashboard descriptions
        m.insert("dashboard.server_online".into(), "Server Online".into());
        m.insert(
            "dashboard.total_registered_users".into(),
            "Total registered users".into(),
        );
        m.insert(
            "dashboard.total_rooms_on_server".into(),
            "Total rooms on server".into(),
        );
        m.insert("dashboard.pending_reports".into(), "Pending reports".into());
        m.insert(
            "dashboard.non_guest_non_deactivated".into(),
            "Non-guest, non-deactivated".into(),
        );
        m.insert(
            "dashboard.avg_api_response".into(),
            "Avg API response time".into(),
        );
        m.insert(
            "dashboard.spec_description".into(),
            "Supported Matrix specification versions and unstable features".into(),
        );

        // Auth extra
        m.insert(
            "auth.sign_in_subtitle".into(),
            "Sign in to manage your server".into(),
        );
        m.insert(
            "auth.footer".into(),
            "Palpo Admin - Matrix Server Management".into(),
        );
        m.insert(
            "auth.not_admin".into(),
            "This account is not a server administrator".into(),
        );
        m.insert(
            "auth.admin_check_failed".into(),
            "Failed to verify admin status".into(),
        );

        // Users
        m.insert("users.title".into(), "Users".into());
        m.insert("users.subtitle".into(), "Manage Matrix users".into());
        m.insert("users.tab_accounts".into(), "Local accounts".into());
        m.insert("users.tab_known".into(), "Seen users".into());
        m.insert(
            "users.known_subtitle".into(),
            "Every user this server has seen in its rooms, local or remote".into(),
        );
        m.insert("users.origin_all".into(), "All users".into());
        m.insert("users.origin_local".into(), "Local".into());
        m.insert("users.origin_remote".into(), "Remote".into());
        m.insert("users.server".into(), "Server".into());
        m.insert(
            "users.filter_by_server".into(),
            "Show only users from this server".into(),
        );
        m.insert("users.all_rooms".into(), "All rooms".into());
        m.insert(
            "users.last_membership_change".into(),
            "Last membership change".into(),
        );
        m.insert("users.banned".into(), "banned".into());
        m.insert(
            "users.known_empty".into(),
            "No users match these filters".into(),
        );
        m.insert("users.open_account".into(), "Open local account".into());
        m.insert("users.remote_note".into(), "This account lives on another homeserver and can only be managed there. Here you can only ban it from individual rooms on this server.".into());
        m.insert("users.seen_in_rooms".into(), "Rooms".into());
        m.insert(
            "users.seen_in_rooms_desc".into(),
            "Rooms where this server has recorded a membership for the user, most recent first"
                .into(),
        );
        m.insert("users.room".into(), "Room".into());
        m.insert("users.membership".into(), "Membership".into());
        m.insert("users.membership_sender".into(), "Changed by".into());
        m.insert("users.updated".into(), "Updated".into());
        m.insert("users.create".into(), "Create User".into());
        m.insert("users.user_id".into(), "User ID".into());
        m.insert("users.display_name".into(), "Display Name".into());
        m.insert("users.admin".into(), "Admin".into());
        m.insert("users.status".into(), "Status".into());
        m.insert("users.active".into(), "Active".into());
        m.insert("users.deactivated".into(), "Deactivated".into());
        m.insert("users.guest".into(), "Guest".into());
        m.insert("users.created".into(), "Created".into());
        m.insert("users.actions".into(), "Actions".into());
        m.insert("users.view_details".into(), "View Details".into());
        m.insert("users.deactivate".into(), "Deactivate".into());
        m.insert("users.reactivate".into(), "Reactivate".into());
        m.insert("users.delete".into(), "Delete".into());
        m.insert("users.suspend".into(), "Suspend".into());
        m.insert("users.erase".into(), "Erase (GDPR)".into());
        m.insert("users.send_notice".into(), "Send Server Notice".into());
        m.insert("users.search".into(), "Search users...".into());
        m.insert("users.edit".into(), "Edit User".into());
        m.insert("users.reset_password".into(), "Reset Password".into());
        m.insert("users.shadow_ban".into(), "Shadow Ban".into());
        m.insert("users.locked".into(), "Locked".into());
        m.insert("users.suspended".into(), "Suspended".into());
        m.insert("users.no_users".into(), "No users found".into());
        m.insert("users.export_csv".into(), "Export CSV".into());
        m.insert("users.import_csv".into(), "Import CSV".into());
        m.insert("users.create_new".into(), "Create New User".into());
        m.insert(
            "users.create_subtitle".into(),
            "Add a new user to the Matrix server".into(),
        );
        m.insert("users.password_confirm".into(), "Confirm Password".into());
        m.insert("users.admin_privileges".into(), "Admin Privileges".into());
        m.insert("users.user_type".into(), "User Type".into());
        m.insert("users.regular".into(), "Regular".into());
        m.insert("users.bot".into(), "Bot".into());
        m.insert("users.support".into(), "Support".into());
        m.insert("users.username".into(), "Username".into());
        m.insert("users.password".into(), "Password".into());
        m.insert("users.email_optional".into(), "Email (optional)".into());
        m.insert("users.phone_optional".into(), "Phone (optional)".into());
        m.insert(
            "users.deactivate_selected".into(),
            "Deactivate Selected".into(),
        );
        m.insert("users.set_admin".into(), "Set Admin".into());
        m.insert("users.exporting".into(), "Exporting...".into());
        m.insert("users.import_users".into(), "Import Users from CSV".into());
        m.insert("users.cancel_edit".into(), "Cancel Edit".into());
        m.insert("users.save_changes".into(), "Save Changes".into());
        m.insert("users.user_info".into(), "User Information".into());
        m.insert("users.account_details".into(), "Account Details".into());
        m.insert("users.third_party_ids".into(), "Third-party IDs".into());
        m.insert("users.overview".into(), "Overview".into());
        m.insert("users.rooms".into(), "Rooms".into());
        m.insert("users.devices".into(), "Devices".into());
        m.insert("users.sessions".into(), "Sessions".into());
        m.insert("users.account_data".into(), "Account Data".into());
        m.insert("users.rate_limits".into(), "Rate Limits".into());
        m.insert("users.features".into(), "Features".into());
        m.insert("users.joined_rooms".into(), "Joined Rooms".into());
        m.insert("users.shadow_banned".into(), "Shadow Banned".into());
        m.insert("users.deactivate_user".into(), "Deactivate User".into());
        m.insert("users.new_password".into(), "New Password".into());
        m.insert(
            "users.no_rooms".into(),
            "User has not joined any rooms.".into(),
        );
        m.insert("users.no_devices".into(), "No devices found.".into());
        m.insert(
            "users.no_sessions".into(),
            "No active sessions found.".into(),
        );
        m.insert(
            "users.revoke_all_sessions".into(),
            "Revoke All Sessions".into(),
        );
        m.insert("users.remove_threepid".into(), "Remove".into());
        m.insert("users.verified".into(), "Verified".into());
        m.insert("users.unverified".into(), "Unverified".into());
        m.insert("common.remove".into(), "Remove".into());
        m.insert("common.enable".into(), "Enable".into());
        m.insert("common.user".into(), "User".into());
        m.insert("common.rows".into(), "Rows:".into());

        // Rooms
        m.insert("rooms.title".into(), "Rooms".into());
        m.insert("rooms.name".into(), "Name".into());
        m.insert("rooms.alias".into(), "Alias".into());
        m.insert("rooms.members".into(), "Members".into());
        m.insert("rooms.visibility".into(), "Visibility".into());
        m.insert("rooms.public".into(), "Public".into());
        m.insert("rooms.private".into(), "Private".into());
        m.insert("rooms.join_rules".into(), "Join Rules".into());
        m.insert(
            "rooms.history_visibility".into(),
            "History Visibility".into(),
        );
        m.insert("rooms.encrypted".into(), "Encrypted".into());
        m.insert("rooms.topic".into(), "Topic".into());
        m.insert("rooms.delete".into(), "Delete Room".into());
        m.insert("rooms.block".into(), "Block Room".into());
        m.insert("rooms.unblock".into(), "Unblock Room".into());
        m.insert("rooms.purge_history".into(), "Purge History".into());
        m.insert("rooms.create".into(), "Create Room".into());
        m.insert("rooms.search".into(), "Search rooms...".into());
        m.insert("rooms.no_rooms".into(), "No rooms found".into());
        m.insert("rooms.messages".into(), "Messages".into());
        m.insert("rooms.state_events".into(), "State Events".into());
        m.insert("rooms.hierarchy".into(), "Hierarchy".into());
        m.insert("rooms.aliases".into(), "Aliases".into());
        m.insert("rooms.subtitle".into(), "Manage Matrix rooms".into());
        m.insert(
            "rooms.search_placeholder".into(),
            "Search rooms by name or alias...".into(),
        );
        m.insert("rooms.sort".into(), "Sort:".into());
        m.insert("rooms.all".into(), "All".into());
        m.insert(
            "rooms.no_rooms_description".into(),
            "There are no rooms matching your search criteria.".into(),
        );
        m.insert("rooms.delete_selected".into(), "Delete Selected".into());
        m.insert("rooms.block_selected".into(), "Block Selected".into());
        m.insert("rooms.overview".into(), "Overview".into());
        m.insert("rooms.room_information".into(), "Room Information".into());
        m.insert("rooms.room_settings".into(), "Room Settings".into());
        m.insert("rooms.room_id".into(), "Room ID".into());
        m.insert("rooms.canonical_alias".into(), "Canonical Alias".into());
        m.insert("rooms.room_type".into(), "Room Type".into());
        m.insert("rooms.space".into(), "Space".into());
        m.insert("rooms.blocked".into(), "Blocked".into());
        m.insert("rooms.other_aliases".into(), "Other Aliases".into());
        m.insert(
            "rooms.no_aliases".into(),
            "No additional aliases found.".into(),
        );
        m.insert("rooms.no_members".into(), "No members found.".into());
        m.insert("rooms.no_messages".into(), "No messages found.".into());
        m.insert("rooms.no_state_events".into(), "No state events.".into());
        m.insert("rooms.space_hierarchy".into(), "Space Hierarchy".into());
        m.insert(
            "rooms.space_hierarchy_desc".into(),
            "Child rooms and sub-spaces in this space.".into(),
        );
        m.insert("rooms.no_children".into(), "No child rooms found.".into());
        m.insert("rooms.room_name".into(), "Room Name *".into());
        m.insert(
            "rooms.topic_placeholder".into(),
            "Room topic (optional)".into(),
        );
        m.insert(
            "rooms.create_description".into(),
            "Create a new Matrix room.".into(),
        );
        m.insert("rooms.create_button".into(), "Create".into());
        m.insert("rooms.purge_title".into(), "Purge Room History".into());
        m.insert(
            "rooms.purge_description".into(),
            "Delete all messages before the specified date. This action cannot be undone.".into(),
        );
        m.insert("rooms.purge_before".into(), "Delete messages before".into());
        m.insert(
            "rooms.purge_warning".into(),
            "Warning: This will permanently delete all messages before the selected date.".into(),
        );
        m.insert(
            "rooms.delete_confirm".into(),
            "Are you sure you want to delete this room? All messages and media will be permanently removed. This action cannot be undone.".into(),
        );
        m.insert("rooms.created".into(), "Created".into());
        m.insert("rooms.kick".into(), "Kick".into());
        m.insert("rooms.unban".into(), "Unban".into());
        m.insert("rooms.invite".into(), "Invite".into());
        m.insert("rooms.promote".into(), "Make Admin".into());
        m.insert("rooms.invite_user_id".into(), "User ID to invite".into());
        m.insert("rooms.edit_room".into(), "Edit Room".into());
        m.insert("rooms.save_changes".into(), "Save Changes".into());
        m.insert("rooms.add_alias".into(), "Add Alias".into());
        m.insert("rooms.delete_alias".into(), "Delete".into());
        m.insert("rooms.new_alias".into(), "New alias".into());
        m.insert(
            "rooms.forward_extremities".into(),
            "Forward Extremities".into(),
        );
        m.insert("rooms.forward_extremities_desc".into(), "Forward extremities are the leaf events in the room DAG. Multiple extremities may indicate fragmentation.".into());
        m.insert(
            "rooms.no_forward_extremities".into(),
            "No forward extremities found.".into(),
        );
        m.insert("rooms.forward_extremities_warning".into(), "Warning: Multiple forward extremities detected. This may indicate DAG fragmentation and could impact performance.".into());
        m.insert("rooms.count".into(), "Count".into());
        m.insert("rooms.directory_listing".into(), "Directory Listing".into());
        m.insert("rooms.published".into(), "Published".into());
        m.insert("rooms.unpublished".into(), "Unpublished".into());
        m.insert("rooms.publish".into(), "Publish".into());
        m.insert("rooms.unpublish".into(), "Unpublish".into());
        m.insert("rooms.event_lookup".into(), "Event Lookup".into());
        m.insert(
            "rooms.event_lookup_desc".into(),
            "Look up any event by its event ID.".into(),
        );
        m.insert("rooms.lookup".into(), "Lookup".into());

        // Reports
        m.insert("reports.title".into(), "Reports".into());
        m.insert("reports.reporter".into(), "Reporter".into());
        m.insert("reports.reason".into(), "Reason".into());
        m.insert("reports.received".into(), "Received".into());
        m.insert("reports.status".into(), "Status".into());
        m.insert("reports.new".into(), "New".into());
        m.insert("reports.in_review".into(), "In Review".into());
        m.insert("reports.resolved".into(), "Resolved".into());
        m.insert(
            "reports.status_local_only".into(),
            "Stored only in this browser.".into(),
        );
        m.insert("reports.redact".into(), "Redact Event".into());
        m.insert("reports.ban_user".into(), "Ban User".into());
        m.insert("reports.block_room".into(), "Block Room".into());
        m.insert("reports.delete_report".into(), "Delete Report".into());
        m.insert(
            "reports.moderation_actions".into(),
            "Moderation Actions".into(),
        );
        m.insert("reports.no_reports".into(), "No reports".into());
        m.insert(
            "reports.subtitle".into(),
            "Event reports submitted by users".into(),
        );
        m.insert("reports.id".into(), "ID".into());
        m.insert("reports.room".into(), "Room".into());
        m.insert(
            "reports.no_reports_description".into(),
            "There are no event reports to review at this time.".into(),
        );
        m.insert("reports.report_details".into(), "Report Details".into());
        m.insert("reports.reporter_user_id".into(), "Reporter User ID".into());
        m.insert("reports.room_id".into(), "Room ID".into());
        m.insert("reports.event_id".into(), "Event ID".into());
        m.insert("reports.sender".into(), "Sender".into());
        m.insert("reports.score".into(), "Score".into());
        m.insert(
            "reports.event_content".into(),
            "Reported Event Content".into(),
        );
        m.insert(
            "reports.no_event_content".into(),
            "No event content available.".into(),
        );

        // Media
        m.insert("media.title".into(), "Media".into());
        m.insert("media.user_id".into(), "User ID".into());
        m.insert("media.media_count".into(), "Media Count".into());
        m.insert("media.total_size".into(), "Total Size".into());
        m.insert("media.actions".into(), "Actions".into());
        m.insert("media.quarantine".into(), "Quarantine".into());
        m.insert("media.delete_local".into(), "Delete Local Media".into());
        m.insert("media.purge_remote".into(), "Purge Remote Media".into());
        m.insert("media.search".into(), "Search users...".into());
        m.insert(
            "media.subtitle".into(),
            "Media usage statistics by user".into(),
        );
        m.insert("media.display_name".into(), "Display Name".into());
        m.insert("media.no_media".into(), "No media statistics found".into());

        // Server
        m.insert("server.version".into(), "Server Version".into());
        m.insert("server.features".into(), "Server Features".into());
        m.insert("server.status".into(), "Server Status".into());
        m.insert("server.online".into(), "Online".into());
        m.insert("server.healthy".into(), "Healthy".into());
        m.insert("server.issues_detected".into(), "Issues Detected".into());
        m.insert("server.maintenance_mode".into(), "Maintenance Mode".into());
        m.insert("server.actions".into(), "Server Actions".into());
        m.insert("server.notifications".into(), "Notifications".into());
        m.insert("server.clear_all".into(), "Clear All".into());
        m.insert("server.no_notifications".into(), "No notifications".into());
        m.insert(
            "server.command_running".into(),
            "A command is currently running".into(),
        );

        // Common
        m.insert("common.save".into(), "Save".into());
        m.insert("common.cancel".into(), "Cancel".into());
        m.insert("common.delete".into(), "Delete".into());
        m.insert("common.confirm".into(), "Confirm".into());
        m.insert("common.search".into(), "Search...".into());
        m.insert("common.loading".into(), "Loading...".into());
        m.insert("common.error".into(), "Error".into());
        m.insert("common.success".into(), "Success".into());
        m.insert("common.no_results".into(), "No results found".into());
        m.insert("common.yes".into(), "Yes".into());
        m.insert("common.no".into(), "No".into());
        m.insert("common.previous".into(), "Previous".into());
        m.insert("common.next".into(), "Next".into());
        m.insert("common.retry".into(), "Retry".into());
        m.insert("common.close".into(), "Close".into());
        m.insert("common.edit".into(), "Edit".into());
        m.insert("common.view".into(), "View".into());
        m.insert("common.selected".into(), "Selected".into());
        m.insert("common.clear_selection".into(), "Clear Selection".into());
        m.insert("common.columns".into(), "Columns".into());
        m.insert("common.refresh".into(), "Refresh".into());
        m.insert("common.load_more".into(), "Load More".into());
        m.insert("common.show".into(), "Show".into());
        m.insert("common.hide".into(), "Hide".into());
        m.insert("common.dismiss".into(), "Dismiss".into());
        m.insert("common.copy".into(), "Copy".into());
        m.insert("common.create".into(), "Create".into());
        m.insert("common.status".into(), "Status".into());
        m.insert("common.name".into(), "Name".into());
        m.insert("common.actions".into(), "Actions".into());
        m.insert("common.failed".into(), "Failed".into());

        // Pasion shared
        m.insert("pasion.status_active".into(), "Active".into());
        m.insert("pasion.status_revoked".into(), "Revoked".into());
        m.insert("pasion.status_healthy".into(), "Healthy".into());
        m.insert("pasion.status_degraded".into(), "Degraded".into());
        m.insert("pasion.status_unhealthy".into(), "Unhealthy".into());
        m.insert("pasion.status_down".into(), "Down".into());

        // Pasion audit log
        m.insert("pasion.audit_log.title".into(), "Audit Log".into());
        m.insert(
            "pasion.audit_log.description".into(),
            "Track administrative actions and changes".into(),
        );
        m.insert(
            "pasion.audit_log.filter_placeholder".into(),
            "Filter by operation...".into(),
        );
        m.insert(
            "pasion.audit_log.empty".into(),
            "No audit entries found".into(),
        );
        m.insert(
            "pasion.audit_log.load_more_failed".into(),
            "Failed to load more:".into(),
        );
        m.insert("pasion.audit_log.col_timestamp".into(), "Timestamp".into());
        m.insert("pasion.audit_log.col_operation".into(), "Operation".into());
        m.insert("pasion.audit_log.col_admin".into(), "Admin".into());
        m.insert("pasion.audit_log.col_resource".into(), "Resource".into());
        m.insert("pasion.audit_log.col_ip".into(), "IP Address".into());
        m.insert("pasion.audit_log.col_detail".into(), "Detail".into());

        // Pasion connector health
        m.insert(
            "pasion.connector_health.title".into(),
            "Connector Health".into(),
        );
        m.insert(
            "pasion.connector_health.description".into(),
            "Status of all backend service connectors".into(),
        );
        m.insert(
            "pasion.connector_health.empty".into(),
            "No connector health data available".into(),
        );
        m.insert(
            "pasion.connector_health.homeserver".into(),
            "Homeserver".into(),
        );

        // Pasion personal sessions
        m.insert(
            "pasion.personal_sessions.title".into(),
            "Personal Access Tokens".into(),
        );
        m.insert(
            "pasion.personal_sessions.description".into(),
            "Manage API tokens for personal access".into(),
        );
        m.insert(
            "pasion.personal_sessions.create".into(),
            "Create Token".into(),
        );
        m.insert(
            "pasion.personal_sessions.empty".into(),
            "No personal access tokens found".into(),
        );
        m.insert(
            "pasion.personal_sessions.new_token_warning".into(),
            "New token generated — copy it now, it will not be shown again!".into(),
        );
        m.insert(
            "pasion.personal_sessions.copy_success".into(),
            "Token copied to clipboard".into(),
        );
        m.insert(
            "pasion.personal_sessions.created_success".into(),
            "Personal access token created".into(),
        );
        m.insert(
            "pasion.personal_sessions.regenerated_success".into(),
            "Token regenerated".into(),
        );
        m.insert(
            "pasion.personal_sessions.revoked_success".into(),
            "Token revoked".into(),
        );
        m.insert(
            "pasion.personal_sessions.regenerate".into(),
            "Regenerate".into(),
        );
        m.insert("pasion.personal_sessions.revoke".into(), "Revoke".into());
        m.insert(
            "pasion.personal_sessions.create_title".into(),
            "Create Personal Access Token".into(),
        );
        m.insert(
            "pasion.personal_sessions.name_placeholder".into(),
            "Token name (optional)".into(),
        );
        m.insert(
            "pasion.personal_sessions.scope_label".into(),
            "Scope".into(),
        );
        m.insert(
            "pasion.personal_sessions.scope_placeholder".into(),
            "e.g. read write".into(),
        );
        m.insert(
            "pasion.personal_sessions.owner_label".into(),
            "Owner User ID (optional)".into(),
        );
        m.insert(
            "pasion.personal_sessions.owner_placeholder".into(),
            "User ID".into(),
        );
        m.insert(
            "pasion.personal_sessions.revoke_title".into(),
            "Revoke Token".into(),
        );
        m.insert(
            "pasion.personal_sessions.revoke_description".into(),
            "Are you sure you want to revoke this access token? This cannot be undone.".into(),
        );
        m.insert("pasion.personal_sessions.col_scope".into(), "Scope".into());
        m.insert("pasion.personal_sessions.col_owner".into(), "Owner".into());
        m.insert(
            "pasion.personal_sessions.col_created".into(),
            "Created".into(),
        );
        m.insert(
            "pasion.personal_sessions.col_last_active".into(),
            "Last Active".into(),
        );

        // Pasion OAuth2 sessions
        m.insert(
            "pasion.oauth2_sessions.title".into(),
            "OAuth2 Sessions".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.description".into(),
            "Browser and app OAuth2 sessions issued by Pasion".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.empty".into(),
            "No OAuth2 sessions found".into(),
        );
        m.insert("pasion.oauth2_sessions.finish".into(), "Finish".into());
        m.insert(
            "pasion.oauth2_sessions.finish_title".into(),
            "Finish Session".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.finish_description".into(),
            "End this OAuth2 session? The user will be signed out from the corresponding client."
                .into(),
        );
        m.insert(
            "pasion.oauth2_sessions.finished_success".into(),
            "Session finished".into(),
        );

        // Pasion upstream providers
        m.insert(
            "pasion.upstream_providers.title".into(),
            "Upstream Providers".into(),
        );
        m.insert(
            "pasion.upstream_providers.description".into(),
            "OIDC identity providers that Pasion federates with".into(),
        );
        m.insert(
            "pasion.upstream_providers.empty".into(),
            "No upstream providers configured".into(),
        );

        // Pasion local accounts
        m.insert("users.pasion_account".into(), "Identity account".into());
        m.insert("pasion.accounts.title".into(), "Local Accounts".into());
        m.insert(
            "pasion.accounts.description".into(),
            "Accounts managed by Pasion: roles, lock and deactivation, passwords and sessions"
                .into(),
        );
        m.insert("pasion.accounts.empty".into(), "No accounts found".into());
        m.insert("pasion.accounts.status_active".into(), "Active".into());
        m.insert("pasion.accounts.status_locked".into(), "Locked".into());
        m.insert(
            "pasion.accounts.status_deactivated".into(),
            "Deactivated".into(),
        );
        m.insert("pasion.accounts.admin".into(), "Admin".into());

        // Pasion upstream links
        m.insert(
            "pasion.upstream_links.title".into(),
            "Upstream Links".into(),
        );
        m.insert(
            "pasion.upstream_links.description".into(),
            "User-to-provider bindings for federated OAuth2 accounts".into(),
        );
        m.insert(
            "pasion.upstream_links.empty".into(),
            "No upstream links found".into(),
        );

        // Pasion notification channels
        m.insert(
            "pasion.notification_channels.title".into(),
            "Notification Channels".into(),
        );
        m.insert(
            "pasion.notification_channels.description".into(),
            "Status of configured notification delivery channels".into(),
        );
        m.insert(
            "pasion.notification_channels.empty".into(),
            "No notification channels configured".into(),
        );

        // Pasion notification templates
        m.insert(
            "pasion.notification_templates.title".into(),
            "Notification Templates".into(),
        );
        m.insert(
            "pasion.notification_templates.description".into(),
            "Email and SMS templates used for outgoing notifications".into(),
        );
        m.insert(
            "pasion.notification_templates.empty".into(),
            "No notification templates found".into(),
        );

        // Dashboard
        m.insert("dashboard.title".into(), "Dashboard".into());
        m.insert(
            "dashboard.welcome".into(),
            "Welcome to Hagency Server".into(),
        );
        m.insert("dashboard.total_users".into(), "Total Users".into());
        m.insert("dashboard.total_rooms".into(), "Total Rooms".into());
        m.insert("dashboard.total_reports".into(), "Pending Reports".into());
        m.insert("dashboard.active_users".into(), "Active Users".into());
        m.insert("dashboard.api_latency".into(), "API Latency".into());
        m.insert(
            "dashboard.supported_versions".into(),
            "Supported Versions".into(),
        );
        m.insert(
            "dashboard.unstable_features".into(),
            "Unstable Features".into(),
        );

        // Registration tokens
        m.insert(
            "registration_tokens.title".into(),
            "Registration Tokens".into(),
        );
        m.insert("registration_tokens.token".into(), "Token".into());
        m.insert(
            "registration_tokens.uses_allowed".into(),
            "Uses Allowed".into(),
        );
        m.insert("registration_tokens.pending".into(), "Pending".into());
        m.insert("registration_tokens.completed".into(), "Completed".into());
        m.insert("registration_tokens.expiry".into(), "Expiry".into());
        m.insert("registration_tokens.unlimited".into(), "Unlimited".into());
        m.insert("registration_tokens.never".into(), "Never".into());
        m.insert("registration_tokens.create".into(), "Create Token".into());
        m.insert(
            "registration_tokens.no_tokens".into(),
            "No registration tokens".into(),
        );
        m.insert(
            "registration_tokens.subtitle".into(),
            "Manage registration tokens for user sign-up".into(),
        );
        m.insert("registration_tokens.actions".into(), "Actions".into());
        m.insert(
            "registration_tokens.no_tokens_description".into(),
            "Create a token to allow new user registrations.".into(),
        );
        m.insert("registration_tokens.delete".into(), "Delete".into());
        m.insert(
            "registration_tokens.delete_token".into(),
            "Delete Token".into(),
        );
        m.insert(
            "registration_tokens.search_placeholder".into(),
            "Search tokens...".into(),
        );
        m.insert("registration_tokens.filter_all".into(), "All".into());
        m.insert("registration_tokens.filter_active".into(), "Active".into());
        m.insert(
            "registration_tokens.filter_expired".into(),
            "Expired".into(),
        );
        m.insert(
            "registration_tokens.create_description".into(),
            "Create a new registration token with optional constraints.".into(),
        );
        m.insert(
            "registration_tokens.custom_token".into(),
            "Custom Token Value".into(),
        );
        m.insert(
            "registration_tokens.custom_token_placeholder".into(),
            "Leave empty for auto-generated".into(),
        );
        m.insert(
            "registration_tokens.custom_token_hint".into(),
            "Leave empty to auto-generate a 16-character token.".into(),
        );

        // Destinations
        m.insert("destinations.title".into(), "Federation".into());
        m.insert("destinations.destination".into(), "Destination".into());
        m.insert(
            "destinations.retry_interval".into(),
            "Retry Interval".into(),
        );
        m.insert("destinations.last_failure".into(), "Last Failure".into());
        m.insert("destinations.reset".into(), "Reset Connection".into());
        m.insert(
            "destinations.search".into(),
            "Search destinations...".into(),
        );
        m.insert(
            "destinations.no_destinations".into(),
            "No federation destinations".into(),
        );
        m.insert(
            "destinations.subtitle".into(),
            "Remote server destinations".into(),
        );
        m.insert("destinations.status".into(), "Status".into());
        m.insert("destinations.last_retry".into(), "Last Retry".into());
        m.insert("destinations.actions".into(), "Actions".into());
        m.insert(
            "destinations.no_destinations_found".into(),
            "No destinations found".into(),
        );
        m.insert("destinations.reset_button".into(), "Reset".into());
        m.insert("destinations.failed".into(), "Failed".into());
        m.insert("destinations.ok".into(), "OK".into());
        m.insert(
            "destinations.detail_description".into(),
            "Federation destination details and retry status".into(),
        );
        m.insert(
            "destinations.connection_info".into(),
            "Connection Info".into(),
        );
        m.insert("destinations.retry_info".into(), "Retry Info".into());
        m.insert(
            "destinations.last_successful_stream".into(),
            "Last Successful Stream".into(),
        );
        m.insert("destinations.status".into(), "Status".into());

        // Server Notices
        m.insert("server_notices.title".into(), "Server Notices".into());
        m.insert(
            "server_notices.subtitle".into(),
            "Send server notices to users.".into(),
        );
        m.insert(
            "server_notices.send_title".into(),
            "Send Server Notice".into(),
        );
        m.insert(
            "server_notices.broadcast_desc".into(),
            "Broadcast an administrative notice to all users.".into(),
        );
        m.insert(
            "server_notices.single_desc".into(),
            "Send an administrative notice to a specific user.".into(),
        );
        m.insert("server_notices.single_user".into(), "Single User".into());
        m.insert("server_notices.broadcast".into(), "Broadcast to All".into());
        m.insert("server_notices.user_id".into(), "User ID".into());
        m.insert("server_notices.message".into(), "Message".into());
        m.insert(
            "server_notices.message_placeholder".into(),
            "Type your notice message...".into(),
        );
        m.insert(
            "server_notices.broadcast_notice".into(),
            "Broadcast Notice".into(),
        );
        m.insert("server_notices.send_notice".into(), "Send Notice".into());
        m.insert(
            "server_notices.history_title".into(),
            "Notice History".into(),
        );
        m.insert(
            "server_notices.history_desc".into(),
            "Previously sent server notices (stored locally).".into(),
        );
        m.insert(
            "server_notices.clear_history".into(),
            "Clear History".into(),
        );
        m.insert(
            "server_notices.no_notices".into(),
            "No notices sent yet.".into(),
        );
        m.insert("server_notices.user".into(), "User".into());
        m.insert("server_notices.timestamp".into(), "Timestamp".into());
        m.insert("server_notices.event_id".into(), "Event ID".into());

        // Scheduled / recurring commands
        m.insert(
            "commands.scheduled_title".into(),
            "Scheduled Commands".into(),
        );
        m.insert(
            "commands.recurring_title".into(),
            "Recurring Commands".into(),
        );
        m.insert("commands.create".into(), "Create".into());
        m.insert("commands.edit".into(), "Edit".into());
        m.insert("commands.delete".into(), "Delete".into());
        m.insert("commands.cancel".into(), "Cancel".into());
        m.insert("commands.save".into(), "Save".into());
        m.insert("commands.retry".into(), "Retry".into());
        m.insert(
            "commands.none_scheduled".into(),
            "No scheduled commands.".into(),
        );
        m.insert(
            "commands.none_recurring".into(),
            "No recurring commands.".into(),
        );
        m.insert(
            "commands.load_failed".into(),
            "Failed to load commands.".into(),
        );
        m.insert("commands.col_command".into(), "Command".into());
        m.insert("commands.col_arguments".into(), "Arguments".into());
        m.insert("commands.col_scheduled_at".into(), "Scheduled At".into());
        m.insert("commands.col_time".into(), "Time (UTC)".into());
        m.insert("commands.col_actions".into(), "Actions".into());
        m.insert("commands.field_command".into(), "Command".into());
        m.insert(
            "commands.field_command_placeholder".into(),
            "Command name".into(),
        );
        m.insert(
            "commands.field_arguments".into(),
            "Arguments (optional)".into(),
        );
        m.insert(
            "commands.field_arguments_placeholder".into(),
            "Arguments".into(),
        );
        m.insert(
            "commands.scheduled_at_label".into(),
            "Scheduled At (ISO 8601)".into(),
        );
        m.insert(
            "commands.scheduled_at_placeholder".into(),
            "2025-01-15T10:00:00Z".into(),
        );
        m.insert(
            "commands.time_label".into(),
            "Time (UTC, e.g. 03:00)".into(),
        );
        m.insert("commands.time_placeholder".into(), "HH:MM".into());
        m.insert(
            "commands.dialog_edit_scheduled".into(),
            "Edit Scheduled Command".into(),
        );
        m.insert("commands.dialog_schedule".into(), "Schedule Command".into());
        m.insert(
            "commands.dialog_edit_recurring".into(),
            "Edit Recurring Command".into(),
        );
        m.insert(
            "commands.dialog_create_recurring".into(),
            "Create Recurring Command".into(),
        );
        m.insert("commands.submit_schedule".into(), "Schedule".into());
        m.insert("commands.submit_create".into(), "Create".into());
        m.insert("commands.submit_save".into(), "Save".into());
        m.insert(
            "commands.delete_scheduled_title".into(),
            "Delete Scheduled Command".into(),
        );
        m.insert(
            "commands.delete_recurring_title".into(),
            "Delete Recurring Command".into(),
        );
        m.insert(
            "commands.delete_scheduled_desc".into(),
            "Are you sure you want to delete this scheduled command?".into(),
        );
        m.insert(
            "commands.delete_recurring_desc".into(),
            "Are you sure you want to delete this recurring command?".into(),
        );
        m.insert(
            "commands.validation_scheduled".into(),
            "Command and schedule time are required".into(),
        );
        m.insert(
            "commands.validation_recurring".into(),
            "Command and time are required".into(),
        );
        m.insert("commands.toast_deleted".into(), "Deleted".into());
        m.insert(
            "commands.toast_scheduled".into(),
            "Command scheduled".into(),
        );
        m.insert("commands.toast_updated".into(), "Command updated".into());
        m.insert(
            "commands.toast_recurring_created".into(),
            "Recurring command created".into(),
        );
        m.insert(
            "commands.toast_recurring_updated".into(),
            "Recurring command updated".into(),
        );

        // Not Found
        m.insert("not_found.title".into(), "404".into());
        m.insert("not_found.message".into(), "Page not found".into());
        m.insert("not_found.go_dashboard".into(), "Go to Dashboard".into());

        // Language
        m.insert("language.en".into(), "English".into());
        m.insert("language.zh_cn".into(), "\u{4e2d}\u{6587}".into());
        m.insert("language.select".into(), "Language".into());

        m
    }

    fn load_zh_cn() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("welcome.title".into(), "欢迎来到 Hagency".into());
        m.insert(
            "welcome.description".into(),
            "登录后，开始与 Agent 协作，管理项目和对话。".into(),
        );
        m.insert("welcome.sign_in".into(), "登录".into());
        m.insert("welcome.signing_in".into(), "正在跳转…".into());
        m.insert("welcome.continue".into(), "进入工作台".into());
        m.insert(
            "welcome.signed_in".into(),
            "你已登录，可以进入工作台。".into(),
        );
        m.insert("welcome.sign_in_hint".into(), "使用你的账号继续。".into());
        m.insert("welcome.loading".into(), "正在加载登录…".into());
        m.insert(
            "welcome.unavailable".into(),
            "此服务器暂未开放登录。".into(),
        );
        m.insert("welcome.create_account".into(), "创建账号".into());
        m.insert("welcome.account_links".into(), "账号选项".into());
        m.insert(
            "welcome.caption".into(),
            "让项目与 Agent 在同一个空间协作。".into(),
        );
        m.insert("welcome.theme".into(), "切换明暗主题".into());

        m.insert("home.title".into(), "首页".into());
        m.insert(
            "home.description".into(),
            "管理你的项目、Agent 和个人账号。".into(),
        );
        m.insert("home.projects".into(), "项目".into());
        m.insert("home.agents".into(), "我的 Agent".into());
        m.insert("home.account".into(), "账号中心".into());
        m.insert("home.admin".into(), "服务器管理".into());

        // Auth
        m.insert(
            "auth.welcome".into(),
            "\u{6b22}\u{8fce}\u{4f7f}\u{7528} {name}".into(),
        );
        m.insert(
            "auth.base_url".into(),
            "\u{670d}\u{52a1}\u{5668}\u{5730}\u{5740}".into(),
        );
        m.insert(
            "auth.credentials".into(),
            "\u{8d26}\u{53f7}\u{5bc6}\u{7801}".into(),
        );
        m.insert(
            "auth.access_token".into(),
            "\u{8bbf}\u{95ee}\u{4ee4}\u{724c}".into(),
        );
        m.insert("auth.username".into(), "\u{7528}\u{6237}\u{540d}".into());
        m.insert("auth.password".into(), "\u{5bc6}\u{7801}".into());
        m.insert("auth.sign_in".into(), "\u{767b}\u{5f55}".into());
        m.insert("auth.sign_out".into(), "\u{9000}\u{51fa}".into());
        m.insert(
            "auth.sso_sign_in".into(),
            "\u{4f7f}\u{7528} SSO \u{767b}\u{5f55}".into(),
        );
        m.insert(
            "auth.server_version".into(),
            "\u{670d}\u{52a1}\u{5668}\u{7248}\u{672c}\u{ff1a}".into(),
        );
        m.insert(
            "auth.supports_specs".into(),
            "\u{652f}\u{6301}\u{89c4}\u{8303}\u{ff1a}".into(),
        );
        m.insert(
            "auth.protocol_error".into(),
            "URL \u{5fc5}\u{987b}\u{4ee5} http:// \u{6216} https:// \u{5f00}\u{5934}".into(),
        );
        m.insert(
            "auth.url_error".into(),
            "\u{4e0d}\u{662f}\u{6709}\u{6548}\u{7684}\u{670d}\u{52a1}\u{5668}\u{5730}\u{5740}"
                .into(),
        );
        m.insert(
            "auth.fields_required".into(),
            "\u{8bf7}\u{8f93}\u{5165}\u{7528}\u{6237}\u{540d}\u{548c}\u{5bc6}\u{7801}".into(),
        );
        m.insert(
            "auth.invalid_credentials".into(),
            "\u{7528}\u{6237}\u{540d}\u{6216}\u{5bc6}\u{7801}\u{9519}\u{8bef}".into(),
        );
        m.insert("auth.rate_limited".into(), "\u{5c1d}\u{8bd5}\u{6b21}\u{6570}\u{8fc7}\u{591a}\u{ff0c}\u{8bf7}\u{7a0d}\u{540e}\u{518d}\u{8bd5}".into());
        m.insert(
            "auth.account_deactivated".into(),
            "\u{8be5}\u{8d26}\u{6237}\u{5df2}\u{88ab}\u{505c}\u{7528}".into(),
        );
        m.insert(
            "auth.account_locked".into(),
            "\u{8be5}\u{8d26}\u{6237}\u{5df2}\u{88ab}\u{9501}\u{5b9a}".into(),
        );
        m.insert(
            "auth.username_placeholder".into(),
            "\u{7528}\u{6237}\u{540d}\u{6216}\u{90ae}\u{7bb1}".into(),
        );
        m.insert("auth.oauth_hint".into(), "\u{767b}\u{5f55}\u{60a8}\u{7684}\u{8d26}\u{6237}\u{4ee5}\u{8bbf}\u{95ee}\u{7ba1}\u{7406}\u{540e}\u{53f0}".into());
        m.insert(
            "auth.processing".into(),
            "\u{6b63}\u{5728}\u{767b}\u{5f55}...".into(),
        );
        m.insert("auth.try_again".into(), "\u{91cd}\u{8bd5}".into());

        // Navigation
        m.insert("nav.dashboard".into(), "\u{4eea}\u{8868}\u{76d8}".into());
        m.insert("nav.users".into(), "\u{7528}\u{6237}".into());
        m.insert("nav.rooms".into(), "\u{623f}\u{95f4}".into());
        m.insert("nav.media".into(), "\u{5a92}\u{4f53}".into());
        m.insert("nav.reports".into(), "\u{4e3e}\u{62a5}".into());
        m.insert("nav.federation".into(), "\u{8054}\u{90a6}".into());
        m.insert(
            "nav.registration_tokens".into(),
            "\u{6ce8}\u{518c}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "nav.server_status".into(),
            "\u{670d}\u{52a1}\u{5668}\u{72b6}\u{6001}".into(),
        );
        m.insert(
            "nav.server_actions".into(),
            "\u{670d}\u{52a1}\u{5668}\u{64cd}\u{4f5c}".into(),
        );
        m.insert("nav.notifications".into(), "\u{901a}\u{77e5}".into());
        m.insert(
            "nav.server_notices".into(),
            "\u{670d}\u{52a1}\u{5668}\u{901a}\u{77e5}".into(),
        );
        m.insert("nav.management".into(), "\u{7ba1}\u{7406}".into());
        m.insert("nav.section_identity".into(), "\u{8eab}\u{4efd}".into());
        m.insert("nav.section_moderation".into(), "\u{5ba1}\u{6838}".into());
        m.insert(
            "nav.section_infrastructure".into(),
            "\u{57fa}\u{7840}\u{8bbe}\u{65bd}".into(),
        );
        m.insert(
            "nav.section_server_ops".into(),
            "\u{670d}\u{52a1}\u{5668}\u{64cd}\u{4f5c}".into(),
        );
        m.insert(
            "nav.section_pasion".into(),
            "\u{8eab}\u{4efd}\u{63d0}\u{4f9b}\u{8005}".into(),
        );
        m.insert(
            "nav.pasion_accounts".into(),
            "\u{672c}\u{5730}\u{8d26}\u{53f7}".into(),
        );
        m.insert(
            "nav.audit_log".into(),
            "\u{5ba1}\u{8ba1}\u{65e5}\u{5fd7}".into(),
        );
        m.insert(
            "nav.oauth2_sessions".into(),
            "OAuth2 \u{4f1a}\u{8bdd}".into(),
        );
        m.insert(
            "nav.personal_tokens".into(),
            "\u{4e2a}\u{4eba}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "nav.upstream_providers".into(),
            "\u{4e0a}\u{6e38}\u{63d0}\u{4f9b}\u{8005}".into(),
        );
        m.insert(
            "nav.upstream_links".into(),
            "\u{4e0a}\u{6e38}\u{94fe}\u{63a5}".into(),
        );
        m.insert(
            "nav.connector_health".into(),
            "\u{8fde}\u{63a5}\u{5668}\u{5065}\u{5eb7}".into(),
        );
        m.insert(
            "nav.notification_templates".into(),
            "\u{901a}\u{77e5}\u{6a21}\u{677f}".into(),
        );
        m.insert(
            "nav.notification_channels".into(),
            "\u{901a}\u{77e5}\u{6e20}\u{9053}".into(),
        );
        m.insert(
            "nav.notification_prefs".into(),
            "\u{901a}\u{77e5}\u{8bbe}\u{7f6e}".into(),
        );
        m.insert(
            "nav.auth_status".into(),
            "\u{8ba4}\u{8bc1}\u{72b6}\u{6001}".into(),
        );
        // 应用服务
        m.insert(
            "nav.appservices".into(),
            "\u{5e94}\u{7528}\u{670d}\u{52a1}".into(),
        );
        m.insert("nav.logout".into(), "\u{9000}\u{51fa}".into());

        // Auth Status page
        m.insert(
            "auth_status.title".into(),
            "\u{8ba4}\u{8bc1}\u{72b6}\u{6001}".into(),
        );
        m.insert("auth_status.subtitle".into(), "\u{59d4}\u{6258}\u{8ba4}\u{8bc1}\u{80fd}\u{529b}\u{548c}\u{767b}\u{5f55}\u{6d41}\u{72b6}\u{6001}".into());
        m.insert(
            "auth_status.server_info".into(),
            "\u{670d}\u{52a1}\u{5668}\u{8fde}\u{63a5}".into(),
        );
        m.insert("auth_status.base_url".into(), "\u{57fa}\u{7840} URL".into());
        m.insert(
            "auth_status.server_version".into(),
            "\u{670d}\u{52a1}\u{5668}\u{7248}\u{672c}".into(),
        );
        m.insert(
            "auth_status.auth_capabilities".into(),
            "\u{8ba4}\u{8bc1}\u{80fd}\u{529b}".into(),
        );
        m.insert("auth_status.auth_capabilities_desc".into(), "\u{6b64}\u{670d}\u{52a1}\u{5668}\u{652f}\u{6301}\u{7684}\u{767b}\u{5f55}\u{65b9}\u{5f0f}".into());
        m.insert(
            "auth_status.login_flows".into(),
            "\u{767b}\u{5f55}\u{6d41}".into(),
        );
        m.insert("auth_status.no_password_warning".into(), "\u{6b64}\u{670d}\u{52a1}\u{5668}\u{4e0d}\u{652f}\u{6301}\u{5bc6}\u{7801}\u{767b}\u{5f55}\u{3002}\u{7528}\u{6237}\u{5fc5}\u{987b}\u{901a}\u{8fc7} SSO \u{6216}\u{8bbf}\u{95ee}\u{4ee4}\u{724c}\u{8fdb}\u{884c}\u{8eab}\u{4efd}\u{9a8c}\u{8bc1}\u{3002}".into());
        m.insert("auth_status.sso_only_hint".into(), "\u{6b64}\u{670d}\u{52a1}\u{5668}\u{914d}\u{7f6e}\u{4e3a}\u{4ec5} SSO \u{8ba4}\u{8bc1}\u{3002}\u{7ba1}\u{7406}\u{5458}\u{8bbf}\u{95ee}\u{9700}\u{8981}\u{8bbf}\u{95ee}\u{4ee4}\u{724c}\u{6216} SSO \u{4f1a}\u{8bdd}\u{3002}".into());
        m.insert(
            "auth_status.diagnostics_title".into(),
            "\u{59d4}\u{6258}\u{8ba4}\u{8bc1}\u{8bca}\u{65ad}".into(),
        );
        m.insert("auth_status.diagnostics_desc".into(), "\u{68c0}\u{67e5} OIDC/MAS \u{53d1}\u{884c}\u{8005}\u{53d1}\u{73b0}\u{3001}\u{7aef}\u{70b9}\u{53ef}\u{7528}\u{6027}\u{3001}DCR \u{652f}\u{6301}\u{548c}\u{4f5c}\u{7528}\u{57df}\u{914d}\u{7f6e}".into());
        m.insert(
            "auth_status.dev_diagnostics_title".into(),
            "\u{5f00}\u{53d1}\u{8bca}\u{65ad}".into(),
        );
        m.insert("auth_status.dev_diagnostics_desc".into(), "\u{63a2}\u{6d4b} MAS/Pasion \u{7aef}\u{70b9}\u{4ee5}\u{8c03}\u{8bd5}\u{6ce8}\u{518c}\u{3001}\u{540c}\u{610f}\u{548c} well-known \u{53d1}\u{73b0}".into());
        m.insert("nav.palpo_admin".into(), "Palpo Admin".into());
        m.insert("nav.server".into(), "\u{670d}\u{52a1}\u{5668}".into());

        // Header
        m.insert(
            "header.switch_light".into(),
            "\u{5207}\u{6362}\u{5230}\u{6d45}\u{8272}\u{6a21}\u{5f0f}".into(),
        );
        m.insert(
            "header.switch_dark".into(),
            "\u{5207}\u{6362}\u{5230}\u{6df1}\u{8272}\u{6a21}\u{5f0f}".into(),
        );
        m.insert("header.notifications".into(), "\u{901a}\u{77e5}".into());
        m.insert(
            "header.no_notifications".into(),
            "\u{6682}\u{65e0}\u{901a}\u{77e5}".into(),
        );
        m.insert(
            "header.mark_all_read".into(),
            "\u{5168}\u{90e8}\u{6807}\u{8bb0}\u{5df2}\u{8bfb}".into(),
        );
        m.insert(
            "header.clear_all".into(),
            "\u{6e05}\u{9664}\u{5168}\u{90e8}".into(),
        );
        m.insert("header.close".into(), "\u{5173}\u{95ed}".into());

        // Dashboard descriptions
        m.insert(
            "dashboard.server_online".into(),
            "\u{670d}\u{52a1}\u{5668}\u{5728}\u{7ebf}".into(),
        );
        m.insert(
            "dashboard.total_registered_users".into(),
            "\u{6ce8}\u{518c}\u{7528}\u{6237}\u{603b}\u{6570}".into(),
        );
        m.insert(
            "dashboard.total_rooms_on_server".into(),
            "\u{670d}\u{52a1}\u{5668}\u{623f}\u{95f4}\u{603b}\u{6570}".into(),
        );
        m.insert(
            "dashboard.pending_reports".into(),
            "\u{5f85}\u{5904}\u{7406}\u{4e3e}\u{62a5}".into(),
        );
        m.insert(
            "dashboard.non_guest_non_deactivated".into(),
            "\u{975e}\u{8bbf}\u{5ba2}\u{3001}\u{975e}\u{505c}\u{7528}".into(),
        );
        m.insert(
            "dashboard.avg_api_response".into(),
            "\u{5e73}\u{5747} API \u{54cd}\u{5e94}\u{65f6}\u{95f4}".into(),
        );
        m.insert("dashboard.spec_description".into(), "\u{652f}\u{6301}\u{7684} Matrix \u{89c4}\u{8303}\u{7248}\u{672c}\u{548c}\u{5b9e}\u{9a8c}\u{6027}\u{529f}\u{80fd}".into());

        // Auth extra
        m.insert(
            "auth.sign_in_subtitle".into(),
            "\u{767b}\u{5f55}\u{4ee5}\u{7ba1}\u{7406}\u{60a8}\u{7684}\u{670d}\u{52a1}\u{5668}"
                .into(),
        );
        m.insert(
            "auth.footer".into(),
            "Palpo Admin - Matrix \u{670d}\u{52a1}\u{5668}\u{7ba1}\u{7406}".into(),
        );
        m.insert(
            "auth.not_admin".into(),
            "\u{8be5}\u{8d26}\u{6237}\u{4e0d}\u{662f}\u{670d}\u{52a1}\u{5668}\u{7ba1}\u{7406}\u{5458}".into(),
        );
        m.insert(
            "auth.admin_check_failed".into(),
            "\u{9a8c}\u{8bc1}\u{7ba1}\u{7406}\u{5458}\u{72b6}\u{6001}\u{5931}\u{8d25}".into(),
        );

        // Users
        m.insert(
            "users.title".into(),
            "\u{7528}\u{6237}\u{7ba1}\u{7406}".into(),
        );
        m.insert(
            "users.subtitle".into(),
            "\u{7ba1}\u{7406} Matrix \u{7528}\u{6237}".into(),
        );
        m.insert(
            "users.tab_accounts".into(),
            "\u{672c}\u{5730}\u{8d26}\u{53f7}".into(),
        );
        m.insert(
            "users.tab_known".into(),
            "\u{5df2}\u{89c1}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "users.known_subtitle".into(),
            "\u{672c}\u{670d}\u{52a1}\u{5668}\u{5728}\u{623f}\u{95f4}\u{4e2d}\u{89c1}\u{8fc7}\u{7684}\u{6240}\u{6709}\u{7528}\u{6237}\u{ff08}\u{542b}\u{672c}\u{5730}\u{548c}\u{8fdc}\u{7a0b}\u{ff09}".into(),
        );
        m.insert(
            "users.origin_all".into(),
            "\u{5168}\u{90e8}\u{7528}\u{6237}".into(),
        );
        m.insert("users.origin_local".into(), "\u{672c}\u{5730}".into());
        m.insert("users.origin_remote".into(), "\u{8fdc}\u{7a0b}".into());
        m.insert("users.server".into(), "\u{670d}\u{52a1}\u{5668}".into());
        m.insert(
            "users.filter_by_server".into(),
            "\u{53ea}\u{663e}\u{793a}\u{8be5}\u{670d}\u{52a1}\u{5668}\u{7684}\u{7528}\u{6237}"
                .into(),
        );
        m.insert(
            "users.all_rooms".into(),
            "\u{5168}\u{90e8}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "users.last_membership_change".into(),
            "\u{6700}\u{8fd1}\u{6210}\u{5458}\u{53d8}\u{66f4}".into(),
        );
        m.insert("users.banned".into(), "\u{5df2}\u{5c01}\u{7981}".into());
        m.insert(
            "users.known_empty".into(),
            "\u{6ca1}\u{6709}\u{7b26}\u{5408}\u{7b5b}\u{9009}\u{6761}\u{4ef6}\u{7684}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "users.open_account".into(),
            "\u{6253}\u{5f00}\u{672c}\u{5730}\u{8d26}\u{53f7}".into(),
        );
        m.insert(
            "users.remote_note".into(),
            "\u{8be5}\u{8d26}\u{53f7}\u{5c5e}\u{4e8e}\u{5176}\u{4ed6}\u{670d}\u{52a1}\u{5668}\u{ff0c}\u{53ea}\u{80fd}\u{7531}\u{90a3}\u{53f0}\u{670d}\u{52a1}\u{5668}\u{7ba1}\u{7406}\u{3002}\u{5728}\u{8fd9}\u{91cc}\u{53ea}\u{80fd}\u{5728}\u{672c}\u{670d}\u{52a1}\u{5668}\u{7684}\u{5404}\u{4e2a}\u{623f}\u{95f4}\u{4e2d}\u{5c01}\u{7981}\u{8be5}\u{7528}\u{6237}\u{3002}".into(),
        );
        m.insert("users.seen_in_rooms".into(), "\u{623f}\u{95f4}".into());
        m.insert(
            "users.seen_in_rooms_desc".into(),
            "\u{672c}\u{670d}\u{52a1}\u{5668}\u{8bb0}\u{5f55}\u{8fc7}\u{8be5}\u{7528}\u{6237}\u{6210}\u{5458}\u{5173}\u{7cfb}\u{7684}\u{623f}\u{95f4}\u{ff0c}\u{6309}\u{6700}\u{8fd1}\u{53d8}\u{66f4}\u{6392}\u{5e8f}".into(),
        );
        m.insert("users.room".into(), "\u{623f}\u{95f4}".into());
        m.insert(
            "users.membership".into(),
            "\u{6210}\u{5458}\u{72b6}\u{6001}".into(),
        );
        m.insert(
            "users.membership_sender".into(),
            "\u{64cd}\u{4f5c}\u{8005}".into(),
        );
        m.insert(
            "users.updated".into(),
            "\u{66f4}\u{65b0}\u{65f6}\u{95f4}".into(),
        );
        m.insert(
            "users.create".into(),
            "\u{521b}\u{5efa}\u{7528}\u{6237}".into(),
        );
        m.insert("users.user_id".into(), "\u{7528}\u{6237} ID".into());
        m.insert(
            "users.display_name".into(),
            "\u{663e}\u{793a}\u{540d}\u{79f0}".into(),
        );
        m.insert("users.admin".into(), "\u{7ba1}\u{7406}\u{5458}".into());
        m.insert("users.status".into(), "\u{72b6}\u{6001}".into());
        m.insert("users.active".into(), "\u{6d3b}\u{8dc3}".into());
        m.insert(
            "users.deactivated".into(),
            "\u{5df2}\u{505c}\u{7528}".into(),
        );
        m.insert("users.guest".into(), "\u{8bbf}\u{5ba2}".into());
        m.insert(
            "users.created".into(),
            "\u{521b}\u{5efa}\u{65f6}\u{95f4}".into(),
        );
        m.insert("users.actions".into(), "\u{64cd}\u{4f5c}".into());
        m.insert(
            "users.view_details".into(),
            "\u{67e5}\u{770b}\u{8be6}\u{60c5}".into(),
        );
        m.insert("users.deactivate".into(), "\u{505c}\u{7528}".into());
        m.insert(
            "users.reactivate".into(),
            "\u{91cd}\u{65b0}\u{6fc0}\u{6d3b}".into(),
        );
        m.insert("users.delete".into(), "\u{5220}\u{9664}".into());
        m.insert("users.suspend".into(), "\u{6682}\u{505c}".into());
        m.insert("users.erase".into(), "\u{64e6}\u{9664} (GDPR)".into());
        m.insert(
            "users.send_notice".into(),
            "\u{53d1}\u{9001}\u{670d}\u{52a1}\u{5668}\u{901a}\u{77e5}".into(),
        );
        m.insert(
            "users.search".into(),
            "\u{641c}\u{7d22}\u{7528}\u{6237}...".into(),
        );
        m.insert(
            "users.edit".into(),
            "\u{7f16}\u{8f91}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "users.reset_password".into(),
            "\u{91cd}\u{7f6e}\u{5bc6}\u{7801}".into(),
        );
        m.insert(
            "users.shadow_ban".into(),
            "\u{5f71}\u{5b50}\u{5c01}\u{7981}".into(),
        );
        m.insert("users.locked".into(), "\u{5df2}\u{9501}\u{5b9a}".into());
        m.insert("users.suspended".into(), "\u{5df2}\u{6682}\u{505c}".into());
        m.insert(
            "users.no_users".into(),
            "\u{672a}\u{627e}\u{5230}\u{7528}\u{6237}".into(),
        );
        m.insert("users.export_csv".into(), "\u{5bfc}\u{51fa} CSV".into());
        m.insert("users.import_csv".into(), "\u{5bfc}\u{5165} CSV".into());
        m.insert(
            "users.create_new".into(),
            "\u{521b}\u{5efa}\u{65b0}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "users.create_subtitle".into(),
            "\u{5411} Matrix \u{670d}\u{52a1}\u{5668}\u{6dfb}\u{52a0}\u{65b0}\u{7528}\u{6237}"
                .into(),
        );
        m.insert(
            "users.password_confirm".into(),
            "\u{786e}\u{8ba4}\u{5bc6}\u{7801}".into(),
        );
        m.insert(
            "users.admin_privileges".into(),
            "\u{7ba1}\u{7406}\u{5458}\u{6743}\u{9650}".into(),
        );
        m.insert(
            "users.user_type".into(),
            "\u{7528}\u{6237}\u{7c7b}\u{578b}".into(),
        );
        m.insert("users.regular".into(), "\u{666e}\u{901a}".into());
        m.insert("users.bot".into(), "\u{673a}\u{5668}\u{4eba}".into());
        m.insert("users.support".into(), "\u{5ba2}\u{670d}".into());
        m.insert("users.username".into(), "\u{7528}\u{6237}\u{540d}".into());
        m.insert("users.password".into(), "\u{5bc6}\u{7801}".into());
        m.insert(
            "users.email_optional".into(),
            "\u{90ae}\u{7bb1}\u{ff08}\u{53ef}\u{9009}\u{ff09}".into(),
        );
        m.insert(
            "users.phone_optional".into(),
            "\u{7535}\u{8bdd}\u{ff08}\u{53ef}\u{9009}\u{ff09}".into(),
        );
        m.insert(
            "users.deactivate_selected".into(),
            "\u{505c}\u{7528}\u{6240}\u{9009}".into(),
        );
        m.insert(
            "users.set_admin".into(),
            "\u{8bbe}\u{4e3a}\u{7ba1}\u{7406}\u{5458}".into(),
        );
        m.insert(
            "users.exporting".into(),
            "\u{5bfc}\u{51fa}\u{4e2d}...".into(),
        );
        m.insert(
            "users.import_users".into(),
            "\u{4ece} CSV \u{5bfc}\u{5165}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "users.cancel_edit".into(),
            "\u{53d6}\u{6d88}\u{7f16}\u{8f91}".into(),
        );
        m.insert(
            "users.save_changes".into(),
            "\u{4fdd}\u{5b58}\u{66f4}\u{6539}".into(),
        );
        m.insert(
            "users.user_info".into(),
            "\u{7528}\u{6237}\u{4fe1}\u{606f}".into(),
        );
        m.insert(
            "users.account_details".into(),
            "\u{8d26}\u{6237}\u{8be6}\u{60c5}".into(),
        );
        m.insert(
            "users.third_party_ids".into(),
            "\u{7b2c}\u{4e09}\u{65b9} ID".into(),
        );
        m.insert("users.overview".into(), "\u{6982}\u{89c8}".into());
        m.insert("users.rooms".into(), "\u{623f}\u{95f4}".into());
        m.insert("users.devices".into(), "\u{8bbe}\u{5907}".into());
        m.insert("users.sessions".into(), "\u{4f1a}\u{8bdd}".into());
        m.insert(
            "users.account_data".into(),
            "\u{8d26}\u{6237}\u{6570}\u{636e}".into(),
        );
        m.insert(
            "users.rate_limits".into(),
            "\u{901f}\u{7387}\u{9650}\u{5236}".into(),
        );
        m.insert("users.features".into(), "\u{529f}\u{80fd}".into());
        m.insert(
            "users.joined_rooms".into(),
            "\u{5df2}\u{52a0}\u{5165}\u{7684}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "users.shadow_banned".into(),
            "\u{5f71}\u{5b50}\u{5c01}\u{7981}".into(),
        );
        m.insert(
            "users.deactivate_user".into(),
            "\u{505c}\u{7528}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "users.new_password".into(),
            "\u{65b0}\u{5bc6}\u{7801}".into(),
        );
        m.insert(
            "users.no_rooms".into(),
            "\u{7528}\u{6237}\u{672a}\u{52a0}\u{5165}\u{4efb}\u{4f55}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "users.no_devices".into(),
            "\u{672a}\u{627e}\u{5230}\u{8bbe}\u{5907}".into(),
        );
        m.insert(
            "users.no_sessions".into(),
            "\u{672a}\u{627e}\u{5230}\u{6d3b}\u{8dc3}\u{4f1a}\u{8bdd}".into(),
        );
        m.insert(
            "users.revoke_all_sessions".into(),
            "\u{64a4}\u{9500}\u{6240}\u{6709}\u{4f1a}\u{8bdd}".into(),
        );
        m.insert("users.remove_threepid".into(), "\u{79fb}\u{9664}".into());
        m.insert("users.verified".into(), "\u{5df2}\u{9a8c}\u{8bc1}".into());
        m.insert("users.unverified".into(), "\u{672a}\u{9a8c}\u{8bc1}".into());
        m.insert("common.remove".into(), "\u{79fb}\u{9664}".into());
        m.insert("common.enable".into(), "\u{542f}\u{7528}".into());
        m.insert("common.user".into(), "\u{7528}\u{6237}".into());
        m.insert("common.rows".into(), "\u{884c}\u{6570}\u{ff1a}".into());

        // Rooms
        m.insert(
            "rooms.title".into(),
            "\u{623f}\u{95f4}\u{7ba1}\u{7406}".into(),
        );
        m.insert("rooms.name".into(), "\u{540d}\u{79f0}".into());
        m.insert("rooms.alias".into(), "\u{522b}\u{540d}".into());
        m.insert("rooms.members".into(), "\u{6210}\u{5458}".into());
        m.insert("rooms.visibility".into(), "\u{53ef}\u{89c1}\u{6027}".into());
        m.insert("rooms.public".into(), "\u{516c}\u{5f00}".into());
        m.insert("rooms.private".into(), "\u{79c1}\u{5bc6}".into());
        m.insert(
            "rooms.join_rules".into(),
            "\u{52a0}\u{5165}\u{89c4}\u{5219}".into(),
        );
        m.insert(
            "rooms.history_visibility".into(),
            "\u{5386}\u{53f2}\u{53ef}\u{89c1}\u{6027}".into(),
        );
        m.insert("rooms.encrypted".into(), "\u{5df2}\u{52a0}\u{5bc6}".into());
        m.insert("rooms.topic".into(), "\u{8bdd}\u{9898}".into());
        m.insert(
            "rooms.delete".into(),
            "\u{5220}\u{9664}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "rooms.block".into(),
            "\u{5c01}\u{9501}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "rooms.unblock".into(),
            "\u{89e3}\u{9664}\u{5c01}\u{9501}".into(),
        );
        m.insert(
            "rooms.purge_history".into(),
            "\u{6e05}\u{9664}\u{5386}\u{53f2}".into(),
        );
        m.insert(
            "rooms.create".into(),
            "\u{521b}\u{5efa}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "rooms.search".into(),
            "\u{641c}\u{7d22}\u{623f}\u{95f4}...".into(),
        );
        m.insert(
            "rooms.no_rooms".into(),
            "\u{672a}\u{627e}\u{5230}\u{623f}\u{95f4}".into(),
        );
        m.insert("rooms.messages".into(), "\u{6d88}\u{606f}".into());
        m.insert(
            "rooms.state_events".into(),
            "\u{72b6}\u{6001}\u{4e8b}\u{4ef6}".into(),
        );
        m.insert(
            "rooms.hierarchy".into(),
            "\u{5c42}\u{7ea7}\u{7ed3}\u{6784}".into(),
        );
        m.insert(
            "rooms.aliases".into(),
            "\u{522b}\u{540d}\u{5217}\u{8868}".into(),
        );
        m.insert(
            "rooms.subtitle".into(),
            "\u{7ba1}\u{7406} Matrix \u{623f}\u{95f4}".into(),
        );
        m.insert(
            "rooms.search_placeholder".into(),
            "\u{6309}\u{540d}\u{79f0}\u{6216}\u{522b}\u{540d}\u{641c}\u{7d22}\u{623f}\u{95f4}..."
                .into(),
        );
        m.insert("rooms.sort".into(), "\u{6392}\u{5e8f}\u{ff1a}".into());
        m.insert("rooms.all".into(), "\u{5168}\u{90e8}".into());
        m.insert("rooms.no_rooms_description".into(), "\u{6ca1}\u{6709}\u{7b26}\u{5408}\u{641c}\u{7d22}\u{6761}\u{4ef6}\u{7684}\u{623f}\u{95f4}".into());
        m.insert(
            "rooms.delete_selected".into(),
            "\u{5220}\u{9664}\u{6240}\u{9009}".into(),
        );
        m.insert(
            "rooms.block_selected".into(),
            "\u{5c01}\u{9501}\u{6240}\u{9009}".into(),
        );
        m.insert("rooms.overview".into(), "\u{6982}\u{89c8}".into());
        m.insert(
            "rooms.room_information".into(),
            "\u{623f}\u{95f4}\u{4fe1}\u{606f}".into(),
        );
        m.insert(
            "rooms.room_settings".into(),
            "\u{623f}\u{95f4}\u{8bbe}\u{7f6e}".into(),
        );
        m.insert("rooms.room_id".into(), "\u{623f}\u{95f4} ID".into());
        m.insert(
            "rooms.canonical_alias".into(),
            "\u{89c4}\u{8303}\u{522b}\u{540d}".into(),
        );
        m.insert(
            "rooms.room_type".into(),
            "\u{623f}\u{95f4}\u{7c7b}\u{578b}".into(),
        );
        m.insert("rooms.space".into(), "\u{7a7a}\u{95f4}".into());
        m.insert("rooms.blocked".into(), "\u{5df2}\u{5c01}\u{9501}".into());
        m.insert(
            "rooms.other_aliases".into(),
            "\u{5176}\u{4ed6}\u{522b}\u{540d}".into(),
        );
        m.insert(
            "rooms.no_aliases".into(),
            "\u{672a}\u{627e}\u{5230}\u{5176}\u{4ed6}\u{522b}\u{540d}".into(),
        );
        m.insert(
            "rooms.no_members".into(),
            "\u{672a}\u{627e}\u{5230}\u{6210}\u{5458}".into(),
        );
        m.insert(
            "rooms.no_messages".into(),
            "\u{672a}\u{627e}\u{5230}\u{6d88}\u{606f}".into(),
        );
        m.insert(
            "rooms.no_state_events".into(),
            "\u{6ca1}\u{6709}\u{72b6}\u{6001}\u{4e8b}\u{4ef6}".into(),
        );
        m.insert(
            "rooms.space_hierarchy".into(),
            "\u{7a7a}\u{95f4}\u{5c42}\u{7ea7}".into(),
        );
        m.insert("rooms.space_hierarchy_desc".into(), "\u{6b64}\u{7a7a}\u{95f4}\u{4e2d}\u{7684}\u{5b50}\u{623f}\u{95f4}\u{548c}\u{5b50}\u{7a7a}\u{95f4}".into());
        m.insert(
            "rooms.no_children".into(),
            "\u{672a}\u{627e}\u{5230}\u{5b50}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "rooms.room_name".into(),
            "\u{623f}\u{95f4}\u{540d}\u{79f0} *".into(),
        );
        m.insert(
            "rooms.topic_placeholder".into(),
            "\u{623f}\u{95f4}\u{8bdd}\u{9898}\u{ff08}\u{53ef}\u{9009}\u{ff09}".into(),
        );
        m.insert(
            "rooms.create_description".into(),
            "\u{521b}\u{5efa}\u{65b0}\u{7684} Matrix \u{623f}\u{95f4}".into(),
        );
        m.insert("rooms.create_button".into(), "\u{521b}\u{5efa}".into());
        m.insert(
            "rooms.purge_title".into(),
            "\u{6e05}\u{9664}\u{623f}\u{95f4}\u{5386}\u{53f2}".into(),
        );
        m.insert("rooms.purge_description".into(), "\u{5220}\u{9664}\u{6307}\u{5b9a}\u{65e5}\u{671f}\u{4e4b}\u{524d}\u{7684}\u{6240}\u{6709}\u{6d88}\u{606f}\u{3002}\u{6b64}\u{64cd}\u{4f5c}\u{4e0d}\u{53ef}\u{64a4}\u{9500}\u{3002}".into());
        m.insert(
            "rooms.purge_before".into(),
            "\u{5220}\u{9664}\u{6b64}\u{65e5}\u{671f}\u{4e4b}\u{524d}\u{7684}\u{6d88}\u{606f}"
                .into(),
        );
        m.insert("rooms.purge_warning".into(), "\u{8b66}\u{544a}\u{ff1a}\u{8fd9}\u{5c06}\u{6c38}\u{4e45}\u{5220}\u{9664}\u{6240}\u{9009}\u{65e5}\u{671f}\u{4e4b}\u{524d}\u{7684}\u{6240}\u{6709}\u{6d88}\u{606f}\u{3002}".into());
        m.insert("rooms.delete_confirm".into(), "\u{786e}\u{5b9a}\u{8981}\u{5220}\u{9664}\u{6b64}\u{623f}\u{95f4}\u{5417}\u{ff1f}\u{6240}\u{6709}\u{6d88}\u{606f}\u{548c}\u{5a92}\u{4f53}\u{5c06}\u{88ab}\u{6c38}\u{4e45}\u{5220}\u{9664}\u{3002}\u{6b64}\u{64cd}\u{4f5c}\u{4e0d}\u{53ef}\u{64a4}\u{9500}\u{3002}".into());
        m.insert(
            "rooms.created".into(),
            "\u{521b}\u{5efa}\u{65f6}\u{95f4}".into(),
        );
        m.insert("rooms.kick".into(), "\u{8e22}\u{51fa}".into());
        m.insert(
            "rooms.unban".into(),
            "\u{89e3}\u{9664}\u{5c01}\u{7981}".into(),
        );
        m.insert("rooms.invite".into(), "\u{9080}\u{8bf7}".into());
        m.insert(
            "rooms.promote".into(),
            "\u{8bbe}\u{4e3a}\u{7ba1}\u{7406}\u{5458}".into(),
        );
        m.insert(
            "rooms.invite_user_id".into(),
            "\u{8981}\u{9080}\u{8bf7}\u{7684}\u{7528}\u{6237} ID".into(),
        );
        m.insert(
            "rooms.edit_room".into(),
            "\u{7f16}\u{8f91}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "rooms.save_changes".into(),
            "\u{4fdd}\u{5b58}\u{66f4}\u{6539}".into(),
        );
        m.insert(
            "rooms.add_alias".into(),
            "\u{6dfb}\u{52a0}\u{522b}\u{540d}".into(),
        );
        m.insert("rooms.delete_alias".into(), "\u{5220}\u{9664}".into());
        m.insert("rooms.new_alias".into(), "\u{65b0}\u{522b}\u{540d}".into());
        m.insert("rooms.forward_extremities".into(), "前向极端事件".into());
        m.insert(
            "rooms.forward_extremities_desc".into(),
            "前向极端事件是房间 DAG 中的叶子事件。多个极端事件可能表示碎片化。".into(),
        );
        m.insert(
            "rooms.no_forward_extremities".into(),
            "没有找到前向极端事件。".into(),
        );
        m.insert(
            "rooms.forward_extremities_warning".into(),
            "警告：检测到多个前向极端事件。这可能表示 DAG 碎片化，可能影响性能。".into(),
        );
        m.insert("rooms.count".into(), "数量".into());
        m.insert("rooms.directory_listing".into(), "目录列表".into());
        m.insert("rooms.published".into(), "已发布".into());
        m.insert("rooms.unpublished".into(), "未发布".into());
        m.insert("rooms.publish".into(), "发布".into());
        m.insert("rooms.unpublish".into(), "取消发布".into());
        m.insert("rooms.event_lookup".into(), "事件查找".into());
        m.insert(
            "rooms.event_lookup_desc".into(),
            "通过事件 ID 查找任意事件。".into(),
        );
        m.insert("rooms.lookup".into(), "查找".into());

        // Reports
        m.insert(
            "reports.title".into(),
            "\u{4e3e}\u{62a5}\u{7ba1}\u{7406}".into(),
        );
        m.insert("reports.reporter".into(), "\u{4e3e}\u{62a5}\u{8005}".into());
        m.insert("reports.reason".into(), "\u{539f}\u{56e0}".into());
        m.insert(
            "reports.received".into(),
            "\u{63a5}\u{6536}\u{65f6}\u{95f4}".into(),
        );
        m.insert("reports.status".into(), "\u{72b6}\u{6001}".into());
        m.insert("reports.new".into(), "\u{65b0}\u{4e3e}\u{62a5}".into());
        m.insert(
            "reports.in_review".into(),
            "\u{5ba1}\u{6838}\u{4e2d}".into(),
        );
        m.insert("reports.resolved".into(), "\u{5df2}\u{89e3}\u{51b3}".into());
        m.insert(
            "reports.status_local_only".into(),
            "\u{4ec5}\u{4fdd}\u{5b58}\u{5728}\u{5f53}\u{524d}\u{6d4f}\u{89c8}\u{5668}".into(),
        );
        m.insert(
            "reports.redact".into(),
            "\u{64a4}\u{56de}\u{4e8b}\u{4ef6}".into(),
        );
        m.insert(
            "reports.ban_user".into(),
            "\u{5c01}\u{7981}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "reports.block_room".into(),
            "\u{5c01}\u{9501}\u{623f}\u{95f4}".into(),
        );
        m.insert(
            "reports.moderation_actions".into(),
            "\u{5ba1}\u{6838}\u{64cd}\u{4f5c}".into(),
        );
        m.insert(
            "reports.delete_report".into(),
            "\u{5220}\u{9664}\u{4e3e}\u{62a5}".into(),
        );
        m.insert(
            "reports.no_reports".into(),
            "\u{6682}\u{65e0}\u{4e3e}\u{62a5}".into(),
        );
        m.insert(
            "reports.subtitle".into(),
            "\u{7528}\u{6237}\u{63d0}\u{4ea4}\u{7684}\u{4e8b}\u{4ef6}\u{4e3e}\u{62a5}".into(),
        );
        m.insert("reports.id".into(), "ID".into());
        m.insert("reports.room".into(), "\u{623f}\u{95f4}".into());
        m.insert("reports.no_reports_description".into(), "\u{76ee}\u{524d}\u{6ca1}\u{6709}\u{9700}\u{8981}\u{5ba1}\u{67e5}\u{7684}\u{4e8b}\u{4ef6}\u{4e3e}\u{62a5}".into());
        m.insert(
            "reports.report_details".into(),
            "\u{4e3e}\u{62a5}\u{8be6}\u{60c5}".into(),
        );
        m.insert(
            "reports.reporter_user_id".into(),
            "\u{4e3e}\u{62a5}\u{8005}\u{7528}\u{6237} ID".into(),
        );
        m.insert("reports.room_id".into(), "\u{623f}\u{95f4} ID".into());
        m.insert("reports.event_id".into(), "\u{4e8b}\u{4ef6} ID".into());
        m.insert("reports.sender".into(), "\u{53d1}\u{9001}\u{8005}".into());
        m.insert("reports.score".into(), "\u{8bc4}\u{5206}".into());
        m.insert(
            "reports.event_content".into(),
            "\u{88ab}\u{4e3e}\u{62a5}\u{7684}\u{4e8b}\u{4ef6}\u{5185}\u{5bb9}".into(),
        );
        m.insert(
            "reports.no_event_content".into(),
            "\u{6ca1}\u{6709}\u{53ef}\u{7528}\u{7684}\u{4e8b}\u{4ef6}\u{5185}\u{5bb9}".into(),
        );

        // Media
        m.insert(
            "media.title".into(),
            "\u{5a92}\u{4f53}\u{7ba1}\u{7406}".into(),
        );
        m.insert("media.user_id".into(), "\u{7528}\u{6237} ID".into());
        m.insert(
            "media.media_count".into(),
            "\u{5a92}\u{4f53}\u{6570}\u{91cf}".into(),
        );
        m.insert("media.total_size".into(), "\u{603b}\u{5927}\u{5c0f}".into());
        m.insert("media.actions".into(), "\u{64cd}\u{4f5c}".into());
        m.insert("media.quarantine".into(), "\u{9694}\u{79bb}".into());
        m.insert(
            "media.delete_local".into(),
            "\u{5220}\u{9664}\u{672c}\u{5730}\u{5a92}\u{4f53}".into(),
        );
        m.insert(
            "media.purge_remote".into(),
            "\u{6e05}\u{9664}\u{8fdc}\u{7a0b}\u{5a92}\u{4f53}".into(),
        );
        m.insert(
            "media.search".into(),
            "\u{641c}\u{7d22}\u{7528}\u{6237}...".into(),
        );
        m.insert("media.subtitle".into(), "\u{6309}\u{7528}\u{6237}\u{7edf}\u{8ba1}\u{5a92}\u{4f53}\u{4f7f}\u{7528}\u{60c5}\u{51b5}".into());
        m.insert(
            "media.display_name".into(),
            "\u{663e}\u{793a}\u{540d}\u{79f0}".into(),
        );
        m.insert(
            "media.no_media".into(),
            "\u{672a}\u{627e}\u{5230}\u{5a92}\u{4f53}\u{7edf}\u{8ba1}\u{6570}\u{636e}".into(),
        );

        // Server
        m.insert(
            "server.version".into(),
            "\u{670d}\u{52a1}\u{5668}\u{7248}\u{672c}".into(),
        );
        m.insert(
            "server.features".into(),
            "\u{670d}\u{52a1}\u{5668}\u{529f}\u{80fd}".into(),
        );
        m.insert(
            "server.status".into(),
            "\u{670d}\u{52a1}\u{5668}\u{72b6}\u{6001}".into(),
        );
        m.insert("server.online".into(), "\u{5728}\u{7ebf}".into());
        m.insert("server.healthy".into(), "\u{5065}\u{5eb7}".into());
        m.insert(
            "server.issues_detected".into(),
            "\u{68c0}\u{6d4b}\u{5230}\u{95ee}\u{9898}".into(),
        );
        m.insert(
            "server.maintenance_mode".into(),
            "\u{7ef4}\u{62a4}\u{6a21}\u{5f0f}".into(),
        );
        m.insert(
            "server.actions".into(),
            "\u{670d}\u{52a1}\u{5668}\u{64cd}\u{4f5c}".into(),
        );
        m.insert("server.notifications".into(), "\u{901a}\u{77e5}".into());
        m.insert(
            "server.clear_all".into(),
            "\u{6e05}\u{9664}\u{5168}\u{90e8}".into(),
        );
        m.insert(
            "server.no_notifications".into(),
            "\u{6682}\u{65e0}\u{901a}\u{77e5}".into(),
        );
        m.insert(
            "server.command_running".into(),
            "\u{6b63}\u{5728}\u{6267}\u{884c}\u{547d}\u{4ee4}".into(),
        );

        // Common
        m.insert("common.save".into(), "\u{4fdd}\u{5b58}".into());
        m.insert("common.cancel".into(), "\u{53d6}\u{6d88}".into());
        m.insert("common.delete".into(), "\u{5220}\u{9664}".into());
        m.insert("common.confirm".into(), "\u{786e}\u{8ba4}".into());
        m.insert("common.search".into(), "\u{641c}\u{7d22}...".into());
        m.insert(
            "common.loading".into(),
            "\u{52a0}\u{8f7d}\u{4e2d}...".into(),
        );
        m.insert("common.error".into(), "\u{9519}\u{8bef}".into());
        m.insert("common.success".into(), "\u{6210}\u{529f}".into());
        m.insert(
            "common.no_results".into(),
            "\u{672a}\u{627e}\u{5230}\u{7ed3}\u{679c}".into(),
        );
        m.insert("common.yes".into(), "\u{662f}".into());
        m.insert("common.no".into(), "\u{5426}".into());
        m.insert("common.previous".into(), "\u{4e0a}\u{4e00}\u{9875}".into());
        m.insert("common.next".into(), "\u{4e0b}\u{4e00}\u{9875}".into());
        m.insert("common.retry".into(), "\u{91cd}\u{8bd5}".into());
        m.insert("common.close".into(), "\u{5173}\u{95ed}".into());
        m.insert("common.edit".into(), "\u{7f16}\u{8f91}".into());
        m.insert("common.view".into(), "\u{67e5}\u{770b}".into());
        m.insert("common.selected".into(), "\u{5df2}\u{9009}\u{62e9}".into());
        m.insert(
            "common.clear_selection".into(),
            "\u{6e05}\u{9664}\u{9009}\u{62e9}".into(),
        );
        m.insert("common.columns".into(), "\u{5217}".into());
        // 刷新 / 加载更多 / 显示 / 隐藏 / 关闭提示 / 复制 / 创建 / 状态 / 名称 / 操作 / 失败
        m.insert("common.refresh".into(), "\u{5237}\u{65b0}".into());
        m.insert(
            "common.load_more".into(),
            "\u{52a0}\u{8f7d}\u{66f4}\u{591a}".into(),
        );
        m.insert("common.show".into(), "\u{663e}\u{793a}".into());
        m.insert("common.hide".into(), "\u{9690}\u{85cf}".into());
        m.insert("common.dismiss".into(), "\u{5173}\u{95ed}".into());
        m.insert("common.copy".into(), "\u{590d}\u{5236}".into());
        m.insert("common.create".into(), "\u{521b}\u{5efa}".into());
        m.insert("common.status".into(), "\u{72b6}\u{6001}".into());
        m.insert("common.name".into(), "\u{540d}\u{79f0}".into());
        m.insert("common.actions".into(), "\u{64cd}\u{4f5c}".into());
        m.insert("common.failed".into(), "\u{5931}\u{8d25}".into());

        // Pasion 共享 —— 状态标签
        m.insert("pasion.status_active".into(), "\u{6d3b}\u{8dc3}".into());
        m.insert(
            "pasion.status_revoked".into(),
            "\u{5df2}\u{64a4}\u{9500}".into(),
        );
        m.insert("pasion.status_healthy".into(), "\u{5065}\u{5eb7}".into());
        m.insert("pasion.status_degraded".into(), "\u{964d}\u{7ea7}".into());
        m.insert(
            "pasion.status_unhealthy".into(),
            "\u{4e0d}\u{5065}\u{5eb7}".into(),
        );
        m.insert(
            "pasion.status_down".into(),
            "\u{5df2}\u{5173}\u{95ed}".into(),
        );

        // Pasion 审计日志
        m.insert(
            "pasion.audit_log.title".into(),
            "\u{5ba1}\u{8ba1}\u{65e5}\u{5fd7}".into(),
        );
        m.insert(
            "pasion.audit_log.description".into(),
            "\u{8ffd}\u{8e2a}\u{7ba1}\u{7406}\u{64cd}\u{4f5c}\u{4e0e}\u{53d8}\u{66f4}".into(),
        );
        m.insert(
            "pasion.audit_log.filter_placeholder".into(),
            "\u{6309}\u{64cd}\u{4f5c}\u{7b5b}\u{9009}...".into(),
        );
        m.insert(
            "pasion.audit_log.empty".into(),
            "\u{6682}\u{65e0}\u{5ba1}\u{8ba1}\u{8bb0}\u{5f55}".into(),
        );
        m.insert(
            "pasion.audit_log.load_more_failed".into(),
            "\u{52a0}\u{8f7d}\u{66f4}\u{591a}\u{5931}\u{8d25}\u{ff1a}".into(),
        );
        m.insert(
            "pasion.audit_log.col_timestamp".into(),
            "\u{65f6}\u{95f4}".into(),
        );
        m.insert(
            "pasion.audit_log.col_operation".into(),
            "\u{64cd}\u{4f5c}".into(),
        );
        m.insert(
            "pasion.audit_log.col_admin".into(),
            "\u{7ba1}\u{7406}\u{5458}".into(),
        );
        m.insert(
            "pasion.audit_log.col_resource".into(),
            "\u{8d44}\u{6e90}".into(),
        );
        m.insert(
            "pasion.audit_log.col_ip".into(),
            "IP \u{5730}\u{5740}".into(),
        );
        m.insert(
            "pasion.audit_log.col_detail".into(),
            "\u{8be6}\u{60c5}".into(),
        );

        // Pasion 连接器健康
        m.insert(
            "pasion.connector_health.title".into(),
            "\u{8fde}\u{63a5}\u{5668}\u{5065}\u{5eb7}".into(),
        );
        m.insert(
            "pasion.connector_health.description".into(),
            "\u{540e}\u{7aef}\u{670d}\u{52a1}\u{8fde}\u{63a5}\u{5668}\u{72b6}\u{6001}".into(),
        );
        m.insert(
            "pasion.connector_health.empty".into(),
            "\u{6682}\u{65e0}\u{8fde}\u{63a5}\u{5668}\u{5065}\u{5eb7}\u{6570}\u{636e}".into(),
        );
        m.insert(
            "pasion.connector_health.homeserver".into(),
            "\u{4e3b}\u{670d}\u{52a1}\u{5668}".into(),
        );

        // Pasion 个人访问令牌
        m.insert(
            "pasion.personal_sessions.title".into(),
            "\u{4e2a}\u{4eba}\u{8bbf}\u{95ee}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "pasion.personal_sessions.description".into(),
            "\u{7ba1}\u{7406}\u{4e2a}\u{4eba}\u{8bbf}\u{95ee}\u{4f7f}\u{7528}\u{7684} API \u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "pasion.personal_sessions.create".into(),
            "\u{521b}\u{5efa}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "pasion.personal_sessions.empty".into(),
            "\u{6682}\u{65e0}\u{4e2a}\u{4eba}\u{8bbf}\u{95ee}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "pasion.personal_sessions.new_token_warning".into(),
            "\u{65b0}\u{4ee4}\u{724c}\u{5df2}\u{751f}\u{6210} —— \u{8bf7}\u{7acb}\u{523b}\u{590d}\u{5236}\u{ff0c}\u{7a0d}\u{540e}\u{5c06}\u{4e0d}\u{4f1a}\u{518d}\u{663e}\u{793a}\u{ff01}".into(),
        );
        m.insert(
            "pasion.personal_sessions.copy_success".into(),
            "\u{4ee4}\u{724c}\u{5df2}\u{590d}\u{5236}\u{5230}\u{526a}\u{8d34}\u{677f}".into(),
        );
        m.insert(
            "pasion.personal_sessions.created_success".into(),
            "\u{4e2a}\u{4eba}\u{8bbf}\u{95ee}\u{4ee4}\u{724c}\u{5df2}\u{521b}\u{5efa}".into(),
        );
        m.insert(
            "pasion.personal_sessions.regenerated_success".into(),
            "\u{4ee4}\u{724c}\u{5df2}\u{91cd}\u{65b0}\u{751f}\u{6210}".into(),
        );
        m.insert(
            "pasion.personal_sessions.revoked_success".into(),
            "\u{4ee4}\u{724c}\u{5df2}\u{64a4}\u{9500}".into(),
        );
        m.insert(
            "pasion.personal_sessions.regenerate".into(),
            "\u{91cd}\u{65b0}\u{751f}\u{6210}".into(),
        );
        m.insert(
            "pasion.personal_sessions.revoke".into(),
            "\u{64a4}\u{9500}".into(),
        );
        m.insert(
            "pasion.personal_sessions.create_title".into(),
            "\u{521b}\u{5efa}\u{4e2a}\u{4eba}\u{8bbf}\u{95ee}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "pasion.personal_sessions.name_placeholder".into(),
            "\u{4ee4}\u{724c}\u{540d}\u{79f0}\u{ff08}\u{53ef}\u{9009}\u{ff09}".into(),
        );
        m.insert(
            "pasion.personal_sessions.scope_label".into(),
            "\u{4f5c}\u{7528}\u{57df}".into(),
        );
        m.insert(
            "pasion.personal_sessions.scope_placeholder".into(),
            "\u{4f8b}\u{5982} read write".into(),
        );
        m.insert(
            "pasion.personal_sessions.owner_label".into(),
            "\u{6240}\u{6709}\u{8005}\u{7528}\u{6237} ID\u{ff08}\u{53ef}\u{9009}\u{ff09}".into(),
        );
        m.insert(
            "pasion.personal_sessions.owner_placeholder".into(),
            "\u{7528}\u{6237} ID".into(),
        );
        m.insert(
            "pasion.personal_sessions.revoke_title".into(),
            "\u{64a4}\u{9500}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "pasion.personal_sessions.revoke_description".into(),
            "\u{786e}\u{8ba4}\u{8981}\u{64a4}\u{9500}\u{6b64}\u{8bbf}\u{95ee}\u{4ee4}\u{724c}\u{5417}\u{ff1f}\u{6b64}\u{64cd}\u{4f5c}\u{65e0}\u{6cd5}\u{64a4}\u{9500}\u{3002}".into(),
        );
        m.insert(
            "pasion.personal_sessions.col_scope".into(),
            "\u{4f5c}\u{7528}\u{57df}".into(),
        );
        m.insert(
            "pasion.personal_sessions.col_owner".into(),
            "\u{6240}\u{6709}\u{8005}".into(),
        );
        m.insert(
            "pasion.personal_sessions.col_created".into(),
            "\u{521b}\u{5efa}\u{65f6}\u{95f4}".into(),
        );
        m.insert(
            "pasion.personal_sessions.col_last_active".into(),
            "\u{6700}\u{8fd1}\u{6d3b}\u{52a8}".into(),
        );

        // Pasion OAuth2 会话
        m.insert(
            "pasion.oauth2_sessions.title".into(),
            "OAuth2 \u{4f1a}\u{8bdd}".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.description".into(),
            "Pasion \u{7b7e}\u{53d1}\u{7684}\u{6d4f}\u{89c8}\u{5668}\u{548c}\u{5e94}\u{7528} OAuth2 \u{4f1a}\u{8bdd}".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.empty".into(),
            "\u{6682}\u{65e0} OAuth2 \u{4f1a}\u{8bdd}".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.finish".into(),
            "\u{7ed3}\u{675f}".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.finish_title".into(),
            "\u{7ed3}\u{675f}\u{4f1a}\u{8bdd}".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.finish_description".into(),
            "\u{786e}\u{8ba4}\u{7ed3}\u{675f}\u{6b64} OAuth2 \u{4f1a}\u{8bdd}\u{5417}\u{ff1f}\u{5bf9}\u{5e94}\u{5ba2}\u{6237}\u{7aef}\u{7684}\u{7528}\u{6237}\u{5c06}\u{88ab}\u{767b}\u{51fa}\u{3002}".into(),
        );
        m.insert(
            "pasion.oauth2_sessions.finished_success".into(),
            "\u{4f1a}\u{8bdd}\u{5df2}\u{7ed3}\u{675f}".into(),
        );

        // Pasion 上游提供者
        m.insert(
            "pasion.upstream_providers.title".into(),
            "\u{4e0a}\u{6e38}\u{63d0}\u{4f9b}\u{8005}".into(),
        );
        m.insert(
            "pasion.upstream_providers.description".into(),
            "Pasion \u{5bf9}\u{63a5}\u{7684} OIDC \u{8eab}\u{4efd}\u{63d0}\u{4f9b}\u{8005}".into(),
        );
        m.insert(
            "pasion.upstream_providers.empty".into(),
            "\u{6682}\u{672a}\u{914d}\u{7f6e}\u{4e0a}\u{6e38}\u{63d0}\u{4f9b}\u{8005}".into(),
        );

        // Pasion 本地账号
        m.insert(
            "users.pasion_account".into(),
            "\u{8eab}\u{4efd}\u{8d26}\u{53f7}".into(),
        );
        m.insert(
            "pasion.accounts.title".into(),
            "\u{672c}\u{5730}\u{8d26}\u{53f7}".into(),
        );
        m.insert(
            "pasion.accounts.description".into(),
            "Pasion \u{7ba1}\u{7406}\u{7684}\u{8d26}\u{53f7}\u{ff1a}\u{89d2}\u{8272}\u{3001}\u{9501}\u{5b9a}\u{4e0e}\u{505c}\u{7528}\u{3001}\u{5bc6}\u{7801}\u{548c}\u{4f1a}\u{8bdd}".into(),
        );
        m.insert(
            "pasion.accounts.empty".into(),
            "\u{6682}\u{65e0}\u{8d26}\u{53f7}".into(),
        );
        m.insert(
            "pasion.accounts.status_active".into(),
            "\u{6b63}\u{5e38}".into(),
        );
        m.insert(
            "pasion.accounts.status_locked".into(),
            "\u{5df2}\u{9501}\u{5b9a}".into(),
        );
        m.insert(
            "pasion.accounts.status_deactivated".into(),
            "\u{5df2}\u{505c}\u{7528}".into(),
        );
        m.insert(
            "pasion.accounts.admin".into(),
            "\u{7ba1}\u{7406}\u{5458}".into(),
        );

        // Pasion 上游绑定
        m.insert(
            "pasion.upstream_links.title".into(),
            "\u{4e0a}\u{6e38}\u{7ed1}\u{5b9a}".into(),
        );
        m.insert(
            "pasion.upstream_links.description".into(),
            "\u{7528}\u{6237}\u{4e0e}\u{4e0a}\u{6e38}\u{63d0}\u{4f9b}\u{8005}\u{7684}\u{8054}\u{5408}\u{767b}\u{5f55}\u{7ed1}\u{5b9a}".into(),
        );
        m.insert(
            "pasion.upstream_links.empty".into(),
            "\u{6682}\u{65e0}\u{4e0a}\u{6e38}\u{7ed1}\u{5b9a}".into(),
        );

        // Pasion 通知渠道
        m.insert(
            "pasion.notification_channels.title".into(),
            "\u{901a}\u{77e5}\u{6e20}\u{9053}".into(),
        );
        m.insert(
            "pasion.notification_channels.description".into(),
            "\u{5df2}\u{914d}\u{7f6e}\u{7684}\u{901a}\u{77e5}\u{6295}\u{9012}\u{6e20}\u{9053}\u{72b6}\u{6001}".into(),
        );
        m.insert(
            "pasion.notification_channels.empty".into(),
            "\u{6682}\u{672a}\u{914d}\u{7f6e}\u{901a}\u{77e5}\u{6e20}\u{9053}".into(),
        );

        // Pasion 通知模板
        m.insert(
            "pasion.notification_templates.title".into(),
            "\u{901a}\u{77e5}\u{6a21}\u{677f}".into(),
        );
        m.insert(
            "pasion.notification_templates.description".into(),
            "\u{7528}\u{4e8e}\u{53d1}\u{9001}\u{901a}\u{77e5}\u{7684}\u{90ae}\u{4ef6}\u{548c}\u{77ed}\u{4fe1}\u{6a21}\u{677f}".into(),
        );
        m.insert(
            "pasion.notification_templates.empty".into(),
            "\u{6682}\u{65e0}\u{901a}\u{77e5}\u{6a21}\u{677f}".into(),
        );

        // Dashboard
        m.insert("dashboard.title".into(), "\u{4eea}\u{8868}\u{76d8}".into());
        m.insert(
            "dashboard.welcome".into(),
            "\u{6b22}\u{8fce}\u{4f7f}\u{7528} Hagency Server".into(),
        );
        m.insert(
            "dashboard.total_users".into(),
            "\u{7528}\u{6237}\u{603b}\u{6570}".into(),
        );
        m.insert(
            "dashboard.total_rooms".into(),
            "\u{623f}\u{95f4}\u{603b}\u{6570}".into(),
        );
        m.insert(
            "dashboard.total_reports".into(),
            "\u{5f85}\u{5904}\u{7406}\u{4e3e}\u{62a5}".into(),
        );
        m.insert(
            "dashboard.active_users".into(),
            "\u{6d3b}\u{8dc3}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "dashboard.api_latency".into(),
            "API \u{5ef6}\u{8fdf}".into(),
        );
        m.insert(
            "dashboard.supported_versions".into(),
            "\u{652f}\u{6301}\u{7684}\u{7248}\u{672c}".into(),
        );
        m.insert(
            "dashboard.unstable_features".into(),
            "\u{5b9e}\u{9a8c}\u{6027}\u{529f}\u{80fd}".into(),
        );

        // Registration tokens
        m.insert(
            "registration_tokens.title".into(),
            "\u{6ce8}\u{518c}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "registration_tokens.token".into(),
            "\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "registration_tokens.uses_allowed".into(),
            "\u{5141}\u{8bb8}\u{4f7f}\u{7528}\u{6b21}\u{6570}".into(),
        );
        m.insert(
            "registration_tokens.pending".into(),
            "\u{5f85}\u{4f7f}\u{7528}".into(),
        );
        m.insert(
            "registration_tokens.completed".into(),
            "\u{5df2}\u{4f7f}\u{7528}".into(),
        );
        m.insert(
            "registration_tokens.expiry".into(),
            "\u{8fc7}\u{671f}\u{65f6}\u{95f4}".into(),
        );
        m.insert(
            "registration_tokens.unlimited".into(),
            "\u{65e0}\u{9650}\u{5236}".into(),
        );
        m.insert(
            "registration_tokens.never".into(),
            "\u{6c38}\u{4e0d}".into(),
        );
        m.insert(
            "registration_tokens.create".into(),
            "\u{521b}\u{5efa}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "registration_tokens.no_tokens".into(),
            "\u{6682}\u{65e0}\u{6ce8}\u{518c}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "registration_tokens.subtitle".into(),
            "\u{7ba1}\u{7406}\u{7528}\u{6237}\u{6ce8}\u{518c}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "registration_tokens.actions".into(),
            "\u{64cd}\u{4f5c}".into(),
        );
        m.insert("registration_tokens.no_tokens_description".into(), "\u{521b}\u{5efa}\u{4ee4}\u{724c}\u{4ee5}\u{5141}\u{8bb8}\u{65b0}\u{7528}\u{6237}\u{6ce8}\u{518c}".into());
        m.insert(
            "registration_tokens.delete".into(),
            "\u{5220}\u{9664}".into(),
        );
        m.insert(
            "registration_tokens.delete_token".into(),
            "\u{5220}\u{9664}\u{4ee4}\u{724c}".into(),
        );
        m.insert(
            "registration_tokens.search_placeholder".into(),
            "\u{641c}\u{7d22}\u{4ee4}\u{724c}...".into(),
        );
        m.insert(
            "registration_tokens.filter_all".into(),
            "\u{5168}\u{90e8}".into(),
        );
        m.insert(
            "registration_tokens.filter_active".into(),
            "\u{6d3b}\u{8dc3}".into(),
        );
        m.insert(
            "registration_tokens.filter_expired".into(),
            "\u{5df2}\u{8fc7}\u{671f}".into(),
        );
        m.insert("registration_tokens.create_description".into(), "\u{521b}\u{5efa}\u{5e26}\u{53ef}\u{9009}\u{7ea6}\u{675f}\u{7684}\u{65b0}\u{6ce8}\u{518c}\u{4ee4}\u{724c}".into());
        m.insert(
            "registration_tokens.custom_token".into(),
            "\u{81ea}\u{5b9a}\u{4e49}\u{4ee4}\u{724c}\u{503c}".into(),
        );
        m.insert(
            "registration_tokens.custom_token_placeholder".into(),
            "\u{7559}\u{7a7a}\u{81ea}\u{52a8}\u{751f}\u{6210}".into(),
        );
        m.insert(
            "registration_tokens.custom_token_hint".into(),
            "\u{7559}\u{7a7a}\u{5c06}\u{81ea}\u{52a8}\u{751f}\u{6210}16\u{4f4d}\u{4ee4}\u{724c}"
                .into(),
        );

        // Destinations
        m.insert(
            "destinations.title".into(),
            "\u{8054}\u{90a6}\u{8282}\u{70b9}".into(),
        );
        m.insert(
            "destinations.destination".into(),
            "\u{76ee}\u{6807}\u{670d}\u{52a1}\u{5668}".into(),
        );
        m.insert(
            "destinations.retry_interval".into(),
            "\u{91cd}\u{8bd5}\u{95f4}\u{9694}".into(),
        );
        m.insert(
            "destinations.last_failure".into(),
            "\u{6700}\u{540e}\u{5931}\u{8d25}".into(),
        );
        m.insert(
            "destinations.reset".into(),
            "\u{91cd}\u{7f6e}\u{8fde}\u{63a5}".into(),
        );
        m.insert(
            "destinations.search".into(),
            "\u{641c}\u{7d22}\u{8282}\u{70b9}...".into(),
        );
        m.insert(
            "destinations.no_destinations".into(),
            "\u{6682}\u{65e0}\u{8054}\u{90a6}\u{8282}\u{70b9}".into(),
        );
        m.insert(
            "destinations.subtitle".into(),
            "\u{8fdc}\u{7a0b}\u{670d}\u{52a1}\u{5668}\u{76ee}\u{6807}".into(),
        );
        m.insert("destinations.status".into(), "\u{72b6}\u{6001}".into());
        m.insert(
            "destinations.last_retry".into(),
            "\u{6700}\u{540e}\u{91cd}\u{8bd5}".into(),
        );
        m.insert("destinations.actions".into(), "\u{64cd}\u{4f5c}".into());
        m.insert(
            "destinations.no_destinations_found".into(),
            "\u{672a}\u{627e}\u{5230}\u{76ee}\u{6807}\u{670d}\u{52a1}\u{5668}".into(),
        );
        m.insert(
            "destinations.reset_button".into(),
            "\u{91cd}\u{7f6e}".into(),
        );
        m.insert("destinations.failed".into(), "\u{5931}\u{8d25}".into());
        m.insert("destinations.ok".into(), "\u{6b63}\u{5e38}".into());
        m.insert("destinations.detail_description".into(), "\u{8054}\u{90a6}\u{76ee}\u{6807}\u{8be6}\u{60c5}\u{548c}\u{91cd}\u{8bd5}\u{72b6}\u{6001}".into());
        m.insert(
            "destinations.connection_info".into(),
            "\u{8fde}\u{63a5}\u{4fe1}\u{606f}".into(),
        );
        m.insert(
            "destinations.retry_info".into(),
            "\u{91cd}\u{8bd5}\u{4fe1}\u{606f}".into(),
        );
        m.insert(
            "destinations.last_successful_stream".into(),
            "\u{6700}\u{540e}\u{6210}\u{529f}\u{6d41}".into(),
        );
        m.insert("destinations.status".into(), "\u{72b6}\u{6001}".into());

        // Server Notices
        m.insert(
            "server_notices.title".into(),
            "\u{670d}\u{52a1}\u{5668}\u{901a}\u{77e5}".into(),
        );
        m.insert(
            "server_notices.subtitle".into(),
            "\u{5411}\u{7528}\u{6237}\u{53d1}\u{9001}\u{670d}\u{52a1}\u{5668}\u{901a}\u{77e5}"
                .into(),
        );
        m.insert(
            "server_notices.send_title".into(),
            "\u{53d1}\u{9001}\u{670d}\u{52a1}\u{5668}\u{901a}\u{77e5}".into(),
        );
        m.insert("server_notices.broadcast_desc".into(), "\u{5411}\u{6240}\u{6709}\u{7528}\u{6237}\u{5e7f}\u{64ad}\u{7ba1}\u{7406}\u{901a}\u{77e5}".into());
        m.insert("server_notices.single_desc".into(), "\u{5411}\u{7279}\u{5b9a}\u{7528}\u{6237}\u{53d1}\u{9001}\u{7ba1}\u{7406}\u{901a}\u{77e5}".into());
        m.insert(
            "server_notices.single_user".into(),
            "\u{5355}\u{4e2a}\u{7528}\u{6237}".into(),
        );
        m.insert(
            "server_notices.broadcast".into(),
            "\u{5e7f}\u{64ad}\u{5168}\u{90e8}".into(),
        );
        m.insert(
            "server_notices.user_id".into(),
            "\u{7528}\u{6237} ID".into(),
        );
        m.insert("server_notices.message".into(), "\u{6d88}\u{606f}".into());
        m.insert(
            "server_notices.message_placeholder".into(),
            "\u{8f93}\u{5165}\u{901a}\u{77e5}\u{6d88}\u{606f}...".into(),
        );
        m.insert(
            "server_notices.broadcast_notice".into(),
            "\u{5e7f}\u{64ad}\u{901a}\u{77e5}".into(),
        );
        m.insert(
            "server_notices.send_notice".into(),
            "\u{53d1}\u{9001}\u{901a}\u{77e5}".into(),
        );
        m.insert(
            "server_notices.history_title".into(),
            "\u{901a}\u{77e5}\u{5386}\u{53f2}".into(),
        );
        m.insert("server_notices.history_desc".into(), "\u{4e4b}\u{524d}\u{53d1}\u{9001}\u{7684}\u{670d}\u{52a1}\u{5668}\u{901a}\u{77e5}\u{ff08}\u{672c}\u{5730}\u{5b58}\u{50a8}\u{ff09}".into());
        m.insert(
            "server_notices.clear_history".into(),
            "\u{6e05}\u{9664}\u{5386}\u{53f2}".into(),
        );
        m.insert(
            "server_notices.no_notices".into(),
            "\u{5c1a}\u{672a}\u{53d1}\u{9001}\u{901a}\u{77e5}".into(),
        );
        m.insert("server_notices.user".into(), "\u{7528}\u{6237}".into());
        m.insert(
            "server_notices.timestamp".into(),
            "\u{65f6}\u{95f4}\u{6233}".into(),
        );
        m.insert(
            "server_notices.event_id".into(),
            "\u{4e8b}\u{4ef6} ID".into(),
        );

        // Scheduled / recurring commands
        m.insert(
            "commands.scheduled_title".into(),
            "\u{8ba1}\u{5212}\u{547d}\u{4ee4}".into(),
        );
        m.insert(
            "commands.recurring_title".into(),
            "\u{5468}\u{671f}\u{547d}\u{4ee4}".into(),
        );
        m.insert("commands.create".into(), "\u{521b}\u{5efa}".into());
        m.insert("commands.edit".into(), "\u{7f16}\u{8f91}".into());
        m.insert("commands.delete".into(), "\u{5220}\u{9664}".into());
        m.insert("commands.cancel".into(), "\u{53d6}\u{6d88}".into());
        m.insert("commands.save".into(), "\u{4fdd}\u{5b58}".into());
        m.insert("commands.retry".into(), "\u{91cd}\u{8bd5}".into());
        m.insert(
            "commands.none_scheduled".into(),
            "\u{6ca1}\u{6709}\u{8ba1}\u{5212}\u{547d}\u{4ee4}\u{3002}".into(),
        );
        m.insert(
            "commands.none_recurring".into(),
            "\u{6ca1}\u{6709}\u{5468}\u{671f}\u{547d}\u{4ee4}\u{3002}".into(),
        );
        m.insert(
            "commands.load_failed".into(),
            "\u{52a0}\u{8f7d}\u{547d}\u{4ee4}\u{5931}\u{8d25}\u{3002}".into(),
        );
        m.insert("commands.col_command".into(), "\u{547d}\u{4ee4}".into());
        m.insert("commands.col_arguments".into(), "\u{53c2}\u{6570}".into());
        m.insert(
            "commands.col_scheduled_at".into(),
            "\u{8ba1}\u{5212}\u{65f6}\u{95f4}".into(),
        );
        m.insert("commands.col_time".into(), "\u{65f6}\u{95f4}（UTC）".into());
        m.insert("commands.col_actions".into(), "\u{64cd}\u{4f5c}".into());
        m.insert("commands.field_command".into(), "\u{547d}\u{4ee4}".into());
        m.insert(
            "commands.field_command_placeholder".into(),
            "\u{547d}\u{4ee4}\u{540d}\u{79f0}".into(),
        );
        m.insert(
            "commands.field_arguments".into(),
            "\u{53c2}\u{6570}（\u{53ef}\u{9009}）".into(),
        );
        m.insert(
            "commands.field_arguments_placeholder".into(),
            "\u{53c2}\u{6570}".into(),
        );
        m.insert(
            "commands.scheduled_at_label".into(),
            "\u{8ba1}\u{5212}\u{65f6}\u{95f4}（ISO 8601）".into(),
        );
        m.insert(
            "commands.scheduled_at_placeholder".into(),
            "2025-01-15T10:00:00Z".into(),
        );
        m.insert(
            "commands.time_label".into(),
            "\u{65f6}\u{95f4}（UTC，\u{4f8b}\u{5982} 03:00）".into(),
        );
        m.insert("commands.time_placeholder".into(), "HH:MM".into());
        m.insert(
            "commands.dialog_edit_scheduled".into(),
            "\u{7f16}\u{8f91}\u{8ba1}\u{5212}\u{547d}\u{4ee4}".into(),
        );
        m.insert(
            "commands.dialog_schedule".into(),
            "\u{8ba1}\u{5212}\u{547d}\u{4ee4}".into(),
        );
        m.insert(
            "commands.dialog_edit_recurring".into(),
            "\u{7f16}\u{8f91}\u{5468}\u{671f}\u{547d}\u{4ee4}".into(),
        );
        m.insert(
            "commands.dialog_create_recurring".into(),
            "\u{521b}\u{5efa}\u{5468}\u{671f}\u{547d}\u{4ee4}".into(),
        );
        m.insert("commands.submit_schedule".into(), "\u{8ba1}\u{5212}".into());
        m.insert("commands.submit_create".into(), "\u{521b}\u{5efa}".into());
        m.insert("commands.submit_save".into(), "\u{4fdd}\u{5b58}".into());
        m.insert(
            "commands.delete_scheduled_title".into(),
            "\u{5220}\u{9664}\u{8ba1}\u{5212}\u{547d}\u{4ee4}".into(),
        );
        m.insert(
            "commands.delete_recurring_title".into(),
            "\u{5220}\u{9664}\u{5468}\u{671f}\u{547d}\u{4ee4}".into(),
        );
        m.insert(
            "commands.delete_scheduled_desc".into(),
            "\u{786e}\u{5b9a}\u{8981}\u{5220}\u{9664}\u{8fd9}\u{4e2a}\u{8ba1}\u{5212}\u{547d}\u{4ee4}\u{5417}？".into(),
        );
        m.insert(
            "commands.delete_recurring_desc".into(),
            "\u{786e}\u{5b9a}\u{8981}\u{5220}\u{9664}\u{8fd9}\u{4e2a}\u{5468}\u{671f}\u{547d}\u{4ee4}\u{5417}？".into(),
        );
        m.insert(
            "commands.validation_scheduled".into(),
            "\u{547d}\u{4ee4}\u{548c}\u{8ba1}\u{5212}\u{65f6}\u{95f4}\u{4e3a}\u{5fc5}\u{586b}\u{9879}".into(),
        );
        m.insert(
            "commands.validation_recurring".into(),
            "\u{547d}\u{4ee4}\u{548c}\u{65f6}\u{95f4}\u{4e3a}\u{5fc5}\u{586b}\u{9879}".into(),
        );
        m.insert(
            "commands.toast_deleted".into(),
            "\u{5df2}\u{5220}\u{9664}".into(),
        );
        m.insert(
            "commands.toast_scheduled".into(),
            "\u{547d}\u{4ee4}\u{5df2}\u{8ba1}\u{5212}".into(),
        );
        m.insert(
            "commands.toast_updated".into(),
            "\u{547d}\u{4ee4}\u{5df2}\u{66f4}\u{65b0}".into(),
        );
        m.insert(
            "commands.toast_recurring_created".into(),
            "\u{5468}\u{671f}\u{547d}\u{4ee4}\u{5df2}\u{521b}\u{5efa}".into(),
        );
        m.insert(
            "commands.toast_recurring_updated".into(),
            "\u{5468}\u{671f}\u{547d}\u{4ee4}\u{5df2}\u{66f4}\u{65b0}".into(),
        );

        // Not Found
        m.insert("not_found.title".into(), "404".into());
        m.insert(
            "not_found.message".into(),
            "\u{9875}\u{9762}\u{672a}\u{627e}\u{5230}".into(),
        );
        m.insert(
            "not_found.go_dashboard".into(),
            "\u{8fd4}\u{56de}\u{4eea}\u{8868}\u{76d8}".into(),
        );

        // Language
        m.insert("language.en".into(), "English".into());
        m.insert("language.zh_cn".into(), "\u{4e2d}\u{6587}".into());
        m.insert("language.select".into(), "\u{8bed}\u{8a00}".into());

        m
    }
}

impl Default for I18n {
    fn default() -> Self {
        Self::new()
    }
}

impl std::hash::Hash for Language {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
    }
}

impl Eq for Language {}

thread_local! {
    static I18N: I18n = I18n::new();
}

static CURRENT_LANG: GlobalSignal<Language> = GlobalSignal::new(|| {
    crate::utils::storage::get_item("language")
        .and_then(|s| Language::from_code(&s))
        .unwrap_or(Language::En)
});

pub fn t(key: &str) -> String {
    let lang = *CURRENT_LANG.read();
    I18N.with(|i18n| i18n.t(key, lang))
}

pub fn t_with(key: &str, params: &[(&str, &str)]) -> String {
    let lang = *CURRENT_LANG.read();
    I18N.with(|i18n| i18n.t_with(key, lang, params))
}

pub fn set_language(lang: Language) {
    crate::utils::storage::set_item("language", lang.code());
    *CURRENT_LANG.write() = lang;
}

pub fn current_language() -> Language {
    *CURRENT_LANG.read()
}
