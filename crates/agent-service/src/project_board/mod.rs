//! Shared human work items. A separate pool never queues behind Agent storage.
mod types;
use crate::{Error, Result, domain::DomainStore, store::Principal};
use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Jsonb, Nullable, Text},
};
use diesel_async::{
    AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection,
    pooled_connection::{AsyncDieselConnectionManager, deadpool::Pool},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
pub use types::*;

#[derive(Clone)]
pub struct BoardStore {
    pool: Pool<AsyncPgConnection>,
}
/// Created only by the trusted Matrix gateway; never deserialized from a client.
pub(crate) struct Access {
    pub project: String,
    pub space: String,
    pub actor: String,
    pub members: BTreeSet<String>,
    pub observed: i64,
}
#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type=Jsonb)]
    value: Value,
}
#[derive(diesel::QueryableByName)]
struct Number {
    #[diesel(sql_type=BigInt)]
    value: i64,
}
#[derive(diesel::QueryableByName)]
struct Flag {
    #[diesel(sql_type=Bool)]
    value: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub(crate) enum Mutation {
    Create(Create),
    Update { task: String, input: Update },
    Move { task: String, input: Move },
    Archive { task: String, input: Versioned },
    Restore { task: String, input: Versioned },
    Comment { task: String, input: Comment },
}
impl Mutation {
    fn intent(&self) -> &str {
        match self {
            Self::Create(i) => &i.idempotency_key,
            Self::Update { input, .. } => &input.idempotency_key,
            Self::Move { input, .. } => &input.idempotency_key,
            Self::Archive { input, .. } | Self::Restore { input, .. } => &input.idempotency_key,
            Self::Comment { input, .. } => &input.idempotency_key,
        }
    }
    fn task(&self) -> Option<&str> {
        match self {
            Self::Create(_) => None,
            Self::Update { task, .. }
            | Self::Move { task, .. }
            | Self::Archive { task, .. }
            | Self::Restore { task, .. }
            | Self::Comment { task, .. } => Some(task),
        }
    }
    fn expected(&self) -> Option<i64> {
        match self {
            Self::Update { input, .. } => Some(input.expected_version),
            Self::Move { input, .. } => Some(input.expected_version),
            Self::Archive { input, .. } | Self::Restore { input, .. } => {
                Some(input.expected_version)
            }
            _ => None,
        }
    }
    fn kind(&self) -> &str {
        match self {
            Self::Create(_) => "created",
            Self::Update { .. } => "edited",
            Self::Move { .. } => "moved",
            Self::Archive { .. } => "archived",
            Self::Restore { .. } => "restored",
            Self::Comment { .. } => "commented",
        }
    }
    fn validate(&mut self) -> Result<()> {
        crate::key(self.intent())?;
        if let Some(task) = self.task() {
            crate::key(task)?;
        }
        if self.expected().is_some_and(|n| n < 1) {
            return Err(Error::Invalid("invalid_board_version"));
        }
        match self {
            Self::Create(i) => {
                i.fields.validate()?;
                state(&i.state)?;
            }
            Self::Update { input, .. } => input.fields.validate()?,
            Self::Move { task, input } => {
                state(&input.state)?;
                for id in [&input.before, &input.after].into_iter().flatten() {
                    crate::key(id)?;
                    if id == task {
                        return Err(Error::Invalid("invalid_board_neighbours"));
                    }
                }
                if input.position == Position::Between {
                    if input.expected_sequence.is_none_or(|s| s < 0)
                        || input.before.is_none() && input.after.is_none()
                        || input.before.is_some() && input.before == input.after
                    {
                        return Err(Error::Invalid("invalid_board_neighbours"));
                    }
                } else if input.before.is_some() || input.after.is_some() {
                    return Err(Error::Invalid("invalid_board_neighbours"));
                }
            }
            Self::Comment { input, .. } => {
                input.body = input.body.trim().into();
                if input.body.is_empty() || input.body.len() > 8192 {
                    return Err(Error::Invalid("invalid_board_comment"));
                }
            }
            _ => (),
        }
        Ok(())
    }
}
fn state(value: &str) -> Result<()> {
    if STATES.contains(&value) {
        Ok(())
    } else {
        Err(Error::Invalid("invalid_board_state"))
    }
}
const TASK_JSON: &str = "jsonb_build_object('id',t.id,'projectId',t.project_id,'number',t.number,'title',t.title,'description',t.description,'state',t.state_key,'rank',t.rank,'priority',t.priority,'assigneeMxid',t.assignee_mxid,'labels',t.labels,'dueDate',t.due_date::text,'version',t.version,'createdBy',t.created_by,'createdAt',t.created_at,'updatedAt',t.updated_at,'archivedAt',t.archived_at)";
const FILTER: &str = "t.project_id=$1 AND ($2::text IS NULL OR t.state_key=$2) AND ($3::text IS NULL OR t.assignee_mxid=$3) AND ($4::text IS NULL OR t.labels ? $4) AND ($5::text IS NULL OR position(lower($5) in lower(t.title))>0) AND (NOT $6 OR (t.due_date<(CURRENT_TIMESTAMP AT TIME ZONE 'UTC')::date AND t.state_key<>'done')) AND ((t.archived_at IS NOT NULL)=$7)";

impl BoardStore {
    pub async fn open(url: &str) -> Result<Self> {
        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
        let pool = Pool::builder(manager)
            .max_size(4)
            .build()
            .map_err(|_| Error::Unavailable("board_storage_unavailable"))?;
        let this = Self { pool };
        let mut db = this.connection().await?;
        db.transaction::<_,Error,_>(async |db|{
            db.batch_execute("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='8s'; SELECT pg_advisory_xact_lock(5210750088328915)").await?;
            let exists=sql_query("SELECT to_regclass('hagency_project_v1.migrations') IS NOT NULL AS value").get_result::<Flag>(db).await?.value;
            if exists {
                let version=sql_query("SELECT coalesce(max(version),0)::bigint AS value FROM hagency_project_v1.migrations").get_result::<Number>(db).await?.value;
                if version!=1{return Err(Error::Conflict("board_schema_incompatible"));}
                let complete=sql_query("SELECT to_regclass('hagency_project_v1.boards') IS NOT NULL AND to_regclass('hagency_project_v1.tasks') IS NOT NULL AND to_regclass('hagency_project_v1.comments') IS NOT NULL AND to_regclass('hagency_project_v1.changes') IS NOT NULL AND to_regclass('hagency_project_v1.receipts') IS NOT NULL AS value").get_result::<Flag>(db).await?.value;
                if !complete{return Err(Error::Conflict("board_schema_incompatible"));}
                return Ok(());
            }
            db.batch_execute(include_str!("schema.sql")).await?;
            Ok(())
        }).await?;
        drop(db);
        Ok(this)
    }
    async fn connection(
        &self,
    ) -> Result<diesel_async::pooled_connection::deadpool::Object<AsyncPgConnection>> {
        tokio::time::timeout(std::time::Duration::from_secs(8), self.pool.get())
            .await
            .map_err(|_| Error::Unavailable("board_storage_busy"))?
            .map_err(|_| Error::Unavailable("board_storage_unavailable"))
    }
    async fn authorize(db: &mut AsyncPgConnection, p: &Principal, a: &Access) -> Result<()> {
        db.batch_execute("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='8s'")
            .await?;
        DomainStore::authorize_identity(db, p, crate::api::now_ms()).await?;
        let project=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.projects WHERE id=$1 AND space_id=$2 AND active FOR SHARE) AS value")
            .bind::<Text,_>(&a.project).bind::<Text,_>(&a.space).get_result::<Flag>(db).await?.value;
        if !project || a.actor != p.mxid || !a.members.contains(&p.mxid) {
            return Err(Error::Unauthorized("project_membership_required"));
        }
        Self::fresh(a)?;
        Ok(())
    }
    fn fresh(a: &Access) -> Result<()> {
        let age = crate::api::now_ms().saturating_sub(a.observed);
        if !(0..=30_000).contains(&age) {
            return Err(Error::Unavailable("board_membership_expired"));
        }
        Ok(())
    }
    async fn assignee(db: &mut AsyncPgConnection, a: &Access, fields: &Fields) -> Result<()> {
        if let Some(mxid) = &fields.assignee_mxid {
            if !a.members.contains(mxid) {
                return Err(Error::Invalid("board_assignee_not_member"));
            }
            let puppet=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE puppet_mxid=$1) AS value").bind::<Text,_>(mxid).get_result::<Flag>(db).await?.value;
            if puppet {
                return Err(Error::Invalid("board_human_assignee_required"));
            }
        }
        Ok(())
    }
    async fn sequence(db: &mut AsyncPgConnection, project: &str) -> Result<i64> {
        Ok(sql_query("SELECT coalesce((SELECT change_seq FROM hagency_project_v1.boards WHERE project_id=$1),0) AS value").bind::<Text,_>(project).get_result::<Number>(db).await?.value)
    }
    async fn task(db: &mut AsyncPgConnection, project: &str, id: &str) -> Result<Value> {
        sql_query(format!("SELECT {TASK_JSON} AS value FROM hagency_project_v1.tasks t WHERE t.project_id=$1 AND t.id=$2"))
            .bind::<Text,_>(project).bind::<Text,_>(id).get_result::<Row>(db).await.map(|r|r.value).map_err(|e|match e{diesel::result::Error::NotFound=>Error::NotFound("board_task_not_found"),_=>e.into()})
    }
    fn cursor(project: &str, q: &Query, seq: i64, last: &str) -> String {
        let digest = crate::hash(&format!(
            "{}:{:?}:{:?}:{:?}:{:?}:{}:{}",
            project, q.state, q.assignee, q.label, q.query, q.overdue, q.archived
        ));
        format!("{last}_{seq}_{}", &digest[..16])
    }
    fn cursor_id(project: &str, q: &Query, seq: i64) -> Result<Option<String>> {
        let Some(cursor) = &q.cursor else {
            return Ok(None);
        };
        if q.sequence != Some(seq) {
            return Err(Error::Conflict("board_snapshot_changed"));
        }
        let mut parts = cursor.rsplitn(3, '_');
        let _digest = parts.next();
        let _seq = parts.next();
        let id = parts.next().ok_or(Error::Invalid("invalid_board_cursor"))?;
        crate::key(id)?;
        if *cursor != Self::cursor(project, q, seq, id) {
            return Err(Error::Invalid("invalid_board_cursor"));
        }
        Ok(Some(id.into()))
    }
    async fn page(db: &mut AsyncPgConnection, project: &str, q: &Query, seq: i64) -> Result<Value> {
        let cursor = Self::cursor_id(project, q, seq)?;
        let limit = q.limit.unwrap_or(50);
        let count = sql_query(format!(
            "SELECT count(*)::bigint AS value FROM hagency_project_v1.tasks t WHERE {FILTER}"
        ))
        .bind::<Text, _>(project)
        .bind::<Nullable<Text>, _>(&q.state)
        .bind::<Nullable<Text>, _>(&q.assignee)
        .bind::<Nullable<Text>, _>(&q.label)
        .bind::<Nullable<Text>, _>(&q.query)
        .bind::<Bool, _>(q.overdue)
        .bind::<Bool, _>(q.archived)
        .get_result::<Number>(db)
        .await?
        .value;
        let rows=sql_query(format!("SELECT {TASK_JSON}-'description' AS value FROM hagency_project_v1.tasks t WHERE {FILTER} AND ($8::text IS NULL OR (t.rank,t.id)>(SELECT c.rank,c.id FROM hagency_project_v1.tasks c WHERE c.project_id=$1 AND c.id=$8)) ORDER BY t.rank,t.id LIMIT $9"))
            .bind::<Text,_>(project).bind::<Nullable<Text>,_>(&q.state).bind::<Nullable<Text>,_>(&q.assignee).bind::<Nullable<Text>,_>(&q.label).bind::<Nullable<Text>,_>(&q.query).bind::<Bool,_>(q.overdue).bind::<Bool,_>(q.archived).bind::<Nullable<Text>,_>(cursor).bind::<BigInt,_>(limit+1).load::<Row>(db).await?;
        let more = rows.len() > limit as usize;
        let tasks: Vec<_> = rows
            .into_iter()
            .take(limit as usize)
            .map(|r| r.value)
            .collect();
        let next = if more {
            tasks
                .last()
                .and_then(|t| t["id"].as_str())
                .map(|id| Self::cursor(project, q, seq, id))
        } else {
            None
        };
        Ok(json!({"tasks":tasks,"total":count,"nextCursor":next,"sequence":seq}))
    }
    pub(crate) async fn board(&self, p: &Principal, a: &Access, q: Query) -> Result<Value> {
        q.validate()?;
        if q.cursor.is_some() {
            return Err(Error::Invalid("invalid_board_query"));
        }
        let mut db = self.connection().await?;
        db.build_transaction().repeatable_read().run::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            let seq=Self::sequence(db,&a.project).await?;
            let mut lanes=Vec::new();
            for name in STATES {
                if q.state.as_ref().is_some_and(|state|state!=name){lanes.push(json!({"state":name,"tasks":[],"total":0,"nextCursor":null,"sequence":seq}));continue;}
                let mut query=q.clone();query.state=Some(name.into());
                let mut page=Self::page(db,&a.project,&query,seq).await?;
                page["state"]=json!(name);lanes.push(page);
            }
            let members:Vec<_>=a.members.iter().filter(|m|!m.starts_with("@_hagency_")).take(1000).collect();
            Self::fresh(a)?;
            Ok(json!({"projectId":a.project,"sequence":seq,"lanes":lanes,"members":members,"capability":CAPABILITY}))
        }).await
    }
    pub(crate) async fn tasks(&self, p: &Principal, a: &Access, q: Query) -> Result<Value> {
        q.validate()?;
        let mut db = self.connection().await?;
        db.build_transaction()
            .repeatable_read()
            .run::<_, Error, _>(async |db| {
                Self::authorize(db, p, a).await?;
                let seq = Self::sequence(db, &a.project).await?;
                let result = Self::page(db, &a.project, &q, seq).await?;
                Self::fresh(a)?;
                Ok(result)
            })
            .await
    }
    pub(crate) async fn detail(&self, p: &Principal, a: &Access, id: &str) -> Result<Value> {
        crate::key(id)?;
        let mut db = self.connection().await?;
        db.transaction::<_, Error, _>(async |db| {
            Self::authorize(db, p, a).await?;
            let task = Self::task(db, &a.project, id).await?;
            Self::fresh(a)?;
            Ok(json!({"task":task}))
        })
        .await
    }
    pub(crate) async fn history(
        &self,
        p: &Principal,
        a: &Access,
        id: Option<&str>,
        after: i64,
        limit: i64,
        comments: bool,
    ) -> Result<Value> {
        if after < 0 || !(1..=100).contains(&limit) {
            return Err(Error::Invalid("invalid_board_query"));
        }
        if let Some(id) = id {
            crate::key(id)?;
        }
        let mut db = self.connection().await?;
        db.build_transaction().repeatable_read().run::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            if let Some(id)=id{Self::task(db,&a.project,id).await?;}
            let seq=Self::sequence(db,&a.project).await?;
            if !comments && after>seq{return Err(Error::Conflict("board_cursor_reset"));}
            let query=if comments {
                "SELECT jsonb_build_object('id',id,'author',author,'body',body,'createdAt',created_at,'cursor',seq) AS value FROM hagency_project_v1.comments WHERE project_id=$1 AND task_id=$2 AND seq>$3 ORDER BY seq LIMIT $4"
            }else{
                "SELECT jsonb_build_object('sequence',seq,'taskId',task_id,'actor',actor,'kind',kind,'occurredAt',occurred_at) AS value FROM hagency_project_v1.changes WHERE project_id=$1 AND ($2::text IS NULL OR task_id=$2) AND seq>$3 ORDER BY seq LIMIT $4"
            };
            let rows=sql_query(query).bind::<Text,_>(&a.project).bind::<Nullable<Text>,_>(id).bind::<BigInt,_>(after).bind::<BigInt,_>(limit+1).load::<Row>(db).await?;
            let more=rows.len()>limit as usize;
            let items:Vec<_>=rows.into_iter().take(limit as usize).map(|r|r.value).collect();
            let next=items.last().and_then(|v|v[if comments{"cursor"}else{"sequence"}].as_i64()).unwrap_or(after);
            Self::fresh(a)?;
            Ok(json!({"items":items,"next":next,"hasMore":more,"sequence":seq}))
        }).await
    }
    async fn rank(
        db: &mut AsyncPgConnection,
        project: &str,
        state: &str,
        id: &str,
        movement: Option<&Move>,
    ) -> Result<i64> {
        for attempt in 0..2 {
            let rows=sql_query("SELECT jsonb_build_object('id',id,'rank',rank) AS value FROM hagency_project_v1.tasks WHERE project_id=$1 AND state_key=$2 AND archived_at IS NULL AND id<>$3 ORDER BY rank,id LIMIT 10001")
                .bind::<Text,_>(project).bind::<Text,_>(state).bind::<Text,_>(id).load::<Row>(db).await?;
            if rows.len() > 10000 {
                return Err(Error::Conflict("board_capacity_exceeded"));
            }
            let index = match movement.map(|m| m.position).unwrap_or_default() {
                Position::Append => rows.len(),
                Position::Prepend => 0,
                Position::Between => {
                    let m = movement.unwrap();
                    let index = if let Some(before) = &m.before {
                        rows.iter()
                            .position(|r| r.value["id"] == *before)
                            .ok_or(Error::Conflict("board_neighbours_changed"))?
                    } else {
                        rows.len()
                    };
                    let prev = index
                        .checked_sub(1)
                        .map(|i| rows[i].value["id"].as_str().unwrap());
                    if prev != m.after.as_deref() {
                        return Err(Error::Conflict("board_neighbours_changed"));
                    }
                    index
                }
            };
            let low = index
                .checked_sub(1)
                .map(|i| rows[i].value["rank"].as_i64().unwrap())
                .unwrap_or(0);
            let high = rows
                .get(index)
                .and_then(|r| r.value["rank"].as_i64())
                .or_else(|| low.checked_add(2048));
            if let Some(high) = high
                && high - low > 1
            {
                return Ok(low + (high - low) / 2);
            }
            if attempt == 0 {
                sql_query("WITH ranked AS (SELECT id,row_number() OVER(ORDER BY rank,id)*1024 AS new_rank FROM hagency_project_v1.tasks WHERE project_id=$1 AND state_key=$2 AND archived_at IS NULL AND id<>$3) UPDATE hagency_project_v1.tasks t SET rank=r.new_rank FROM ranked r WHERE t.id=r.id")
                    .bind::<Text,_>(project).bind::<Text,_>(state).bind::<Text,_>(id).execute(db).await?;
            }
        }
        Err(Error::Conflict("board_rank_exhausted"))
    }
    pub(crate) async fn mutate(
        &self,
        p: &Principal,
        a: &Access,
        mut mutation: Mutation,
    ) -> Result<Value> {
        mutation.validate()?;
        let digest = crate::hash(
            &serde_json::to_string(&mutation)
                .map_err(|_| Error::Invalid("invalid_board_payload"))?,
        );
        let mut db = self.connection().await?;
        db.transaction::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            sql_query("INSERT INTO hagency_project_v1.boards(project_id) VALUES($1) ON CONFLICT DO NOTHING").bind::<Text,_>(&a.project).execute(db).await?;
            let seq=sql_query("SELECT change_seq AS value FROM hagency_project_v1.boards WHERE project_id=$1 FOR UPDATE").bind::<Text,_>(&a.project).get_result::<Number>(db).await?.value;
            DomainStore::authorize_identity(db,p,crate::api::now_ms()).await?;Self::fresh(a)?;
            let replay=sql_query("SELECT jsonb_build_object('digest',digest,'result',result) AS value FROM hagency_project_v1.receipts WHERE project_id=$1 AND actor=$2 AND intent_key=$3")
                .bind::<Text,_>(&a.project).bind::<Text,_>(&p.mxid).bind::<Text,_>(mutation.intent()).get_result::<Row>(db).await;
            match replay {
                Ok(row)=>{if row.value["digest"]!=digest{return Err(Error::Conflict("board_idempotency_conflict"));}let mut result=row.value["result"].clone();result["replayed"]=json!(true);return Ok(result);}
                Err(diesel::result::Error::NotFound)=>(),Err(e)=>return Err(e.into())
            }
            let task=if let Some(id)=mutation.task(){Some(Self::task(db,&a.project,id).await?)}else{None};
            if let Some(expected)=mutation.expected() && task.as_ref().and_then(|t|t["version"].as_i64())!=Some(expected){return Err(Error::Conflict("board_version_conflict"));}
            if let Some(task)=&task {
                let archived=!task["archivedAt"].is_null();
                if archived&&!matches!(mutation,Mutation::Restore{..}) || !archived&&matches!(mutation,Mutation::Restore{..}){return Err(Error::Conflict("board_archive_conflict"));}
            }
            let now=crate::api::now_ms();
            let mut comment=Value::Null;
            let id=match &mutation {
                Mutation::Create(input)=>{
                    Self::assignee(db,a,&input.fields).await?;
                    let count=sql_query("SELECT count(*)::bigint AS value FROM hagency_project_v1.tasks WHERE project_id=$1 AND archived_at IS NULL").bind::<Text,_>(&a.project).get_result::<Number>(db).await?.value;
                    if count>=10000{return Err(Error::Conflict("board_capacity_exceeded"));}
                    let id=format!("tsk_{}",crate::entity_id()?);
                    let rank=Self::rank(db,&a.project,&input.state,&id,None).await?;
                    let number=sql_query("UPDATE hagency_project_v1.boards SET next_number=next_number+1 WHERE project_id=$1 RETURNING next_number-1 AS value").bind::<Text,_>(&a.project).get_result::<Number>(db).await?.value;
                    let f=&input.fields;
                    sql_query("INSERT INTO hagency_project_v1.tasks(id,project_id,number,title,description,state_key,rank,priority,assignee_mxid,labels,due_date,created_by,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::text::date,$12,$13,$13)")
                        .bind::<Text,_>(&id).bind::<Text,_>(&a.project).bind::<BigInt,_>(number).bind::<Text,_>(&f.title).bind::<Text,_>(&f.description).bind::<Text,_>(&input.state).bind::<BigInt,_>(rank).bind::<Text,_>(&f.priority).bind::<Nullable<Text>,_>(&f.assignee_mxid).bind::<Jsonb,_>(json!(f.labels)).bind::<Nullable<Text>,_>(&f.due_date).bind::<Text,_>(&p.mxid).bind::<BigInt,_>(now).execute(db).await?;
                    id
                }
                Mutation::Update{task,input}=>{
                    Self::assignee(db,a,&input.fields).await?;let f=&input.fields;
                    sql_query("UPDATE hagency_project_v1.tasks SET title=$3,description=$4,priority=$5,assignee_mxid=$6,labels=$7,due_date=$8::text::date,version=version+1,updated_at=$9 WHERE project_id=$1 AND id=$2")
                        .bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<Text,_>(&f.title).bind::<Text,_>(&f.description).bind::<Text,_>(&f.priority).bind::<Nullable<Text>,_>(&f.assignee_mxid).bind::<Jsonb,_>(json!(f.labels)).bind::<Nullable<Text>,_>(&f.due_date).bind::<BigInt,_>(now).execute(db).await?;
                    task.clone()
                }
                Mutation::Move{task,input}=>{
                    if input.position==Position::Between && input.expected_sequence!=Some(seq){return Err(Error::Conflict("board_snapshot_changed"));}
                    let rank=Self::rank(db,&a.project,&input.state,task,Some(input)).await?;
                    sql_query("UPDATE hagency_project_v1.tasks SET state_key=$3,rank=$4,version=version+1,updated_at=$5 WHERE project_id=$1 AND id=$2")
                        .bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<Text,_>(&input.state).bind::<BigInt,_>(rank).bind::<BigInt,_>(now).execute(db).await?;task.clone()
                }
                Mutation::Archive{task,..}=>{
                    sql_query("UPDATE hagency_project_v1.tasks SET archived_at=$3,updated_at=$3,version=version+1 WHERE project_id=$1 AND id=$2").bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<BigInt,_>(now).execute(db).await?;task.clone()
                }
                Mutation::Restore{task:id,..}=>{
                    let count=sql_query("SELECT count(*)::bigint AS value FROM hagency_project_v1.tasks WHERE project_id=$1 AND archived_at IS NULL").bind::<Text,_>(&a.project).get_result::<Number>(db).await?.value;
                    if count>=10000{return Err(Error::Conflict("board_capacity_exceeded"));}
                    let state=task.as_ref().unwrap()["state"].as_str().unwrap();
                    let rank=Self::rank(db,&a.project,state,id,None).await?;
                    sql_query("UPDATE hagency_project_v1.tasks SET archived_at=NULL,rank=$3,updated_at=$4,version=version+1 WHERE project_id=$1 AND id=$2").bind::<Text,_>(&a.project).bind::<Text,_>(id).bind::<BigInt,_>(rank).bind::<BigInt,_>(now).execute(db).await?;id.clone()
                }
                Mutation::Comment{task,input}=>{
                    let id=format!("cmt_{}",crate::entity_id()?);
                    sql_query("INSERT INTO hagency_project_v1.comments(id,project_id,task_id,author,body,created_at) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&id).bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<Text,_>(&p.mxid).bind::<Text,_>(&input.body).bind::<BigInt,_>(now).execute(db).await?;
                    comment=json!({"id":id,"author":p.mxid,"body":input.body,"createdAt":now});task.clone()
                }
            };
            let seq=sql_query("UPDATE hagency_project_v1.boards SET change_seq=change_seq+1 WHERE project_id=$1 RETURNING change_seq AS value").bind::<Text,_>(&a.project).get_result::<Number>(db).await?.value;
            sql_query("INSERT INTO hagency_project_v1.changes(project_id,seq,task_id,actor,kind,occurred_at) VALUES($1,$2,$3,$4,$5,$6)").bind::<Text,_>(&a.project).bind::<BigInt,_>(seq).bind::<Text,_>(&id).bind::<Text,_>(&p.mxid).bind::<Text,_>(mutation.kind()).bind::<BigInt,_>(now).execute(db).await?;
            let result=json!({"task":Self::task(db,&a.project,&id).await?,"sequence":seq,"comment":comment,"replayed":false});
            sql_query("INSERT INTO hagency_project_v1.receipts(project_id,actor,intent_key,digest,result) VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(&a.project).bind::<Text,_>(&p.mxid).bind::<Text,_>(mutation.intent()).bind::<Text,_>(&digest).bind::<Jsonb,_>(&result).execute(db).await?;
            DomainStore::authorize_identity(db,p,crate::api::now_ms()).await?;Self::fresh(a)?;
            Ok(result)
        }).await
    }
}
#[cfg(test)]
mod tests;
