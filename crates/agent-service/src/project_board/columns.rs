use super::*;
impl BoardStore {
    pub(super) async fn columns(
        db: &mut AsyncPgConnection,
        project: &str,
    ) -> Result<Vec<ColumnDefinition>> {
        let rows = sql_query("SELECT jsonb_build_object('id',column_key,'title',title,'wipLimit',wip_limit,'isDone',is_done,'archived',archived) AS value FROM hagency_project_v1.columns WHERE project_id=$1 ORDER BY position,column_key")
            .bind::<Text,_>(project).load::<Row>(db).await?;
        if rows.is_empty() {
            return Ok(default_columns());
        }
        rows.into_iter()
            .map(|r| {
                serde_json::from_value(r.value)
                    .map_err(|_| Error::Conflict("board_schema_incompatible"))
            })
            .collect()
    }
    pub(super) async fn ensure_columns(db: &mut AsyncPgConnection, project: &str) -> Result<()> {
        let exists = sql_query(
            "SELECT EXISTS(SELECT 1 FROM hagency_project_v1.columns WHERE project_id=$1) AS value",
        )
        .bind::<Text, _>(project)
        .get_result::<Flag>(db)
        .await?
        .value;
        if !exists {
            for (i, c) in default_columns().into_iter().enumerate() {
                Self::put_column(db, project, &c, i as i64).await?;
            }
        }
        Ok(())
    }
    async fn put_column(
        db: &mut AsyncPgConnection,
        project: &str,
        c: &ColumnDefinition,
        position: i64,
    ) -> Result<()> {
        sql_query("INSERT INTO hagency_project_v1.columns(project_id,column_key,title,position,wip_limit,is_done,archived) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(project_id,column_key) DO UPDATE SET title=$3,position=$4,wip_limit=$5,is_done=$6,archived=$7")
            .bind::<Text,_>(project).bind::<Text,_>(&c.id).bind::<Text,_>(&c.title).bind::<BigInt,_>(position).bind::<Nullable<BigInt>,_>(c.wip_limit).bind::<Bool,_>(c.is_done).bind::<Bool,_>(c.archived).execute(db).await?;
        Ok(())
    }
    pub(super) async fn configure(
        db: &mut AsyncPgConnection,
        project: &str,
        seq: i64,
        input: &ConfigureColumns,
    ) -> Result<()> {
        if seq != input.expected_sequence {
            return Err(Error::Conflict("board_snapshot_changed"));
        }
        for old in Self::columns(db, project).await? {
            if !input.columns.iter().any(|c| c.id == old.id) {
                let used=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_project_v1.tasks WHERE project_id=$1 AND state_key=$2) AS value").bind::<Text,_>(project).bind::<Text,_>(&old.id).get_result::<Flag>(db).await?.value;
                if used {
                    return Err(Error::Conflict("board_column_not_empty"));
                }
                sql_query(
                    "DELETE FROM hagency_project_v1.columns WHERE project_id=$1 AND column_key=$2",
                )
                .bind::<Text, _>(project)
                .bind::<Text, _>(&old.id)
                .execute(db)
                .await?;
            }
        }
        for (i, c) in input.columns.iter().enumerate() {
            Self::put_column(db, project, c, i as i64).await?;
        }
        Ok(())
    }
    /// Called only while holding the board row lock, including create/restore.
    pub(super) async fn admit(
        db: &mut AsyncPgConnection,
        project: &str,
        state: &str,
        same_column: bool,
    ) -> Result<()> {
        let column = Self::columns(db, project)
            .await?
            .into_iter()
            .find(|c| c.id == state)
            .ok_or(Error::Invalid("invalid_board_state"))?;
        if column.archived {
            return Err(Error::Conflict("board_column_archived"));
        }
        if !same_column && let Some(limit) = column.wip_limit {
            let count=sql_query("SELECT count(*)::bigint AS value FROM hagency_project_v1.tasks WHERE project_id=$1 AND state_key=$2 AND archived_at IS NULL").bind::<Text,_>(project).bind::<Text,_>(state).get_result::<Number>(db).await?.value;
            if count >= limit {
                return Err(Error::Conflict("board_wip_limit_reached"));
            }
        }
        Ok(())
    }
    pub(super) async fn parent(
        db: &mut AsyncPgConnection,
        project: &str,
        id: &str,
        fields: &Fields,
    ) -> Result<()> {
        if let Some(parent) = &fields.parent_id {
            let task = Self::task(db, project, parent).await.map_err(|e| match e {
                Error::NotFound(_) => Error::Invalid("invalid_board_parent"),
                other => other,
            })?;
            let existing=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_project_v1.tasks WHERE project_id=$1 AND id=$2 AND parent_id=$3) AS value").bind::<Text,_>(project).bind::<Text,_>(id).bind::<Text,_>(parent).get_result::<Flag>(db).await?.value;
            if parent == id
                || !task["parentId"].is_null()
                || !task["archivedAt"].is_null() && !existing
            {
                return Err(Error::Invalid("invalid_board_parent"));
            }
            let children=sql_query("SELECT count(*)::bigint AS value FROM hagency_project_v1.tasks WHERE project_id=$1 AND parent_id=$2 AND id<>$3").bind::<Text,_>(project).bind::<Text,_>(parent).bind::<Text,_>(id).get_result::<Number>(db).await?.value;
            let has_children=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_project_v1.tasks WHERE project_id=$1 AND parent_id=$2) AS value").bind::<Text,_>(project).bind::<Text,_>(id).get_result::<Flag>(db).await?.value;
            if children >= 100 || has_children {
                return Err(Error::Invalid("invalid_board_parent"));
            }
        }
        Ok(())
    }
}
