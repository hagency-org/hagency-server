//! Independent owner-only Matrix direct Room, never a synthetic Project.
use super::*;
use diesel::{BoolExpressionMethods, ExpressionMethods, QueryDsl, SelectableHelper};
impl DomainStore {
    pub async fn adopt_owner_direct(
        &self,
        p: &Principal,
        agent: &str,
        room: &str,
        f: &RoomFacts,
        now: i64,
    ) -> Result<AgentBinding> {
        matrix_id(room, '!')?;
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,now).await?;let a=Self::agent_db(db,p,agent).await?;
            if a.state!="active" {return Err(Error::Conflict("agent_identity_not_active"));}
            membership(p,f,room,"",now)?;
            if f.puppet_mxid.as_deref()!=Some(a.puppet_mxid.as_str())||!f.owner_direct_valid {return Err(Error::Unauthorized("owner_direct_room_required"));}
            if let Some(existing)=&a.owner_direct_room_id && existing!=room {return Err(Error::Conflict("owner_direct_room_already_assigned"));}
            if let Some(binding)={
                use crate::schema::agent_tables::bindings as q;
                let params = (agent, room);
                q::table
                    .select(crate::domain::Binding::as_select())
                    .filter(((q::agent_id).eq(&params.0)).and((q::room_id).eq(&params.1)))
                    .get_result::<Binding>(db)
                    .await
            }.optional()? {
                if binding.scope_kind!="owner_direct" {return Err(Error::Conflict("room_already_project_bound"));}
                let binding = match binding.state.as_str() {
                    "joining" | "active" | "suspended" => binding,
                    "left" => {
                        let rebound={
                            use crate::schema::agent_tables::bindings as q;
                            let params = (&binding.id,);
                            diesel::update(q::table)
                                .filter((q::id).eq(&params.0))
                                .set((
                                    q::state.eq("joining"),
                                    q::owner_service_paused.eq(false),
                                    q::generation.eq(q::generation + 1),
                                ))
                                .returning(crate::domain::Binding::as_select())
                                .get_result(db)
                                .await
                        }?;
                        Self::audit(db,p,"agent.owner_direct.rebind",&binding.id,now).await?;
                        rebound
                    }
                    _ => return Err(Error::Conflict("binding_not_bindable")),
                };
                return Ok(AgentBinding{agent:a,binding});
            }
            let a={
                use crate::schema::agent_tables::agents as q;
                let params = (agent, room);
                diesel::update(q::table)
                    .filter((q::id).eq(&params.0))
                    .set((q::owner_direct_room_id.eq(&params.1),))
                    .returning(crate::domain::Agent::as_select())
                    .get_result::<Agent>(db)
                    .await
            }?;
            let binding=sql_query("INSERT INTO hagency_agent_v1.bindings(id,agent_id,project_id,room_id,state,scope_kind) VALUES($1,$2,NULL,$3,'joining','owner_direct') RETURNING id,agent_id,project_id,room_id,state,generation,scope_kind,owner_service_paused,thread_auto_reply").bind::<Text,_>(format!("bnd_{}",entity_id()?)).bind::<Text,_>(agent).bind::<Text,_>(room).get_result(db).await?;
            Self::audit(db,p,"agent.owner_direct",agent,now).await?;
            Ok(AgentBinding{agent:a,binding})
        }).await
    }).await
    }
}
