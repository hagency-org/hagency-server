use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── User types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Threepid {
    pub medium: String,
    pub address: String,
    #[serde(default)]
    pub added_at: Option<u64>,
    #[serde(default)]
    pub validated_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalId {
    pub auth_provider: String,
    pub external_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct User {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub displayname: Option<String>,
    #[serde(default)]
    pub threepids: Vec<Threepid>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub is_guest: bool,
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub deactivated: bool,
    #[serde(default)]
    pub erased: bool,
    #[serde(default)]
    pub shadow_banned: bool,
    #[serde(default)]
    pub creation_ts: u64,
    #[serde(default)]
    pub external_ids: Vec<ExternalId>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub suspended: bool,
    #[serde(default)]
    pub user_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserRecord {
    pub id: String,
    #[serde(flatten)]
    pub user: User,
    #[serde(default)]
    pub avatar_src: Option<String>,
    #[serde(default)]
    pub creation_ts_ms: u64,
}

// ── Room types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Room {
    #[serde(default)]
    pub room_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub canonical_alias: Option<String>,
    #[serde(default)]
    pub joined_members: u64,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub creator: Option<String>,
    #[serde(default)]
    pub encryption: Option<String>,
    #[serde(default)]
    pub federatable: bool,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub join_rules: Option<String>,
    #[serde(default)]
    pub guest_access: Option<String>,
    #[serde(default)]
    pub history_visibility: Option<String>,
    #[serde(default)]
    pub state_events: u64,
    #[serde(default)]
    pub room_type: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub is_blocked: bool,
    #[serde(default)]
    pub blocked_by: Option<String>,
}

impl Room {
    pub fn is_blocked_effective(&self) -> bool {
        self.blocked || self.is_blocked || self.blocked_by.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomRecord {
    pub id: String,
    #[serde(flatten)]
    pub room: Room,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub members: u64,
    #[serde(default)]
    pub is_encrypted: bool,
    #[serde(default)]
    pub avatar: Option<String>,
}

// ── Room hierarchy types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HierarchyRoom {
    #[serde(default)]
    pub room_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub num_joined_members: u64,
    #[serde(default)]
    pub room_type: Option<String>,
    #[serde(default)]
    pub children_state: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HierarchyResponse {
    #[serde(default)]
    pub rooms: Vec<HierarchyRoom>,
}

// ── Room state types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomState {
    #[serde(default)]
    pub age: Option<u64>,
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub sender: String,
    #[serde(rename = "type", default)]
    pub event_type: String,
    #[serde(default)]
    pub unsigned: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForwardExtremity {
    pub event_id: String,
    #[serde(default)]
    pub state_group: Option<i64>,
    #[serde(default)]
    pub depth: i64,
    #[serde(default)]
    pub received_ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForwardExtremitiesResponse {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub results: Vec<ForwardExtremity>,
}

// ── Room message types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomMessage {
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub sender: String,
    #[serde(default)]
    pub origin_server_ts: u64,
    #[serde(default)]
    pub content: RoomMessageContent,
    #[serde(rename = "type", default)]
    pub event_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomMessageContent {
    #[serde(default)]
    pub msgtype: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

// ── Report types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventReport {
    pub id: u64,
    #[serde(default)]
    pub received_ts: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub room_id: String,
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub score: Option<i64>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub event_json: Option<serde_json::Value>,
}

// ── Device types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Device {
    pub device_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub last_seen_ip: Option<String>,
    #[serde(default)]
    pub last_seen_user_agent: Option<String>,
    #[serde(default)]
    pub last_seen_ts: Option<u64>,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceRecord {
    pub id: String,
    #[serde(flatten)]
    pub device: Device,
}

// ── Connection/Whois types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Connection {
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub last_seen: Option<u64>,
    #[serde(default)]
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhoisSession {
    #[serde(default)]
    pub connections: Vec<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhoisDevice {
    #[serde(default)]
    pub sessions: Vec<WhoisSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Whois {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub devices: HashMap<String, WhoisDevice>,
}

// ── Media types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserMediaStatistic {
    #[serde(default)]
    pub displayname: Option<String>,
    #[serde(default)]
    pub media_count: u64,
    #[serde(default)]
    pub media_length: u64,
    #[serde(default)]
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserMediaStatisticRecord {
    pub id: String,
    #[serde(flatten)]
    pub statistic: UserMediaStatistic,
}

// ── Registration token types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistrationToken {
    pub token: String,
    #[serde(default)]
    pub uses_allowed: Option<u64>,
    #[serde(default)]
    pub pending: u64,
    #[serde(default)]
    pub completed: u64,
    #[serde(default)]
    pub expiry_time: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistrationTokenRecord {
    pub id: String,
    #[serde(flatten)]
    pub token: RegistrationToken,
}

// ── Destination types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Destination {
    pub destination: String,
    #[serde(default)]
    pub retry_last_ts: u64,
    #[serde(default)]
    pub retry_interval: u64,
    #[serde(default)]
    pub failure_ts: Option<u64>,
    #[serde(default)]
    pub last_successful_stream_ordering: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DestinationRecord {
    pub id: String,
    #[serde(flatten)]
    pub destination: Destination,
}

// ── Media operations ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeleteMediaResult {
    #[serde(default)]
    pub deleted_media: Vec<String>,
    #[serde(default)]
    pub total: u64,
}

// ── Feature/limits types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExperimentalFeaturesModel {
    pub features: HashMap<String, bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RateLimitsModel {
    #[serde(default)]
    pub messages_per_second: Option<u64>,
    #[serde(default)]
    pub burst_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountDataModel {
    #[serde(default)]
    pub account_data: serde_json::Value,
}

// ── Pagination types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListResponse<T> {
    pub data: Vec<T>,
    pub total: u64,
}

// ── Server info types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerVersionResponse {
    pub server_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SupportedFeatures {
    #[serde(default)]
    pub versions: Vec<String>,
    #[serde(default)]
    pub unstable_features: HashMap<String, bool>,
}

// ── Server status types (Palpo-specific) ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerStatusComponent {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerStatusResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub maintenance: bool,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub results: Vec<ServerStatusComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerProcessResponse {
    #[serde(default)]
    pub locked_at: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub maintenance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerNotification {
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub sent_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerNotificationsResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub notifications: Vec<ServerNotification>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerCommand {
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub args: Option<serde_json::Value>,
    #[serde(default)]
    pub with_lock: bool,
    #[serde(rename = "additionalArgs", default)]
    pub additional_args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ScheduledCommand {
    #[serde(default)]
    pub args: Option<serde_json::Value>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub is_recurring: bool,
    #[serde(default)]
    pub scheduled_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RecurringCommand {
    #[serde(default)]
    pub args: Option<serde_json::Value>,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub scheduled_at: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
}

// ── Auth types ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhoamiResponse {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoginFlow {
    #[serde(rename = "type")]
    pub flow_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoginFlowsResponse {
    #[serde(default)]
    pub flows: Vec<LoginFlow>,
}

// ── User list API response ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsersListResponse {
    #[serde(default)]
    pub users: Vec<User>,
    #[serde(default)]
    pub total: u64,
}

// ── Room list API response ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomsListResponse {
    #[serde(default)]
    pub rooms: Vec<Room>,
    #[serde(default)]
    pub total_rooms: u64,
}

// ── Reports API response ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventReportsResponse {
    #[serde(default)]
    pub event_reports: Vec<EventReport>,
    #[serde(default)]
    pub total: u64,
}

// ── Destinations API response ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DestinationsResponse {
    #[serde(default)]
    pub destinations: Vec<Destination>,
    #[serde(default)]
    pub total: u64,
}

// ── Registration tokens API response ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistrationTokensResponse {
    #[serde(default)]
    pub registration_tokens: Vec<RegistrationToken>,
}

// ── Media statistics API response ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaStatisticsResponse {
    #[serde(default)]
    pub users: Vec<UserMediaStatistic>,
    #[serde(default)]
    pub total: u64,
}

// ── Profile response ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileResponse {
    #[serde(default)]
    pub displayname: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

// ── Create user request ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateUserRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub displayname: Option<String>,
    #[serde(default)]
    pub admin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deactivated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threepids: Vec<Threepid>,
}
