//! Registered scope discovery is an internal input; the API must apply live
//! Matrix membership/link checks before returning any row to a caller.
use super::*;
use diesel::{BoolExpressionMethods, ExpressionMethods, QueryDsl, SelectableHelper};

#[derive(Debug, Serialize, diesel::Queryable, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct RoomAgent {
    #[diesel(sql_type=Bool)]
    pub is_mine: bool,
    #[diesel(sql_type=Text)]
    pub visibility: String,
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
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    if !Self::project_db(db, project).await?.active {
                        return Err(Error::Unauthorized("project_not_active"));
                    }
                    Ok({
                        use crate::schema::agent_tables::rooms as q;
                        let params = (project,);
                        q::table
                            .select(crate::domain::Room::as_select())
                            .filter(((q::project_id).eq(&params.0)).and(q::active))
                            .order((q::room_id.asc(),))
                            .load(db)
                            .await
                    }?)
                })
                .await
            })
            .await
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
        self.db.with_connection(async |db| {

        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, now).await?;
            if !Self::project_db(db, project).await?.active || !Self::room_db(db, room, project).await?.active {
                return Err(Error::Unauthorized("room_not_active"));
            }
            Ok(sql_query("SELECT (a.owner_user_id=$3) AS is_mine,a.visibility,a.id AS agent_id,a.puppet_mxid,a.display_name,u.mxid AS owner_mxid,b.state AS binding_state FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE b.project_id=$1 AND b.room_id=$2 AND b.state IN ('active','suspended') AND a.state IN ('active','suspended') AND u.active AND (a.visibility='public' OR a.owner_user_id=$3) ORDER BY a.id")
                .bind::<Text,_>(project).bind::<Text,_>(room).bind::<Text,_>(&p.user_id).load(db).await?)
        }).await
    }).await
    }
}

#[derive(Debug, Serialize, diesel::Queryable, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverableAgent {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub display_name: String,
    #[diesel(sql_type=Text)]
    pub owner_mxid: String,
    #[diesel(sql_type=Text)]
    pub visibility: String,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=Bool)]
    pub is_mine: bool,
}
impl DomainStore {
    pub async fn discover_agents(&self, p: &Principal, now: i64) -> Result<Vec<DiscoverableAgent>> {
        self.db.with_connection(async |db| {

        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, now).await?;
            Ok(sql_query("SELECT a.id,a.puppet_mxid,a.display_name,u.mxid AS owner_mxid,a.visibility,a.state,(a.owner_user_id=$1) AS is_mine FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE u.active AND a.state IN ('active','suspended') AND a.visibility='public' ORDER BY a.id LIMIT 256").bind::<Text,_>(&p.user_id).load(db).await?)
        }).await
    }).await
    }
}
