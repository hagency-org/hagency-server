//! Explicit native-only routes. Matrix membership is refreshed before every call.
use crate::{
    Error, Result,
    domain::DomainStore,
    gateway::Gateway,
    project_board::{Access, Mutation, Query},
    store::Store,
};
use salvo::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeSet;

async fn body<T: DeserializeOwned>(req: &mut Request) -> Result<T> {
    let limit = if req.uri().path().ends_with("/attachments") {
        3 * 1024 * 1024
    } else {
        131072
    };
    req.set_secure_max_size(limit);
    let bytes = req
        .payload()
        .await
        .map_err(|_| Error::Invalid("invalid_board_payload"))?;
    if bytes.len() > limit {
        return Err(Error::Invalid("invalid_board_payload"));
    }
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid("invalid_board_payload"))
}
fn query(req: &Request) -> Result<Query> {
    let mut value = serde_json::Map::new();
    for (key, val) in url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()) {
        let val = match key.as_ref() {
            "overdue" | "archived" => Value::Bool(match val.as_ref() {
                "true" => true,
                "false" => false,
                _ => return Err(Error::Invalid("invalid_board_query")),
            }),
            "sequence" | "limit" => Value::Number(
                val.parse::<i64>()
                    .map_err(|_| Error::Invalid("invalid_board_query"))?
                    .into(),
            ),
            _ => Value::String(val.into_owned()),
        };
        if value.insert(key.into_owned(), val).is_some() {
            return Err(Error::Invalid("invalid_board_query"));
        }
    }
    serde_json::from_value(Value::Object(value)).map_err(|_| Error::Invalid("invalid_board_query"))
}
fn history_query(req: &Request) -> Result<(i64, i64)> {
    let mut after = 0;
    let mut limit = 50;
    let mut seen = BTreeSet::new();
    for (key, value) in url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()) {
        if !seen.insert(key.to_string()) {
            return Err(Error::Invalid("invalid_board_query"));
        }
        let value = value
            .parse::<i64>()
            .map_err(|_| Error::Invalid("invalid_board_query"))?;
        match key.as_ref() {
            "after" => after = value,
            "limit" => limit = value,
            _ => return Err(Error::Invalid("invalid_board_query")),
        }
    }
    Ok((after, limit))
}
pub(crate) async fn call(
    req: &mut Request,
    store: &Store,
    domain: &DomainStore,
    gateway: &Gateway,
    credential: &str,
) -> Result<Option<Value>> {
    let path = req.uri().path().to_owned();
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() < 7 || parts[4] != "projects" || !["board", "tasks", "boards"].contains(&parts[6]) {
        return Ok(None);
    }
    crate::key(parts[5])?;
    let principal = store
        .authenticate(credential, crate::api::now_ms(), false)
        .await?;
    let project = domain
        .project(&principal, parts[5], crate::api::now_ms())
        .await
        .map_err(|e| match e {
            Error::NotFound(_) => Error::Unauthorized("project_membership_required"),
            other => other,
        })?;
    let state = gateway.state(&project.space_id).await?;
    let members = state
        .iter()
        .filter(|e| e["type"] == "m.room.member" && e["content"]["membership"] == "join")
        .filter_map(|e| e["state_key"].as_str())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if !project.active || !members.contains(&principal.mxid) {
        return Err(Error::Unauthorized("project_membership_required"));
    }
    let member_names = state
        .iter()
        .filter(|e| e["type"] == "m.room.member" && e["content"]["membership"] == "join")
        .filter_map(|e| {
            Some((
                e["state_key"].as_str()?.to_owned(),
                e["content"]["displayname"].as_str()?.trim().to_owned(),
            ))
        })
        .filter(|(id, name)| {
            members.contains(id)
                && !name.is_empty()
                && name.chars().count() <= 256
                && !name.chars().any(char::is_control)
        })
        .collect();
    let mut access = Access {
        member_names,
        project: project.id.clone(),
        space: project.space_id,
        actor: principal.mxid.clone(),
        members,
        observed: crate::api::now_ms(),
    };
    let board = &domain.board;
    if parts.len()==7 && parts[6]=="boards" {
        return Ok(Some(match req.method().clone() {
            salvo::http::Method::GET => board.catalog(&principal,&access,None).await?,
            salvo::http::Method::POST => board.catalog(&principal,&access,Some(body(req).await?)).await?,
            _ => return Err(Error::Invalid("invalid_board_operation")),
        }));
    }
    let tail = if parts[6]=="boards" {
        if parts.len()<9 {return Err(Error::Invalid("invalid_board_operation"));}
        board.resolve_board(&project.id,parts[7]).await?;
        access.project=parts[7].into();
        &parts[8..]
    } else {&parts[6..]};
    let result = match (req.method().clone(), tail) {
        (salvo::http::Method::GET,["board","overview"]) if access.project==project.id => {
            let facts=gateway.admin(&access.space,&principal.mxid,None).await?;
            let mut result=board.overview(&principal,&access).await?;
            result["canManagePolicy"]=serde_json::json!(facts.joined && facts.is_space && facts.can_manage_policy);
            result["canCreateRoom"]=serde_json::json!(crate::gateway::can_create_room(&state,&principal.mxid)?);
            result["archived"]=serde_json::json!(project.archived);
            result["revision"]=serde_json::json!(project.revision);
            result
        }
        (salvo::http::Method::PUT,["board","project-archive"]) if access.project==project.id => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all="camelCase",deny_unknown_fields)]
            struct Archive {archived:bool,expected_revision:i64}
            let input:Archive=body(req).await?;
            let facts=gateway.admin(&access.space,&principal.mxid,None).await?;
            serde_json::json!({"project":domain.archive_project(&principal,&project.id,input.expected_revision,input.archived,&facts,crate::api::now_ms()).await?})
        }
        (salvo::http::Method::GET, ["board", "notifications"]) => {
            let (before, _) = history_query(req)?;
            board.notifications(&principal, &access, before).await?
        }
        (salvo::http::Method::POST, ["board", "notifications", "read"]) => {
            if req.uri().query().is_some() {
                return Err(Error::Invalid("invalid_board_query"));
            }
            board
                .read_notification(&principal, &access, body(req).await?)
                .await?
        }
        (salvo::http::Method::GET, ["tasks", task, "attachments", id]) => {
            if req.uri().query().is_some() {
                return Err(Error::Invalid("invalid_board_query"));
            }
            board.attachment(&principal, &access, task, id).await?
        }
        (salvo::http::Method::GET, ["board"]) => {
            board.board(&principal, &access, query(req)?).await?
        }
        (salvo::http::Method::GET, ["tasks"]) => {
            board.tasks(&principal, &access, query(req)?).await?
        }
        (salvo::http::Method::GET, ["tasks", id]) => {
            if req.uri().query().is_some() {
                return Err(Error::Invalid("invalid_board_query"));
            }
            board.detail(&principal, &access, id).await?
        }
        (salvo::http::Method::GET, ["board", "changes"]) => {
            let (after, limit) = history_query(req)?;
            board
                .history(&principal, &access, None, after, limit, false)
                .await?
        }
        (salvo::http::Method::GET, ["tasks", id, kind @ ("comments" | "activity")]) => {
            let (after, limit) = history_query(req)?;
            board
                .history(
                    &principal,
                    &access,
                    Some(id),
                    after,
                    limit,
                    *kind == "comments",
                )
                .await?
        }
        (method, tail) => {
            if req.uri().query().is_some() {
                return Err(Error::Invalid("invalid_board_query"));
            }
            let mutation = match (method, tail) {
                (salvo::http::Method::PATCH, ["board", "columns"]) => {
                    Mutation::ConfigureColumns(body(req).await?)
                }
                (salvo::http::Method::POST, ["tasks", id, "attachments"]) => {
                    Mutation::UploadAttachment {
                        task: (*id).into(),
                        input: body(req).await?,
                    }
                }
                (salvo::http::Method::POST, ["tasks", id, "attachments", "remove"]) => {
                    Mutation::RemoveAttachment {
                        task: (*id).into(),
                        input: body(req).await?,
                    }
                }
                (salvo::http::Method::POST, ["tasks"]) => Mutation::Create(body(req).await?),
                (salvo::http::Method::PATCH, ["tasks", id]) => Mutation::Update {
                    task: (*id).into(),
                    input: body(req).await?,
                },
                (salvo::http::Method::POST, ["tasks", id, "move"]) => Mutation::Move {
                    task: (*id).into(),
                    input: body(req).await?,
                },
                (salvo::http::Method::POST, ["tasks", id, "archive"]) => Mutation::Archive {
                    task: (*id).into(),
                    input: body(req).await?,
                },
                (salvo::http::Method::POST, ["tasks", id, "restore"]) => Mutation::Restore {
                    task: (*id).into(),
                    input: body(req).await?,
                },
                (salvo::http::Method::POST, ["tasks", id, "comments"]) => Mutation::Comment {
                    task: (*id).into(),
                    input: body(req).await?,
                },
                _ => return Err(Error::NotFound("not_found")),
            };
            board.mutate(&principal, &access, mutation).await?
        }
    };
    store
        .authenticate(credential, crate::api::now_ms(), false)
        .await?;
    Ok(Some(result))
}

pub(crate) fn matches(path: &str) -> bool {
    let p = path.split('/').collect::<Vec<_>>();
    p.len() >= 7 && p[4] == "projects" && ["board", "tasks", "boards"].contains(&p[6])
}
