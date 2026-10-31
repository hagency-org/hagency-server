use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use diesel::{BoolExpressionMethods, ExpressionMethods, QueryDsl};
use sha2::{Digest, Sha256};

const MAX_FILE: usize = 2 * 1024 * 1024;
pub(super) fn attachment_bytes(input: &AttachmentUpload) -> Result<Vec<u8>> {
    if input.filename.trim().is_empty()
        || input.filename.chars().count() > 180
        || input
            .filename
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        || input.data.len() > MAX_FILE.div_ceil(3) * 4
    {
        return Err(Error::Invalid("invalid_board_attachment"));
    }
    let bytes = STANDARD
        .decode(&input.data)
        .map_err(|_| Error::Invalid("invalid_board_attachment"))?;
    if bytes.is_empty() || bytes.len() > MAX_FILE {
        return Err(Error::Invalid("invalid_board_attachment"));
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"%PDF-") {
        "application/pdf"
    } else {
        "application/octet-stream"
    };
    if input.content_type != mime {
        return Err(Error::Invalid("invalid_board_attachment"));
    }
    Ok(bytes)
}
fn changes(before: &Value, after: &Value) -> Value {
    let mut fields = Vec::new();
    for key in [
        "title",
        "state",
        "priority",
        "assigneeMxid",
        "dueDate",
        "labels",
        "parentId",
    ] {
        if before[key] != after[key] {
            fields.push(json!({"field":key,"before":before[key],"after":after[key]}));
        }
    }
    if before["description"] != after["description"] {
        fields.push(json!({"field":"description"}));
    }
    if before["checklist"] != after["checklist"] {
        fields.push(json!({"field":"checklist","before":before["checklistDone"],"after":after["checklistDone"]}));
    }
    json!({"fields":fields})
}
impl BoardStore {
    pub(super) async fn record_collaboration(
        db: &mut AsyncPgConnection,
        p: &Principal,
        a: &Access,
        seq: i64,
        mutation: &Mutation,
        versions: (&Value, &Value),
        now: i64,
    ) -> Result<()> {
        let (before, task) = versions;
        let mut details = changes(before, task);
        if let Mutation::UploadAttachment { input, .. } = mutation {
            details["filename"] = json!(input.filename);
        }
        if let Mutation::RemoveAttachment { input, .. } = mutation {
            details["attachmentId"] = json!(input.attachment);
        }
        if let Mutation::Comment { input, .. } = mutation {
            details["excerpt"] = json!(input.body.chars().take(160).collect::<String>());
        }
        sql_query(
            "UPDATE hagency_project_v1.changes SET details=$3 WHERE project_id=$1 AND seq=$2",
        )
        .bind::<Text, _>(&a.project)
        .bind::<BigInt, _>(seq)
        .bind::<Jsonb, _>(&details)
        .execute(db)
        .await?;
        let Some(task_id) = task["id"].as_str() else {
            return Ok(());
        };
        if matches!(mutation, Mutation::Update { .. })
            && details["fields"].as_array().is_some_and(Vec::is_empty)
        {
            return Ok(());
        }
        let mut recipients = BTreeSet::new();
        for value in [
            &task["createdBy"],
            &task["assigneeMxid"],
            &before["assigneeMxid"],
        ] {
            if let Some(id) = value.as_str() {
                recipients.insert(id.to_owned());
            }
        }
        // Participants in the discussion automatically receive subsequent updates.
        let authors=sql_query("SELECT jsonb_build_object('author',author) AS value FROM hagency_project_v1.comments WHERE project_id=$1 AND task_id=$2 GROUP BY author ORDER BY max(seq) DESC LIMIT 100")
            .bind::<Text,_>(&a.project).bind::<Text,_>(task_id).load::<Row>(db).await?;
        for row in authors {
            if let Some(author) = row.value["author"].as_str() {
                recipients.insert(author.to_owned());
            }
        }
        if let Mutation::Comment { input, .. } = mutation {
            for member in &a.members {
                // Explicit full Matrix IDs only: substring usernames never notify strangers.
                if input
                    .body
                    .split_whitespace()
                    .any(|word| word.trim_matches(['(', ')', ',', ';', '!', '。', '，']) == member)
                {
                    recipients.insert(member.clone());
                }
            }
        }
        for recipient in recipients {
            if recipient == p.mxid || !a.members.contains(&recipient) {
                continue;
            }
            {
                use crate::schema::project_tables::notifications as q;
                let params = (
                    &a.project,
                    &recipient,
                    seq,
                    task_id,
                    &p.mxid,
                    mutation.kind(),
                    task["title"].as_str().unwrap_or(""),
                    &details,
                    now,
                );
                diesel::insert_into(q::table)
                    .values((
                        q::project_id.eq(&params.0),
                        q::recipient.eq(&params.1),
                        q::seq.eq(&params.2),
                        q::task_id.eq(&params.3),
                        q::actor.eq(&params.4),
                        q::kind.eq(&params.5),
                        q::title.eq(&params.6),
                        q::details.eq(&params.7),
                        q::occurred_at.eq(&params.8),
                    ))
                    .on_conflict_do_nothing()
                    .execute(db)
                    .await
            }?;
        }
        Ok(())
    }
    pub(crate) async fn notifications(
        &self,
        p: &Principal,
        a: &Access,
        before: i64,
    ) -> Result<Value> {
        if before < 0 {
            return Err(Error::Invalid("invalid_board_query"));
        }
        self.pool.with_connection(async |db| {

        db.build_transaction().repeatable_read().run::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            let mut rows=sql_query("SELECT jsonb_build_object('sequence',seq,'taskId',task_id,'actor',actor,'kind',kind,'title',title,'details',details,'occurredAt',occurred_at,'read',read_at IS NOT NULL) AS value FROM hagency_project_v1.notifications WHERE project_id=$1 AND recipient=$2 AND ($3=0 OR seq<$3) ORDER BY seq DESC LIMIT 51")
                .bind::<Text,_>(&a.project).bind::<Text,_>(&p.mxid).bind::<BigInt,_>(before).load::<Row>(db).await?.into_iter().map(|r|r.value).collect::<Vec<_>>();
            let more=rows.len()>50; rows.truncate(50);
            let count={
                use crate::schema::project_tables::notifications as q;
                let params = (&a.project, &p.mxid);
                q::table
                    .select((diesel::dsl::count_star(),))
                    .filter(
                        (((q::project_id).eq(&params.0)).and((q::recipient).eq(&params.1)))
                            .and(q::read_at.is_null()),
                    )
                    .get_result::<Number>(db)
                    .await
            }?.value;
            Self::fresh(a)?;
            Ok(json!({"unread":count,"next":rows.last().map(|r|r["sequence"].clone()).unwrap_or(json!(0)),"items":rows,"hasMore":more}))
        }).await
    }).await
    }
    pub(crate) async fn read_notification(
        &self,
        p: &Principal,
        a: &Access,
        input: NotificationRead,
    ) -> Result<Value> {
        if input.sequence < 1 {
            return Err(Error::Invalid("invalid_board_query"));
        }
        self.pool.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            sql_query("UPDATE hagency_project_v1.notifications SET read_at=coalesce(read_at,$5) WHERE project_id=$1 AND recipient=$2 AND (seq=$3 OR ($4 AND seq<=$3))")
                .bind::<Text,_>(&a.project).bind::<Text,_>(&p.mxid).bind::<BigInt,_>(input.sequence).bind::<Bool,_>(input.all_before)
                .bind::<BigInt,_>(crate::api::now_ms()).execute(db).await?;
            Self::fresh(a)?;
            Ok(json!({"read":true}))
        }).await
    }).await
    }
    pub(super) async fn attachment_list(
        db: &mut AsyncPgConnection,
        project: &str,
        task: &str,
    ) -> Result<Vec<Value>> {
        Ok(sql_query("SELECT jsonb_build_object('id',id,'filename',filename,'contentType',content_type,'size',byte_size,'createdBy',created_by,'createdAt',created_at) AS value FROM hagency_project_v1.attachments WHERE project_id=$1 AND task_id=$2 AND removed_at IS NULL ORDER BY created_at,id LIMIT 20")
            .bind::<Text,_>(project).bind::<Text,_>(task).load::<Row>(db).await?.into_iter().map(|r|r.value).collect())
    }
    pub(super) async fn add_attachment(
        db: &mut AsyncPgConnection,
        p: &Principal,
        a: &Access,
        task: &str,
        input: &AttachmentUpload,
        now: i64,
    ) -> Result<String> {
        let bytes = attachment_bytes(input)?;
        let id = format!("att_{}", ulid::Ulid::generate());
        let full=sql_query("SELECT count(*)>=20 OR coalesce(sum(byte_size),0)+$3>41943040 AS value FROM hagency_project_v1.attachments WHERE project_id=$1 AND task_id=$2 AND removed_at IS NULL")
            .bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<BigInt,_>(bytes.len() as i64).get_result::<Flag>(db).await?.value;
        if full {
            return Err(Error::Conflict("board_attachment_limit"));
        }
        let size = bytes.len() as i64;
        let digest = hex::encode(Sha256::digest(&bytes));
        {
            use crate::schema::project_tables::attachments as q;
            let params = (
                &id,
                &a.project,
                task,
                &input.filename,
                &input.content_type,
                bytes,
                &p.mxid,
                now,
                size,
                digest,
            );
            diesel::insert_into(q::table)
                .values((
                    q::id.eq(&params.0),
                    q::project_id.eq(&params.1),
                    q::task_id.eq(&params.2),
                    q::filename.eq(&params.3),
                    q::content_type.eq(&params.4),
                    q::data.eq(&params.5),
                    q::created_by.eq(&params.6),
                    q::created_at.eq(&params.7),
                    q::byte_size.eq(&params.8),
                    q::content_sha256.eq(&params.9),
                ))
                .execute(db)
                .await
        }?;
        Ok(id)
    }
    pub(crate) async fn attachment(
        &self,
        p: &Principal,
        a: &Access,
        task: &str,
        id: &str,
    ) -> Result<Value> {
        crate::key(task)?;
        crate::key(id)?;
        self.pool.with_connection(async |db| {

        db.build_transaction().repeatable_read().run::<_,Error,_>(async |db|{
            Self::authorize(db,p,a).await?;
            let row=sql_query("SELECT jsonb_build_object('id',id,'filename',filename,'contentType',content_type,'data',replace(encode(data,'base64'),E'\\n','')) AS value FROM hagency_project_v1.attachments WHERE project_id=$1 AND task_id=$2 AND id=$3 AND removed_at IS NULL")
                .bind::<Text,_>(&a.project).bind::<Text,_>(task).bind::<Text,_>(id).get_result::<Row>(db).await.map_err(|e|match e {diesel::result::Error::NotFound=>Error::NotFound("board_attachment_not_found"),other=>other.into()})?;
            Self::fresh(a)?;Ok(row.value)
        }).await
    }).await
    }
}
