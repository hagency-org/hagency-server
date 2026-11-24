//! Member-only display metadata for Spaces whose project registry lives on another server.
use hagency_agent_service::domain::DomainStore;
use palpo::core::{MatrixError, OwnedRoomId, OwnedUserId, events::StateEventType};
use palpo::{AppError, AppResult, DepotExt, GetUrlOrigin};
use salvo::prelude::*;
use serde::{Deserialize, Serialize};

const FEDERATION_PATH: &str = "hagency/api/v1/federation/space-kind";

#[derive(Serialize, Deserialize)]
struct SpaceKind {
    agent: bool,
}

pub fn router(store: DomainStore, public: url::Url) -> Router {
    Router::new()
        .hoop(no_store)
        .hoop(affix_state::inject(store))
        .push(
            Router::with_path("hagency/api/v1/space-kind")
                .hoop(affix_state::inject(public))
                .hoop(native_bearer)
                .hoop(palpo::hoops::auth_by_access_token_without_query_masquerade)
                .get(client_kind),
        )
        .push(
            Router::with_path(FEDERATION_PATH)
                .hoop(palpo::hoops::auth_by_signatures)
                .get(federation_kind),
        )
}

#[handler]
async fn no_store(res: &mut Response) {
    res.add_header("cache-control", "no-store", true).unwrap();
}

#[handler]
async fn native_bearer(req: &mut Request, depot: &mut Depot) -> AppResult<()> {
    let public = depot
        .get_typed::<url::Url>()
        .map_err(|_| StatusError::internal_server_error())?;
    hagency_agent_service::api::validate_native_request(req, public)
        .map_err(|_| StatusError::unauthorized())?;
    if !req.headers().contains_key("authorization") || req.queries().contains_key("access_token") {
        return Err(MatrixError::missing_token("Use the Authorization header").into());
    }
    Ok(())
}

fn hidden() -> AppError {
    MatrixError::not_found("Space unavailable").into()
}

// Preserve Palpo's Matrix status and error body at both HTTP entry points.
#[expect(clippy::result_large_err, reason = "Palpo exposes AppError by value")]
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

async fn registered(depot: &Depot, room: &OwnedRoomId) -> Result<bool, StatusError> {
    let store = depot
        .get_typed::<DomainStore>()
        .map_err(|_| StatusError::internal_server_error())?;
    store
        .has_active_project(room.as_str())
        .await
        .map_err(|_| StatusError::service_unavailable())
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
