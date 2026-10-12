//! Registered scope discovery is an internal input; the API must apply live
//! Matrix membership/link checks before returning any row to a caller.
use super::*;

#[derive(Debug, Serialize, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct RoomAgent {
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub display_name: String,
    #[diesel(sql_type=Text)]
    pub owner_mxid: String,
    #[diesel(sql_type=Text)]
    pub binding_state: String,
}
impl DomainStore {
    pub(crate) async fn registered_rooms(
        &self,
        p: &Principal,
        project: &str,
        now: i64,
    ) -> Result<Vec<Room>> {
        key(project)?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, now).await?;
            if !Self::project_db(db, project).await?.active {
                return Err(Error::Unauthorized("project_not_active"));
            }
            Ok(sql_query("SELECT room_id,project_id,active,creation_policy,revision FROM hagency_agent_v1.rooms WHERE project_id=$1 AND active ORDER BY room_id")
                .bind::<Text,_>(project).load(db).await?)
        }).await
    }
    pub(crate) async fn room_roster(
        &self,
        p: &Principal,
        project: &str,
        room: &str,
        now: i64,
    ) -> Result<Vec<RoomAgent>> {
        key(project)?;
        matrix_id(room, '!')?;
        let mut guard = self.db.lock().await?;
        (*guard).transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, now).await?;
            if !Self::project_db(db, project).await?.active || !Self::room_db(db, room, project).await?.active {
                return Err(Error::Unauthorized("room_not_active"));
            }
            Ok(sql_query("SELECT a.id AS agent_id,a.puppet_mxid,a.display_name,u.mxid AS owner_mxid,b.state AS binding_state FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE b.project_id=$1 AND b.room_id=$2 AND b.state IN ('active','suspended') AND a.state IN ('active','suspended') ORDER BY a.id")
                .bind::<Text,_>(project).bind::<Text,_>(room).load(db).await?)
        }).await
    }
}
