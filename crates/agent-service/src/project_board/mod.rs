//! Shared human work items. A separate pool never queues behind Agent storage.
mod catalog;
pub(crate) use catalog::SetNavigationDefault;
mod collaboration;
mod columns;
mod types;
use crate::{Error, Result, domain::DomainStore, store::Principal};
use diesel::{BoolExpressionMethods, ExpressionMethods, QueryDsl};
use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Jsonb, Nullable, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
pub use types::*;

#[derive(Clone)]
pub struct BoardStore {
    pool: crate::database::Database,
}
/// Created only by the trusted Matrix gateway; never deserialized from a client.
pub(crate) struct Access {
    pub project: String,
    pub space: String,
    pub actor: String,
    pub members: BTreeSet<String>,
    pub member_names: std::collections::BTreeMap<String, String>,
    pub observed: i64,
}
#[derive(diesel::Queryable, diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type=Jsonb)]
    value: Value,
}
#[derive(diesel::Queryable, diesel::QueryableByName)]
struct Number {
    #[diesel(sql_type=BigInt)]
    value: i64,
}
#[derive(diesel::Queryable, diesel::QueryableByName)]
struct LaneCounts {
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    total: i64,
    #[diesel(sql_type=BigInt)]
    active: i64,
    #[diesel(sql_type=BigInt)]
    matching: i64,
}
#[derive(diesel::Queryable, diesel::QueryableByName)]
struct Flag {
    #[diesel(sql_type=Bool)]
    value: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub(crate) enum Mutation {
    UploadAttachment {
        task: String,
        input: AttachmentUpload,
    },
    RemoveAttachment {
        task: String,
        input: AttachmentRemove,
    },
    ConfigureColumns(ConfigureColumns),
    Create(Create),
    Update {
        task: String,
        input: Update,
    },
    Move {
        task: String,
        input: Move,
    },
    Archive {
        task: String,
        input: Versioned,
    },
    Restore {
        task: String,
        input: Versioned,
    },
    Comment {
        task: String,
        input: Comment,
    },
}
impl Mutation {
    fn intent(&self) -> &str {
        match self {
            Self::UploadAttachment { input, .. } => &input.idempotency_key,
            Self::RemoveAttachment { input, .. } => &input.idempotency_key,
            Self::ConfigureColumns(i) => &i.idempotency_key,
            Self::Create(i) => &i.idempotency_key,
            Self::Update { input, .. } => &input.idempotency_key,
            Self::Move { input, .. } => &input.idempotency_key,
            Self::Archive { input, .. } | Self::Restore { input, .. } => &input.idempotency_key,
            Self::Comment { input, .. } => &input.idempotency_key,
        }
    }
    fn task(&self) -> Option<&str> {
        match self {
            Self::Create(_) | Self::ConfigureColumns(_) => None,
            Self::UploadAttachment { task, .. }
            | Self::RemoveAttachment { task, .. }
            | Self::Update { task, .. }
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
            Self::UploadAttachment { .. } => "attachment_added",
            Self::RemoveAttachment { .. } => "attachment_removed",
            Self::ConfigureColumns(_) => "columns_updated",
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
            Self::UploadAttachment { input, .. } => {
                collaboration::attachment_bytes(input)?;
            }
            Self::RemoveAttachment { input, .. } => crate::key(&input.attachment)?,
            Self::ConfigureColumns(i) => i.validate()?,
            Self::Create(i) => {
                i.fields.validate()?;
                state(&i.state)?;
            }
            Self::Update { input, .. } => {
                input.fields.validate()?;
                if input
                    .state
                    .as_deref()
                    .is_some_and(|state| !types::valid_state(state))
                {
                    return Err(Error::Invalid("invalid_board_state"));
                }
            }
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
    if valid_state(value) {
        Ok(())
    } else {
        Err(Error::Invalid("invalid_board_state"))
    }
}
const TASK_SUMMARY_JSON: &str = "jsonb_build_object('id',t.id,'projectId',t.project_id,'number',t.number,'title',t.title,'state',t.state_key,'rank',t.rank,'priority',t.priority,'assigneeMxid',t.assignee_mxid,'labels',t.labels,'dueDate',t.due_date::text,'version',t.version,'createdBy',t.created_by,'createdAt',t.created_at,'updatedAt',t.updated_at,'archivedAt',t.archived_at,'parentId',t.parent_id,'parentNumber',(SELECT p.number FROM hagency_project_v1.tasks p WHERE p.id=t.parent_id AND p.project_id=t.project_id),'attachmentCount',(SELECT count(*) FROM hagency_project_v1.attachments f WHERE f.project_id=t.project_id AND f.task_id=t.id AND f.removed_at IS NULL),'commentCount',(SELECT count(*) FROM hagency_project_v1.comments c WHERE c.project_id=t.project_id AND c.task_id=t.id),'checklistTotal',jsonb_array_length(t.checklist),'checklistDone',(SELECT count(*) FROM jsonb_array_elements(t.checklist) i WHERE (i->>'checked')::boolean),'subtaskTotal',(SELECT count(*) FROM hagency_project_v1.tasks c WHERE c.project_id=t.project_id AND c.parent_id=t.id AND c.archived_at IS NULL),'subtaskDone',(SELECT count(*) FROM hagency_project_v1.tasks c JOIN hagency_project_v1.columns col ON col.project_id=c.project_id AND col.column_key=c.state_key WHERE c.project_id=t.project_id AND c.parent_id=t.id AND c.archived_at IS NULL AND col.is_done))";
const FILTER: &str = "t.project_id=$1 AND ($2::text IS NULL OR t.state_key=$2) AND ($3::text IS NULL OR t.assignee_mxid=$3) AND ($4::text IS NULL OR t.labels ? $4) AND ($5::text IS NULL OR position(lower($5) in lower(t.title))>0) AND (NOT $6 OR (t.due_date<(CURRENT_TIMESTAMP AT TIME ZONE 'UTC')::date AND NOT EXISTS(SELECT 1 FROM hagency_project_v1.columns col WHERE col.project_id=t.project_id AND col.column_key=t.state_key AND col.is_done))) AND ((t.archived_at IS NOT NULL)=$7)";

impl BoardStore {
    pub async fn open(url: &str) -> Result<Self> {
        let this = Self {
            pool: crate::database::Database::connect(url, 4)?,
        };
        // Startup upgrades own their session; large legacy upgrades must not
        // occupy a business slot or inherit the 30s request deadline.
        let mut db = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            AsyncPgConnection::establish(url),
        )
        .await
        .map_err(|_| Error::Unavailable("board_migration_connect_timeout"))?
        .map_err(|_| Error::Unavailable("board_storage_unavailable"))?;
        db.transaction::<_,Error,_>(async |db|{
            db.batch_execute("SET LOCAL lock_timeout='30s'; SET LOCAL statement_timeout='120s'; SELECT pg_advisory_xact_lock(5210750088328915)").await?;
            let exists=sql_query("SELECT to_regclass('hagency_project_v1.migrations') IS NOT NULL AS value").get_result::<Flag>(db).await?.value;
            if exists {
                let version=sql_query("SELECT coalesce(max(version),0)::bigint AS value FROM hagency_project_v1.migrations").get_result::<Number>(db).await?.value;
                if !(1..=7).contains(&version){return Err(Error::Conflict("board_schema_incompatible"));}
                let complete=sql_query("SELECT to_regclass('hagency_project_v1.boards') IS NOT NULL AND to_regclass('hagency_project_v1.tasks') IS NOT NULL AND to_regclass('hagency_project_v1.comments') IS NOT NULL AND to_regclass('hagency_project_v1.changes') IS NOT NULL AND to_regclass('hagency_project_v1.receipts') IS NOT NULL AS value").get_result::<Flag>(db).await?.value;
                if version>=2 && !sql_query("SELECT to_regclass('hagency_project_v1.columns') IS NOT NULL AS value").get_result::<Flag>(db).await?.value {return Err(Error::Conflict("board_schema_incompatible"));}
                if version>=3 && !sql_query("SELECT to_regclass('hagency_project_v1.notifications') IS NOT NULL AND to_regclass('hagency_project_v1.attachments') IS NOT NULL AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='changes' AND column_name='details') AS value").get_result::<Flag>(db).await?.value {return Err(Error::Conflict("board_schema_incompatible"));}
                if version>=4 && !sql_query("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='columns' AND column_name='archived') AS value").get_result::<Flag>(db).await?.value {return Err(Error::Conflict("board_schema_incompatible"));}
                if version>=5 && !sql_query("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='boards' AND column_name='owner_project_id') AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='projects' AND column_name='archived') AS value").get_result::<Flag>(db).await?.value {return Err(Error::Conflict("board_schema_incompatible"));}
                if version>=6 && !sql_query("SELECT to_regclass('hagency_project_v1.navigation_defaults') IS NOT NULL AS value").get_result::<Flag>(db).await?.value {return Err(Error::Conflict("board_schema_incompatible"));}
                if version>=7 && !sql_query("SELECT (SELECT count(*) FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='attachments' AND column_name IN ('byte_size','content_sha256'))=2 AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_project_v1' AND table_name='attachments' AND column_name='data' AND is_nullable='YES') AS value").get_result::<Flag>(db).await?.value {return Err(Error::Conflict("board_schema_incompatible"));}
                if !complete{return Err(Error::Conflict("board_schema_incompatible"));}
                if version == 1 { db.batch_execute(include_str!("schema_v2.sql")).await?; }
                if version < 3 { db.batch_execute(include_str!("schema_v3.sql")).await?; }
                if version < 4 { db.batch_execute(include_str!("schema_v4.sql")).await?; }
                if version < 5 { db.batch_execute(include_str!("schema_v5.sql")).await?; }
                if version < 6 { db.batch_execute(include_str!("schema_v6.sql")).await?; }
                if version < 7 { db.batch_execute(include_str!("schema_v7.sql")).await?; }
                return Ok(());
            }
            db.batch_execute(include_str!("schema.sql")).await?;
            db.batch_execute(include_str!("schema_v2.sql")).await?;
            db.batch_execute(include_str!("schema_v3.sql")).await?;
            db.batch_execute(include_str!("schema_v4.sql")).await?;
            db.batch_execute(include_str!("schema_v5.sql")).await?;
            db.batch_execute(include_str!("schema_v6.sql")).await?;
            db.batch_execute(include_str!("schema_v7.sql")).await?;
            Ok(())
        }).await?;
        Ok(this)
    }
    async fn authorize(db: &mut AsyncPgConnection, p: &Principal, a: &Access) -> Result<()> {
        db.batch_execute("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='8s'")
            .await?;
        DomainStore::authorize_identity(db, p, crate::api::now_ms()).await?;
        let project=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.projects p WHERE (p.id=$1 OR EXISTS(SELECT 1 FROM hagency_project_v1.boards b WHERE b.project_id=$1 AND b.owner_project_id=p.id)) AND p.space_id=$2 AND p.active FOR SHARE) AS value")
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
        sql_query(format!("SELECT {TASK_SUMMARY_JSON} || jsonb_build_object('description',t.description,'checklist',t.checklist) AS value FROM hagency_project_v1.tasks t WHERE t.project_id=$1 AND t.id=$2"))
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
    async fn page(
        db: &mut AsyncPgConnection,
        project: &str,
        q: &Query,
        seq: i64,
        total: Option<i64>,
    ) -> Result<Value> {
        let cursor = Self::cursor_id(project, q, seq)?;
        let limit = q.limit.unwrap_or(50);
        let count = if let Some(total) = total {
            total
        } else {
            sql_query(format!(
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
            .value
        };
        let rows=sql_query(format!("SELECT {TASK_SUMMARY_JSON} AS value FROM hagency_project_v1.tasks t WHERE {FILTER} AND ($8::text IS NULL OR (t.rank,t.id)>(SELECT c.rank,c.id FROM hagency_project_v1.tasks c WHERE c.project_id=$1 AND c.id=$8)) ORDER BY t.rank,t.id LIMIT $9"))
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
        self.pool.with_connection(async |db| {

        db.build_transaction().repeatable_read().run::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            let seq=Self::sequence(db,&a.project).await?;
            // One grouped scan supplies occupancy and filtered totals for every lane.
            let counts=sql_query(format!("SELECT t.state_key AS state,count(*)::bigint AS total,count(*) FILTER(WHERE t.archived_at IS NULL)::bigint AS active,count(*) FILTER(WHERE {FILTER})::bigint AS matching FROM hagency_project_v1.tasks t WHERE t.project_id=$1 GROUP BY t.state_key"))
                .bind::<Text,_>(&a.project).bind::<Nullable<Text>,_>(&q.state).bind::<Nullable<Text>,_>(&q.assignee).bind::<Nullable<Text>,_>(&q.label).bind::<Nullable<Text>,_>(&q.query).bind::<Bool,_>(q.overdue).bind::<Bool,_>(q.archived).load::<LaneCounts>(db).await?;
            let mut lanes=Vec::new();
            for column in Self::columns(db,&a.project).await? {
                let name=&column.id;
                let totals=counts.iter().find(|c| &c.state==name);
                let count=totals.map(|c|c.active).unwrap_or(0);
                let mut query=q.clone();query.state=Some(name.clone());
                let mut page=if q.state.as_ref().is_some_and(|s| s!=name) { json!({"tasks":[],"total":0,"nextCursor":null,"sequence":seq}) } else { Self::page(db,&a.project,&query,seq,Some(totals.map(|c|c.matching).unwrap_or(0))).await? };
                let all_count=totals.map(|c|c.total).unwrap_or(0);
                page["archived"]=json!(column.archived);page["state"]=json!(name);page["title"]=json!(column.title);page["wipLimit"]=json!(column.wip_limit);page["isDone"]=json!(column.is_done);page["taskCount"]=json!(count);page["allTaskCount"]=json!(all_count);lanes.push(page);
            }
            // Project-wide suggestions must not shrink with filters or task pagination.
            // Bound the response; arbitrary existing/new labels remain usable by name.
            let rows=sql_query("SELECT to_jsonb(label) AS value FROM (SELECT DISTINCT jsonb_array_elements_text(labels) AS label FROM hagency_project_v1.tasks WHERE project_id=$1) names ORDER BY label LIMIT 201")
                .bind::<Text,_>(&a.project).load::<Row>(db).await?;
            let more_labels=rows.len()>200;
            let labels:Vec<_>=rows.into_iter().take(200).map(|r|r.value).collect();
            let members:Vec<_>=a.members.iter().filter(|m|!m.starts_with("@_hagency_")).take(1000).collect();
            Self::fresh(a)?;
            let archived=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.projects p WHERE p.archived AND (p.id=$1 OR EXISTS(SELECT 1 FROM hagency_project_v1.boards b WHERE b.project_id=$1 AND b.owner_project_id=p.id))) AS value").bind::<Text,_>(&a.project).get_result::<Flag>(db).await?.value;
            Ok(json!({"readOnly":archived,"projectId":a.project,"sequence":seq,"lanes":lanes,"members":members,"memberNames":a.member_names,"labelSuggestions":labels,"hasMoreLabels":more_labels,"capability":CAPABILITY}))
        }).await
    }).await
    }
    pub(crate) async fn tasks(&self, p: &Principal, a: &Access, q: Query) -> Result<Value> {
        q.validate()?;
        self.pool
            .with_connection(async |db| {
                db.build_transaction()
                    .repeatable_read()
                    .run::<_, Error, _>(async |db| {
                        Self::authorize(db, p, a).await?;
                        let seq = Self::sequence(db, &a.project).await?;
                        let result = Self::page(db, &a.project, &q, seq, None).await?;
                        Self::fresh(a)?;
                        Ok(result)
                    })
                    .await
            })
            .await
    }
    pub(crate) async fn detail(&self, p: &Principal, a: &Access, id: &str) -> Result<Value> {
        crate::key(id)?;
        self.pool.with_connection(async |db| {

        db.build_transaction().repeatable_read().run::<_, Error, _>(async |db| {
            Self::authorize(db, p, a).await?;
            let task = Self::task(db, &a.project, id).await?;
            Self::fresh(a)?;
            let children=sql_query(format!("SELECT {TASK_SUMMARY_JSON} AS value FROM hagency_project_v1.tasks t WHERE t.project_id=$1 AND t.parent_id=$2 ORDER BY t.number LIMIT 100"))
                .bind::<Text,_>(&a.project).bind::<Text,_>(id).load::<Row>(db).await?.into_iter().map(|r|r.value).collect::<Vec<_>>();
            let parent = if let Some(parent)=task["parentId"].as_str() {Self::task(db,&a.project,parent).await?} else {Value::Null};
            Self::fresh(a)?;
            let attachments=Self::attachment_list(db,&a.project,id).await?;
            Ok(json!({"task":task,"subtasks":children,"parent":parent,"attachments":attachments}))
        })
        .await
    }).await
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
        self.pool.with_connection(async |db| {

        db.build_transaction().repeatable_read().run::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            if let Some(id)=id{Self::task(db,&a.project,id).await?;}
            let seq=Self::sequence(db,&a.project).await?;
            if !comments && after>seq{return Err(Error::Conflict("board_cursor_reset"));}
            let query=if comments {
                "SELECT jsonb_build_object('id',id,'author',author,'body',body,'createdAt',created_at,'cursor',seq) AS value FROM hagency_project_v1.comments WHERE project_id=$1 AND task_id=$2 AND seq>$3 ORDER BY seq LIMIT $4"
            }else{
                "SELECT jsonb_build_object('sequence',seq,'taskId',task_id,'actor',actor,'kind',kind,'occurredAt',occurred_at,'details',details) AS value FROM hagency_project_v1.changes WHERE project_id=$1 AND ($2::text IS NULL OR task_id=$2) AND seq>$3 ORDER BY seq LIMIT $4"
            };
            let rows=sql_query(query).bind::<Text,_>(&a.project).bind::<Nullable<Text>,_>(id).bind::<BigInt,_>(after).bind::<BigInt,_>(limit+1).load::<Row>(db).await?;
            let more=rows.len()>limit as usize;
            let items:Vec<_>=rows.into_iter().take(limit as usize).map(|r|r.value).collect();
            let next=items.last().and_then(|v|v[if comments{"cursor"}else{"sequence"}].as_i64()).unwrap_or(after);
            Self::fresh(a)?;
            let archived=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.projects p WHERE p.archived AND (p.id=$1 OR EXISTS(SELECT 1 FROM hagency_project_v1.boards b WHERE b.project_id=$1 AND b.owner_project_id=p.id))) AS value").bind::<Text,_>(&a.project).get_result::<Flag>(db).await?.value;
            Ok(json!({"readOnly":archived,"items":items,"next":next,"hasMore":more,"sequence":seq}))
        }).await
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
        self.pool.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            {
                use crate::schema::project_tables::boards as q;
                let params = (&a.project,);
                diesel::insert_into(q::table)
                    .values((
                        q::project_id.eq(&params.0),
                        q::owner_project_id.eq(&params.0),
                    ))
                    .on_conflict_do_nothing()
                    .execute(db)
                    .await
            }?;
            let seq={
                use crate::schema::project_tables::boards as q;
                let params = (&a.project,);
                q::table
                    .select((q::change_seq,))
                    .filter((q::project_id).eq(&params.0))
                    .for_update()
                    .get_result::<Number>(db)
                    .await
            }?.value;
            DomainStore::authorize_identity(db,p,crate::api::now_ms()).await?;Self::fresh(a)?;
            let replay=sql_query("SELECT jsonb_build_object('digest',digest,'result',result) AS value FROM hagency_project_v1.receipts WHERE project_id=$1 AND actor=$2 AND intent_key=$3")
                .bind::<Text,_>(&a.project).bind::<Text,_>(&p.mxid).bind::<Text,_>(mutation.intent()).get_result::<Row>(db).await;
            match replay {
                Ok(row)=>{if row.value["digest"]!=digest{return Err(Error::Conflict("board_idempotency_conflict"));}let mut result=row.value["result"].clone();result["replayed"]=json!(true);return Ok(result);}
                Err(diesel::result::Error::NotFound)=>(),Err(e)=>return Err(e.into())
            }
            let archived=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.projects p JOIN hagency_project_v1.boards b ON b.owner_project_id=p.id WHERE b.project_id=$1 AND p.archived) AS value").bind::<Text,_>(&a.project).get_result::<Flag>(db).await?.value;
            if archived {return Err(Error::Conflict("project_archived"));}
            Self::ensure_columns(db,&a.project).await?;
            let task=if let Some(id)=mutation.task(){Some(Self::task(db,&a.project,id).await?)}else{None};
            if let Some(expected)=mutation.expected() && task.as_ref().and_then(|t|t["version"].as_i64())!=Some(expected){return Err(Error::Conflict("board_version_conflict"));}
            if let Some(task)=&task {
                let archived=!task["archivedAt"].is_null();
                if archived&&!matches!(mutation,Mutation::Restore{..}) || !archived&&matches!(mutation,Mutation::Restore{..}){return Err(Error::Conflict("board_archive_conflict"));}
            }
            let current_state=task.as_ref().and_then(|t|t["state"].as_str()).unwrap_or("").to_owned();
            if task.is_some() && Self::columns(db,&a.project).await?.iter().any(|c| c.id==current_state && c.archived) { return Err(Error::Conflict("board_column_archived")); }
            let now=crate::api::now_ms();
            let mut comment=Value::Null;
            let mut attachment=Value::Null;
            let id=match &mutation {
                Mutation::UploadAttachment {task,input} => {
                    attachment=json!(Self::add_attachment(db,p,a,task,input,now).await?); Some(task.clone())
                }
                Mutation::RemoveAttachment {task,input} => {
                    let changed=sql_query("UPDATE hagency_project_v1.attachments SET removed_at=$4,data=NULL WHERE project_id=$1 AND task_id=$2 AND id=$3 AND removed_at IS NULL")
                        .bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<Text,_>(&input.attachment).bind::<BigInt,_>(now).execute(db).await?;
                    if changed==0 {return Err(Error::NotFound("board_attachment_not_found"));}
                    Some(task.clone())
                }
                Mutation::ConfigureColumns(input)=>{ Self::configure(db,&a.project,seq,input).await?; None }
                Mutation::Create(input)=>{
                    Self::assignee(db,a,&input.fields).await?;
                    let count={
                        use crate::schema::project_tables::tasks as q;
                        let params = (&a.project,);
                        q::table
                            .select((diesel::dsl::count_star(),))
                            .filter(((q::project_id).eq(&params.0)).and(q::archived_at.is_null()))
                            .get_result::<Number>(db)
                            .await
                    }?.value;
                    if count>=10000{return Err(Error::Conflict("board_capacity_exceeded"));}
                    let id=format!("tsk_{}",crate::entity_id()?);
                    Self::admit(db,&a.project,&input.state,false).await?;
                    Self::parent(db,&a.project,&id,&input.fields).await?;
                    let rank=Self::rank(db,&a.project,&input.state,&id,None).await?;
                    let number={
                        use crate::schema::project_tables::boards as q;
                        let params = (&a.project,);
                        diesel::update(q::table)
                            .filter((q::project_id).eq(&params.0))
                            .set((q::next_number.eq(q::next_number + 1),))
                            .returning(((q::next_number - 1),))
                            .get_result::<Number>(db)
                            .await
                    }?.value;
                    let f=&input.fields;
                    sql_query("INSERT INTO hagency_project_v1.tasks(id,project_id,number,title,description,state_key,rank,priority,assignee_mxid,labels,due_date,created_by,created_at,updated_at,parent_id,checklist) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::text::date,$12,$13,$13,$14,$15)")
                        .bind::<Text,_>(&id).bind::<Text,_>(&a.project).bind::<BigInt,_>(number).bind::<Text,_>(&f.title).bind::<Text,_>(&f.description).bind::<Text,_>(&input.state).bind::<BigInt,_>(rank).bind::<Text,_>(&f.priority).bind::<Nullable<Text>,_>(&f.assignee_mxid).bind::<Jsonb,_>(json!(f.labels)).bind::<Nullable<Text>,_>(&f.due_date).bind::<Text,_>(&p.mxid).bind::<BigInt,_>(now).bind::<Nullable<Text>,_>(&f.parent_id).bind::<Jsonb,_>(json!(f.checklist)).execute(db).await?;
                    Some(id)
                }
                Mutation::Update{task,input}=>{
                    Self::assignee(db,a,&input.fields).await?;Self::parent(db,&a.project,task,&input.fields).await?;let f=&input.fields;
                    let state=input.state.as_deref().unwrap_or(&current_state);
                    Self::admit(db,&a.project,state,state==current_state).await?;
                    let rank=if state==current_state { None } else { Some(Self::rank(db,&a.project,state,task,None).await?) };
                    sql_query("UPDATE hagency_project_v1.tasks SET title=$3,description=$4,priority=$5,assignee_mxid=$6,labels=$7,due_date=$8::text::date,version=version+1,updated_at=$9,parent_id=$10,checklist=$11,state_key=$12,rank=coalesce($13,rank) WHERE project_id=$1 AND id=$2")
                        .bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<Text,_>(&f.title).bind::<Text,_>(&f.description).bind::<Text,_>(&f.priority).bind::<Nullable<Text>,_>(&f.assignee_mxid).bind::<Jsonb,_>(json!(f.labels)).bind::<Nullable<Text>,_>(&f.due_date).bind::<BigInt,_>(now).bind::<Nullable<Text>,_>(&f.parent_id).bind::<Jsonb,_>(json!(f.checklist)).bind::<Text,_>(state).bind::<Nullable<BigInt>,_>(rank).execute(db).await?;
                    Some(task.clone())
                }
                Mutation::Move{task,input}=>{
                    if input.position==Position::Between && input.expected_sequence!=Some(seq){return Err(Error::Conflict("board_snapshot_changed"));}
                    Self::admit(db,&a.project,&input.state,current_state==input.state).await?;
                    let rank=Self::rank(db,&a.project,&input.state,task,Some(input)).await?;
                    {
                        use crate::schema::project_tables::tasks as q;
                        let params = (&a.project, task, &input.state, rank, now);
                        diesel::update(q::table)
                            .filter(((q::project_id).eq(&params.0)).and((q::id).eq(&params.1)))
                            .set((
                                q::state_key.eq(&params.2),
                                q::rank.eq(&params.3),
                                q::version.eq(q::version + 1),
                                q::updated_at.eq(&params.4),
                            ))
                            .execute(db)
                            .await
                    }?;Some(task.clone())
                }
                Mutation::Archive{task,..}=>{
                    {
                        use crate::schema::project_tables::tasks as q;
                        let params = (&a.project, task, now);
                        diesel::update(q::table)
                            .filter(((q::project_id).eq(&params.0)).and((q::id).eq(&params.1)))
                            .set((
                                q::archived_at.eq(&params.2),
                                q::updated_at.eq(&params.2),
                                q::version.eq(q::version + 1),
                            ))
                            .execute(db)
                            .await
                    }?;Some(task.clone())
                }
                Mutation::Restore{task:id,..}=>{
                    let count={
                        use crate::schema::project_tables::tasks as q;
                        let params = (&a.project,);
                        q::table
                            .select((diesel::dsl::count_star(),))
                            .filter(((q::project_id).eq(&params.0)).and(q::archived_at.is_null()))
                            .get_result::<Number>(db)
                            .await
                    }?.value;
                    if count>=10000{return Err(Error::Conflict("board_capacity_exceeded"));}
                    let state=task.as_ref().unwrap()["state"].as_str().unwrap();
                    Self::admit(db,&a.project,state,false).await?;
                    let rank=Self::rank(db,&a.project,state,id,None).await?;
                    sql_query("UPDATE hagency_project_v1.tasks SET archived_at=NULL,rank=$3,updated_at=$4,version=version+1 WHERE project_id=$1 AND id=$2").bind::<Text,_>(&a.project).bind::<Text,_>(id).bind::<BigInt,_>(rank).bind::<BigInt,_>(now).execute(db).await?;Some(id.clone())
                }
                Mutation::Comment{task,input}=>{
                    let id=format!("cmt_{}",crate::entity_id()?);
                    {
                        use crate::schema::project_tables::comments as q;
                        let params = (&id, &a.project, task, &p.mxid, &input.body, now);
                        diesel::insert_into(q::table)
                            .values((
                                q::id.eq(&params.0),
                                q::project_id.eq(&params.1),
                                q::task_id.eq(&params.2),
                                q::author.eq(&params.3),
                                q::body.eq(&params.4),
                                q::created_at.eq(&params.5),
                            ))
                            .execute(db)
                            .await
                    }?;
                    comment=json!({"id":id,"author":p.mxid,"body":input.body,"createdAt":now});Some(task.clone())
                }
            };
            let seq={
                use crate::schema::project_tables::boards as q;
                let params = (&a.project,);
                diesel::update(q::table)
                    .filter((q::project_id).eq(&params.0))
                    .set((q::change_seq.eq(q::change_seq + 1),))
                    .returning((q::change_seq,))
                    .get_result::<Number>(db)
                    .await
            }?.value;
            {
                use crate::schema::project_tables::changes as q;
                let params = (&a.project, seq, &id, &p.mxid, mutation.kind(), now);
                diesel::insert_into(q::table)
                    .values((
                        q::project_id.eq(&params.0),
                        q::seq.eq(&params.1),
                        q::task_id.eq(params.2),
                        q::actor.eq(&params.3),
                        q::kind.eq(&params.4),
                        q::occurred_at.eq(&params.5),
                    ))
                    .execute(db)
                    .await
            }?;
            let previous=task.unwrap_or(Value::Null);
            let task=if let Some(id)=&id { Self::task(db,&a.project,id).await? } else { Value::Null };
            Self::record_collaboration(db,p,a,seq,&mutation,(&previous,&task),now).await?;
            let result=json!({"task":task,"sequence":seq,"comment":comment,"attachment":attachment,"replayed":false});
            {
                use crate::schema::project_tables::receipts as q;
                let params = (&a.project, &p.mxid, mutation.intent(), &digest, &result);
                diesel::insert_into(q::table)
                    .values((
                        q::project_id.eq(&params.0),
                        q::actor.eq(&params.1),
                        q::intent_key.eq(&params.2),
                        q::digest.eq(&params.3),
                        q::result.eq(&params.4),
                    ))
                    .execute(db)
                    .await
            }?;
            DomainStore::authorize_identity(db,p,crate::api::now_ms()).await?;Self::fresh(a)?;
            Ok(result)
        }).await
    }).await
    }
}
#[cfg(test)]
mod tests;
