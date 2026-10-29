use super::*;
use diesel::OptionalExtension;

#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub(crate) struct NewBoard {pub idempotency_key:String,pub name:String}
impl BoardStore {
    pub(crate) async fn catalog(&self,p:&Principal,a:&Access,input:Option<NewBoard>)->Result<Value> {
        let mut db=self.connection().await?;
        db.transaction::<_,Error,_>(async |db| {
            if input.is_some() {sql_query("SELECT pg_advisory_xact_lock(hashtextextended($1,1))").bind::<Text,_>(&a.project).execute(db).await?;}
            Self::authorize(db,p,a).await?;
            sql_query("INSERT INTO hagency_project_v1.boards(project_id,owner_project_id) VALUES($1,$1) ON CONFLICT DO NOTHING").bind::<Text,_>(&a.project).execute(db).await?;
            let mut created_board=None;
            if let Some(input)=input {
                sql_query("SELECT archived AS value FROM hagency_agent_v1.projects WHERE id=$1 FOR UPDATE").bind::<Text,_>(&a.project).get_result::<Flag>(db).await?;
                crate::key(&input.idempotency_key)?;
                let name=input.name.trim();
                if name.is_empty() || name.chars().count()>80 || name.chars().any(char::is_control) {return Err(Error::Invalid("invalid_board_name"));}
                let archived=sql_query("SELECT archived AS value FROM hagency_agent_v1.projects WHERE id=$1 FOR SHARE").bind::<Text,_>(&a.project).get_result::<Flag>(db).await?.value;
                if archived {return Err(Error::Conflict("project_archived"));}
                let existing=sql_query("SELECT jsonb_build_object('id',project_id,'name',name) AS value FROM hagency_project_v1.boards WHERE owner_project_id=$1 AND created_by=$2 AND creation_key=$3").bind::<Text,_>(&a.project).bind::<Text,_>(&p.mxid).bind::<Text,_>(&input.idempotency_key).get_result::<Row>(db).await.optional()?;
                if let Some(row)=existing {if row.value["name"]!=name {return Err(Error::Conflict("board_intent_conflict"));}created_board=row.value["id"].as_str().map(str::to_owned);}
                else {
                    let count=sql_query("SELECT count(*)::bigint AS value FROM hagency_project_v1.boards WHERE owner_project_id=$1").bind::<Text,_>(&a.project).get_result::<Number>(db).await?.value;
                    if count>=64 {return Err(Error::Conflict("board_limit"));}
                    let board_id=format!("board_{}",crate::entity_id()?);
                    created_board=Some(board_id.clone());
                    sql_query("INSERT INTO hagency_project_v1.boards(project_id,owner_project_id,name,created_by,creation_key) VALUES($1,$2,$3,$4,$5)").bind::<Text,_>(board_id).bind::<Text,_>(&a.project).bind::<Text,_>(name).bind::<Text,_>(&p.mxid).bind::<Text,_>(&input.idempotency_key).execute(db).await?;
                }
            }
            let rows=sql_query("SELECT jsonb_build_object('id',project_id,'name',name) AS value FROM hagency_project_v1.boards WHERE owner_project_id=$1 ORDER BY (project_id=owner_project_id) DESC,name,project_id").bind::<Text,_>(&a.project).load::<Row>(db).await?;
            let mut result=json!({"projectId":a.project,"boards":rows.into_iter().map(|r|r.value).collect::<Vec<_>>()});
            if let Some(id)=created_board {result["createdBoardId"]=json!(id);}
            Ok(result)
        }).await
    }
    pub(crate) async fn resolve_board(&self,project:&str,board:&str)->Result<()> {
        crate::key(board)?;
        let mut db=self.connection().await?;
        let exists=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_project_v1.boards WHERE project_id=$1 AND owner_project_id=$2) AS value").bind::<Text,_>(board).bind::<Text,_>(project).get_result::<Flag>(&mut db).await?.value;
        if !exists && board!=project {return Err(Error::NotFound("board_not_found"));}
        Ok(())
    }
}

impl BoardStore {
    pub(crate) async fn overview(&self,p:&Principal,a:&Access)->Result<Value> {
        let catalog=self.catalog(p,a,None).await?;
        let mut db=self.connection().await?;
        db.transaction::<_,Error,_>(async |db| {
            Self::authorize(db,p,a).await?;
            let counts=sql_query("SELECT jsonb_build_object('tasks',count(*) FILTER(WHERE t.archived_at IS NULL),'done',count(*) FILTER(WHERE t.archived_at IS NULL AND c.is_done),'overdue',count(*) FILTER(WHERE t.archived_at IS NULL AND NOT c.is_done AND t.due_date<(CURRENT_TIMESTAMP AT TIME ZONE 'UTC')::date),'members',$2::bigint) AS value FROM hagency_project_v1.tasks t JOIN hagency_project_v1.boards b ON b.project_id=t.project_id LEFT JOIN hagency_project_v1.columns c ON c.project_id=t.project_id AND c.column_key=t.state_key WHERE b.owner_project_id=$1").bind::<Text,_>(&a.project).bind::<BigInt,_>(a.members.len() as i64).get_result::<Row>(db).await?.value;
            let activity=sql_query("SELECT jsonb_build_object('kind',c.kind,'actor',c.actor,'occurredAt',c.occurred_at,'title',coalesce(t.title,b.name),'board',b.name) AS value FROM hagency_project_v1.changes c JOIN hagency_project_v1.boards b ON b.project_id=c.project_id LEFT JOIN hagency_project_v1.tasks t ON t.id=c.task_id AND t.project_id=c.project_id WHERE b.owner_project_id=$1 ORDER BY c.occurred_at DESC,c.seq DESC LIMIT 10").bind::<Text,_>(&a.project).load::<Row>(db).await?;
            let mut result=catalog;
            result["memberNames"]=json!(a.member_names);
            result["stats"]=counts;
            result["activity"]=json!(activity.into_iter().map(|r|r.value).collect::<Vec<_>>());
            Ok(result)
        }).await
    }
}
