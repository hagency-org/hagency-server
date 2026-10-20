use crate::api::client::*;
use crate::types::*;
use crate::utils::cache::{cached, invalidate_cached_prefix, remove_cached};
use crate::utils::error::HttpError;

const ROOM_LIST_CACHE_TTL_MS: f64 = 30_000.0;
const ROOM_LIST_CACHE_PREFIX: &str = "rooms:list:";
const DASHBOARD_ROOM_COUNT_CACHE_KEY: &str = "dashboard_room_count";

fn room_list_cache_key(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> String {
    let encoded_search = urlencoding::encode(search_term);
    format!("{ROOM_LIST_CACHE_PREFIX}{page}:{per_page}:{order_by}:{order}:{encoded_search}")
}

pub fn invalidate_room_related_caches() {
    invalidate_cached_prefix(ROOM_LIST_CACHE_PREFIX);
    remove_cached(DASHBOARD_ROOM_COUNT_CACHE_KEY);
}

fn map_room(room: Room) -> RoomRecord {
    let id = room.room_id.clone();
    let alias = room.canonical_alias.clone();
    let members = room.joined_members;
    let is_encrypted = room.encryption.is_some();
    let avatar = room.avatar_url.clone();
    RoomRecord {
        id,
        alias,
        members,
        is_encrypted,
        avatar,
        room,
    }
}

pub async fn get_room_count() -> Result<u64, HttpError> {
    let url = build_url("/_palpo/admin/v1/rooms", &[("limit", "1")])?;
    let response: RoomsListResponse = api_client(&url, "GET", None).await?;
    Ok(response.total_rooms)
}

pub async fn get_rooms(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> Result<ListResponse<RoomRecord>, HttpError> {
    let from = (page - 1) * per_page;
    let from_str = from.to_string();
    let limit_str = per_page.to_string();
    let dir = get_search_order(order);

    let mut params = vec![
        ("from", from_str.as_str()),
        ("limit", limit_str.as_str()),
        ("order_by", order_by),
        ("dir", dir),
    ];

    if !search_term.is_empty() {
        params.push(("search_term", search_term));
    }

    let url = build_url("/_palpo/admin/v1/rooms", &params)?;
    let response: RoomsListResponse = api_client(&url, "GET", None).await?;

    Ok(ListResponse {
        data: response.rooms.into_iter().map(map_room).collect(),
        total: response.total_rooms,
    })
}

pub async fn get_rooms_cached(
    page: u64,
    per_page: u64,
    order_by: &str,
    order: &str,
    search_term: &str,
) -> Result<ListResponse<RoomRecord>, HttpError> {
    let cache_key = room_list_cache_key(page, per_page, order_by, order, search_term);
    cached(
        &cache_key,
        ROOM_LIST_CACHE_TTL_MS,
        get_rooms(page, per_page, order_by, order, search_term),
    )
    .await
}

pub async fn get_room(id: &str) -> Result<RoomRecord, HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(&format!("/_palpo/admin/v1/rooms/{encoded}"), &[])?;
    let room: Room = api_client(&url, "GET", None).await?;
    Ok(map_room(room))
}

pub async fn delete_room(id: &str, block: bool) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(&format!("/_palpo/admin/v2/rooms/{encoded}"), &[])?;
    let body = serde_json::json!({ "block": block }).to_string();
    let _: serde_json::Value = api_client(&url, "DELETE", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn get_room_members(
    room_id: &str,
    page: u64,
    per_page: u64,
) -> Result<ListResponse<String>, HttpError> {
    let from = (page - 1) * per_page;
    let encoded = urlencoding::encode(room_id);
    let from_str = from.to_string();
    let limit_str = per_page.to_string();
    let url = build_url(
        &format!("/_palpo/admin/v1/rooms/{encoded}/members"),
        &[("from", &from_str), ("limit", &limit_str)],
    )?;

    #[derive(serde::Deserialize)]
    struct Resp {
        members: Vec<String>,
        total: u64,
    }

    let response: Resp = api_client(&url, "GET", None).await?;

    Ok(ListResponse {
        data: response.members,
        total: response.total,
    })
}

pub async fn get_room_state(room_id: &str) -> Result<Vec<RoomState>, HttpError> {
    let encoded = urlencoding::encode(room_id);
    let url = build_url(&format!("/_palpo/admin/v1/rooms/{encoded}/state"), &[])?;

    #[derive(serde::Deserialize)]
    struct Resp {
        state: Vec<RoomState>,
    }

    let response: Resp = api_client(&url, "GET", None).await?;
    Ok(response.state)
}

pub async fn block_room(id: &str, block: bool) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(id);
    let url = build_url(&format!("/_palpo/admin/v1/rooms/{encoded}/block"), &[])?;
    let body = serde_json::json!({ "block": block }).to_string();
    let _: serde_json::Value = api_client(&url, "PUT", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn get_room_messages(room_id: &str, limit: u64) -> Result<Vec<RoomMessage>, HttpError> {
    let encoded = urlencoding::encode(room_id);
    let limit_str = limit.to_string();
    let url = build_url(
        &format!("/_palpo/admin/v1/rooms/{encoded}/messages"),
        &[("limit", &limit_str), ("dir", "b")],
    )?;

    #[derive(serde::Deserialize)]
    struct Resp {
        #[serde(default)]
        chunk: Vec<RoomMessage>,
    }

    let response: Resp = api_client(&url, "GET", None).await?;
    Ok(response.chunk)
}

pub async fn create_room(name: &str, topic: &str, public: bool) -> Result<String, HttpError> {
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/createRoom");
    let visibility = if public { "public" } else { "private" };
    let preset = if public {
        "public_chat"
    } else {
        "private_chat"
    };
    let mut body = serde_json::json!({
        "name": name,
        "visibility": visibility,
        "preset": preset,
    });
    if !topic.is_empty() {
        body["topic"] = serde_json::Value::String(topic.to_string());
    }
    let body_str = body.to_string();

    #[derive(serde::Deserialize)]
    struct Resp {
        room_id: String,
    }

    let response: Resp = api_client(&url, "POST", Some(body_str)).await?;
    invalidate_room_related_caches();
    Ok(response.room_id)
}

pub async fn purge_history(room_id: &str, purge_up_to_ts: u64) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(room_id);
    let url = build_url(&format!("/_palpo/admin/v1/purge_history/{encoded}"), &[])?;
    let body = serde_json::json!({ "purge_up_to_ts": purge_up_to_ts }).to_string();
    let _: serde_json::Value = api_client(&url, "POST", Some(body)).await?;
    Ok(())
}

pub async fn redact_event(
    room_id: &str,
    event_id: &str,
    txn_id: &str,
    reason: &str,
) -> Result<(), HttpError> {
    let encoded_room = urlencoding::encode(room_id);
    let encoded_event = urlencoding::encode(event_id);
    let encoded_txn = urlencoding::encode(txn_id);
    let base_url = get_base_url()?;
    let url = format!(
        "{base_url}/_matrix/client/v3/rooms/{encoded_room}/redact/{encoded_event}/{encoded_txn}"
    );
    let body = serde_json::json!({ "reason": reason }).to_string();
    let _: serde_json::Value = api_client(&url, "PUT", Some(body)).await?;
    Ok(())
}

pub async fn ban_user(room_id: &str, user_id: &str, reason: &str) -> Result<(), HttpError> {
    let encoded_room = urlencoding::encode(room_id);
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/rooms/{encoded_room}/ban");
    let body = serde_json::json!({ "user_id": user_id, "reason": reason }).to_string();
    let _: serde_json::Value = api_client(&url, "POST", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn kick_user(room_id: &str, user_id: &str, reason: &str) -> Result<(), HttpError> {
    let encoded_room = urlencoding::encode(room_id);
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/rooms/{encoded_room}/kick");
    let body = serde_json::json!({ "user_id": user_id, "reason": reason }).to_string();
    let _: serde_json::Value = api_client(&url, "POST", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn invite_user(room_id: &str, user_id: &str) -> Result<(), HttpError> {
    let encoded_room = urlencoding::encode(room_id);
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/rooms/{encoded_room}/invite");
    let body = serde_json::json!({ "user_id": user_id }).to_string();
    let _: serde_json::Value = api_client(&url, "POST", Some(body)).await?;
    Ok(())
}

pub async fn set_room_state(
    room_id: &str,
    event_type: &str,
    state_key: &str,
    content: serde_json::Value,
) -> Result<(), HttpError> {
    let encoded_room = urlencoding::encode(room_id);
    let encoded_type = urlencoding::encode(event_type);
    let encoded_key = urlencoding::encode(state_key);
    let base_url = get_base_url()?;
    let url = format!(
        "{base_url}/_matrix/client/v3/rooms/{encoded_room}/state/{encoded_type}/{encoded_key}"
    );
    let body = content.to_string();
    let _: serde_json::Value = api_client(&url, "PUT", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn set_room_name(room_id: &str, name: &str) -> Result<(), HttpError> {
    set_room_state(
        room_id,
        "m.room.name",
        "",
        serde_json::json!({ "name": name }),
    )
    .await
}

pub async fn set_room_topic(room_id: &str, topic: &str) -> Result<(), HttpError> {
    set_room_state(
        room_id,
        "m.room.topic",
        "",
        serde_json::json!({ "topic": topic }),
    )
    .await
}

pub async fn set_room_join_rules(room_id: &str, join_rule: &str) -> Result<(), HttpError> {
    set_room_state(
        room_id,
        "m.room.join_rules",
        "",
        serde_json::json!({ "join_rule": join_rule }),
    )
    .await
}

pub async fn set_room_history_visibility(room_id: &str, visibility: &str) -> Result<(), HttpError> {
    set_room_state(
        room_id,
        "m.room.history_visibility",
        "",
        serde_json::json!({ "history_visibility": visibility }),
    )
    .await
}

pub async fn put_room_alias(alias: &str, room_id: &str) -> Result<(), HttpError> {
    let encoded_alias = urlencoding::encode(alias);
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/directory/room/{encoded_alias}");
    let body = serde_json::json!({ "room_id": room_id }).to_string();
    let _: serde_json::Value = api_client(&url, "PUT", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn delete_room_alias(alias: &str) -> Result<(), HttpError> {
    let encoded_alias = urlencoding::encode(alias);
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/directory/room/{encoded_alias}");
    let _: serde_json::Value = api_client(&url, "DELETE", None).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn get_room_hierarchy(room_id: &str) -> Result<Vec<HierarchyRoom>, HttpError> {
    let encoded = urlencoding::encode(room_id);
    let url = build_url(&format!("/_palpo/admin/v1/rooms/{encoded}/hierarchy"), &[])?;
    let response: HierarchyResponse = api_client(&url, "GET", None).await?;
    Ok(response.rooms)
}

pub async fn get_forward_extremities(
    room_id: &str,
) -> Result<ForwardExtremitiesResponse, HttpError> {
    let encoded = urlencoding::encode(room_id);
    let url = build_url(
        &format!("/_palpo/admin/v1/rooms/{encoded}/forward_extremities"),
        &[],
    )?;
    let response: ForwardExtremitiesResponse = api_client(&url, "GET", None).await?;
    Ok(response)
}

pub async fn fetch_event(event_id: &str) -> Result<serde_json::Value, HttpError> {
    let encoded = urlencoding::encode(event_id);
    let url = build_url(&format!("/_palpo/admin/v1/fetch_event/{encoded}"), &[])?;

    #[derive(serde::Deserialize)]
    struct Resp {
        event: serde_json::Value,
    }

    let response: Resp = api_client(&url, "GET", None).await?;
    Ok(response.event)
}

pub async fn get_room_directory_visibility(room_id: &str) -> Result<bool, HttpError> {
    let encoded = urlencoding::encode(room_id);
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/directory/list/room/{encoded}");

    #[derive(serde::Deserialize)]
    struct Resp {
        visibility: String,
    }

    let response: Resp = api_client(&url, "GET", None).await?;
    Ok(response.visibility == "public")
}

pub async fn set_room_directory_visibility(room_id: &str, public: bool) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(room_id);
    let base_url = get_base_url()?;
    let url = format!("{base_url}/_matrix/client/v3/directory/list/room/{encoded}");
    let visibility = if public { "public" } else { "private" };
    let body = serde_json::json!({ "visibility": visibility }).to_string();
    let _: serde_json::Value = api_client(&url, "PUT", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}

pub async fn make_room_admin(room_id: &str, user_id: &str) -> Result<(), HttpError> {
    let encoded = urlencoding::encode(room_id);
    let url = build_url(
        &format!("/_palpo/admin/v1/rooms/{encoded}/make_room_admin"),
        &[],
    )?;
    let body = serde_json::json!({ "user_id": user_id }).to_string();
    let _: serde_json::Value = api_client(&url, "POST", Some(body)).await?;
    invalidate_room_related_caches();
    Ok(())
}
