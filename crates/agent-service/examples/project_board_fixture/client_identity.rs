//! Explicit identity/sync test doubles for the complete desktop client.
//! Only mounted by the disposable loopback example, never the production API.
use salvo::prelude::*;
use serde_json::json;
#[derive(Clone)]
struct Origin(String);

#[handler]
async fn metadata(depot: &Depot, res: &mut Response) {
    let origin = &depot.get_typed::<Origin>().unwrap().0;
    res.render(Json(json!({
        "issuer":format!("{origin}auth/"),
        "authorization_endpoint":format!("{origin}auth/authorize"),
        "token_endpoint":format!("{origin}auth/token"),
        "revocation_endpoint":format!("{origin}auth/revoke"),
        "response_types_supported":["code"], "response_modes_supported":["query","fragment"],
        "grant_types_supported":["authorization_code","refresh_token"],
        "code_challenge_methods_supported":["S256"]
    })));
}
#[handler]
async fn versions(res: &mut Response) {
    res.render(Json(
        json!({"versions":["v1.11"],"unstable_features":{"org.matrix.simplified_msc3575":true}}),
    ));
}
#[handler]
async fn identity_guard(req: &mut Request, res: &mut Response, ctrl: &mut FlowCtrl) {
    let valid = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(super::actor)
        .is_some();
    if !valid {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(
            json!({"errcode":"M_UNKNOWN_TOKEN","error":"fixture identity required"}),
        ));
        ctrl.skip_rest();
    }
}
#[handler]
async fn sync(req: &mut Request, res: &mut Response) {
    // Empty Matrix room list is intentional: Projects are read from the actual
    // Server. Never manufacture a successful chat/messaging response.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let input: serde_json::Value = req.parse_json().await.unwrap_or_default();
    let lists: serde_json::Map<String, serde_json::Value> = input["lists"]
        .as_object()
        .map(|lists| {
            lists
                .keys()
                .map(|name| (name.clone(), json!({"count":0,"ops":[]})))
                .collect()
        })
        .unwrap_or_default();
    let mut reply = json!({"pos":format!("fixture-{}",hagency_agent_service::api::now_ms()),"lists":lists,"rooms":{},"extensions":{}});
    if let Some(txn) = input.get("txn_id") {
        reply["txn_id"] = txn.clone();
    }
    res.render(Json(reply));
}
#[handler]
async fn keys_upload(res: &mut Response) {
    res.render(Json(
        json!({"one_time_key_counts":{"signed_curve25519":50}}),
    ));
}
#[handler]
async fn keys_query(res: &mut Response) {
    res.render(Json(json!({"device_keys":{},"failures":{}})));
}
#[handler]
async fn profile(res: &mut Response) {
    res.render(Json(json!({"displayname":"Alice · 本地联调"})));
}
#[handler]
async fn missing(res: &mut Response) {
    res.status_code(StatusCode::NOT_FOUND);
    res.render(Json(
        json!({"errcode":"M_NOT_FOUND","error":"not provided by the explicit identity fixture"}),
    ));
}
pub fn router(origin: &str) -> Router {
    Router::new()
        .hoop(affix_state::inject(Origin(origin.into())))
        .push(Router::with_path("_matrix/client/versions").get(versions))
        .push(
            Router::with_path("_matrix/client/unstable/org.matrix.msc2965/auth_metadata")
                .get(metadata),
        )
        .push(Router::with_path("_matrix/client/v1/auth_metadata").get(metadata))
        .push(
            Router::new()
                .hoop(identity_guard)
                .push(
                    Router::with_path("_matrix/client/unstable/org.matrix.simplified_msc3575/sync")
                        .post(sync),
                )
                .push(Router::with_path("_matrix/client/v3/keys/upload").post(keys_upload))
                .push(Router::with_path("_matrix/client/v3/keys/query").post(keys_query))
                .push(Router::with_path("_matrix/client/v3/profile/{user}").get(profile))
                .push(Router::with_path("_matrix/{**rest}").goal(missing)),
        )
}
