//! Member-only display metadata for Spaces whose project registry lives on another server.
use hagency_agent_service::domain::DomainStore;
use palpo::core::{MatrixError, OwnedRoomId, OwnedUserId, events::StateEventType};
use palpo::{AppError, AppResult, DepotExt, GetUrlOrigin};
use salvo::prelude::*;
use serde::{Deserialize, Serialize};

const FEDERATION_PATH: &str = "_matrix/federation/unstable/im.hagency/space_kind";

#[derive(Serialize, Deserialize)]
struct SpaceKind {
    agent: bool,
}

pub fn router(store: DomainStore) -> Router {
    Router::new()
        .hoop(affix_state::inject(store))
        .push(
            Router::with_path("_matrix/client/unstable/im.hagency/space_kind")
                .hoop(palpo::hoops::auth_by_access_token)
                .get(client_kind),
        )
        .push(
            Router::with_path(FEDERATION_PATH)
                .hoop(palpo::hoops::auth_by_signatures)
                .get(federation_kind),
        )
}

fn hidden() -> AppError {
    MatrixError::not_found("Space unavailable").into()
}

async fn member_space(
    room: &OwnedRoomId,
    viewer: &OwnedUserId,
) -> AppResult<palpo::core::OwnedServerName> {
    if !palpo::room::user::is_joined(viewer, room).await? {
        return Err(hidden());
    }
    let create = palpo::room::get_state(room, &StateEventType::RoomCreate, "", None).await?;
    if create.get_content::<serde_json::Value>()?["type"] != "m.space" {
        return Err(hidden());
    }
    Ok(create.sender.server_name().to_owned())
}

async fn registered(depot: &Depot, room: &OwnedRoomId) -> AppResult<bool> {
    let store = depot
        .obtain::<DomainStore>()
        .map_err(|_| StatusError::internal_server_error())?;
    store
        .has_active_project(room.as_str())
        .await
        .map_err(|_| StatusError::service_unavailable().into())
}

#[handler]
async fn federation_kind(req: &mut Request, depot: &mut Depot) -> AppResult<Json<SpaceKind>> {
    let room: OwnedRoomId = req.query("room").ok_or_else(hidden)?;
    let viewer: OwnedUserId = req.query("viewer").ok_or_else(hidden)?;
    let origin = depot.origin()?;
    if palpo::config::get().enabled_federation().is_none() || origin != viewer.server_name() {
        return Err(hidden());
    }
    palpo::event::handler::acl_check(origin, &room).await?;
    if member_space(&room, &viewer).await? != palpo::config::get().server_name {
        return Err(hidden());
    }
    Ok(Json(SpaceKind {
        agent: registered(depot, &room).await?,
    }))
}

#[handler]
async fn client_kind(req: &mut Request, depot: &mut Depot) -> AppResult<Json<SpaceKind>> {
    let room: OwnedRoomId = req.query("room").ok_or_else(hidden)?;
    let viewer = depot.authed_info()?.user_id().to_owned();
    let authority = member_space(&room, &viewer).await?;
    let local = registered(depot, &room).await?;
    if local || authority == palpo::config::get().server_name {
        return Ok(Json(SpaceKind { agent: local }));
    }
    let unavailable = || StatusError::service_unavailable();
    let config = palpo::config::get();
    if config.enabled_federation().is_none() || !config.federation.is_server_allowed(&authority) {
        return Err(unavailable().into());
    }
    palpo::event::handler::acl_check(&authority, &room).await?;
    let kind = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut url = url::Url::parse(&authority.origin().await).map_err(|_| unavailable())?;
        url.set_path(FEDERATION_PATH);
        url.query_pairs_mut()
            .append_pair("room", room.as_str())
            .append_pair("viewer", viewer.as_str());
        let request = reqwest::Request::new(reqwest::Method::GET, url);
        let mut response =
            palpo::sending::send_federation_request(&authority, request, Some(5)).await?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if chunk.len() > 128 - body.len() {
                return Err(unavailable().into());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice::<SpaceKind>(&body).map_err(|_| AppError::from(unavailable()))
    })
    .await
    .map_err(|_| unavailable())??;
    if !palpo::room::user::is_joined(&viewer, &room).await? {
        return Err(hidden());
    }
    Ok(Json(kind))
}
