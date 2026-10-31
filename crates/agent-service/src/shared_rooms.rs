//! Owner-authorized additional rooms for pool Agents. The reservation and original
//! conversation remain immutable; every extra room has its own revocable generation.
use super::*;
use crate::{gateway::Gateway, matrix_client::MatrixClient};
use diesel::QueryableByName;
#[derive(QueryableByName)]
struct SharedRoom {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Text)]
    assignment_id: String,
    #[diesel(sql_type=Text)]
    agent_id: String,
    #[diesel(sql_type=Text)]
    project_id: String,
    #[diesel(sql_type=Text)]
    room_id: String,
    #[diesel(sql_type=Text)]
    state: String,
    #[diesel(sql_type=BigInt)]
    generation: i64,
}
impl SharedRoom {
    fn binding(&self) -> Binding {
        Binding {
            id: self.id.clone(),
            agent_id: self.agent_id.clone(),
            project_id: Some(self.project_id.clone()),
            room_id: self.room_id.clone(),
            state: self.state.clone(),
            generation: self.generation,
            scope_kind: "project".into(),
            owner_service_paused: false,
            thread_auto_reply: false,
        }
    }
}
async fn row(db: &mut AsyncPgConnection, id: &str) -> Result<SharedRoom> {
    sql_query("SELECT * FROM hagency_agent_v1.shared_agent_rooms WHERE id=$1")
        .bind::<Text, _>(id)
        .get_result(db)
        .await
        .map_err(hidden)
}
#[derive(QueryableByName)]
struct RoomMeta {
    #[diesel(sql_type=Text)]
    owner: String,
    #[diesel(sql_type=Text)]
    space: String,
    #[diesel(sql_type=Bool)]
    live: bool,
}
async fn meta(db: &mut AsyncPgConnection, b: &SharedRoom) -> Result<RoomMeta> {
    sql_query("SELECT u.mxid AS owner,p.space_id AS space,(u.active AND p.active AND r.active AND a.owner_user_id=s.manager_user_id AND a.execution_kind='shared_project' AND s.state='pending_native' AND mm.state='matrix_verified' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.scope_pauses sp WHERE sp.paused AND ((sp.kind='project' AND sp.scope_id=b.project_id) OR (sp.kind='room' AND sp.scope_id=b.room_id)))) AS live FROM hagency_agent_v1.shared_agent_rooms b JOIN hagency_agent_v1.projects p ON p.id=b.project_id JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.management_shared_assignments s ON s.id=b.assignment_id AND s.agent_id=b.agent_id JOIN hagency_agent_v1.management_agent_matrix mm ON mm.assignment_id=s.id JOIN hagency_agent_v1.users u ON u.id=s.manager_user_id WHERE b.id=$1 AND b.assignment_id=$2")
        .bind::<Text,_>(&b.id).bind::<Text,_>(&b.assignment_id).get_result(db).await.map_err(hidden)
}
impl DomainStore {
    pub(crate) async fn room_agent(&self, p: &Principal, id: &str, now: i64) -> Result<Agent> {
        let mut db = self.db.lock().await?;
        Self::authorize(&mut db, p, now).await?;
        Self::room_agent_db(&mut db, p, id).await
    }

    async fn room_agent_db(db: &mut AsyncPgConnection, p: &Principal, id: &str) -> Result<Agent> {
        sql_query("SELECT id,owner_user_id,puppet_mxid,display_name,state,generation,owner_direct_room_id,execution_device_id FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2")
            .bind::<Text,_>(id).bind::<Text,_>(&p.user_id).get_result(db).await.map_err(hidden)
    }
    pub(crate) async fn room_agents(&self, p: &Principal, now: i64) -> Result<Vec<Agent>> {
        let mut db = self.db.lock().await?;
        Self::authorize(&mut db, p, now).await?;
        let agents:Vec<Agent>=sql_query("SELECT a.id,a.owner_user_id,a.puppet_mxid,a.display_name,CASE WHEN a.execution_kind='shared_project' THEN 'active' ELSE a.state END AS state,a.generation,a.owner_direct_room_id,a.execution_device_id FROM hagency_agent_v1.agents a WHERE a.owner_user_id=$1 AND (a.execution_kind='personal' OR EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments s JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=s.id JOIN hagency_agent_v1.management_agent_requests r ON r.id=s.request_id WHERE s.agent_id=a.id AND s.state='pending_native' AND m.state='matrix_verified' AND r.pool_contribution_id IS NOT NULL)) ORDER BY a.id")
            .bind::<Text,_>(&p.user_id).load(&mut *db).await?;
        // Shared agents intentionally keep their identity suspended for the
        // personal execution protocol; this picker projects room availability.
        Ok(agents)
    }
    pub(crate) async fn shared_room_command(
        db: &mut AsyncPgConnection,
        p: &Principal,
        key: &str,
    ) -> Result<Option<AgentCommand>> {
        #[derive(QueryableByName)]
        struct Saved {
            #[diesel(sql_type=Text)]
            binding_id: String,
        }
        let saved=sql_query("SELECT binding_id FROM hagency_agent_v1.shared_agent_room_commands WHERE actor_user_id=$1 AND key=$2").bind::<Text,_>(&p.user_id).bind::<Text,_>(key).get_result::<Saved>(db).await.optional()?;
        if let Some(s) = saved {
            let b = row(db, &s.binding_id).await?;
            return Ok(Some(AgentCommand {
                agent: Self::room_agent_db(db, p, &b.agent_id).await?,
                binding: Some(b.binding()),
            }));
        }
        Ok(None)
    }
    pub(crate) async fn try_bind_shared_room(
        &self,
        p: &Principal,
        agent: &str,
        input: &BindRoom,
        g: &Gateway,
        matrix: &MatrixClient,
    ) -> Result<Option<AgentBinding>> {
        key(agent)?;
        key(&input.project_id)?;
        key(&input.idempotency_key)?;
        matrix_id(&input.room_id, '!')?;
        let (a, project, assignment) = {
            let mut db = self.db.lock().await?;
            Self::authorize(&mut db, p, crate::api::now_ms()).await?;
            let shared=sql_query("SELECT execution_kind='shared_project' AS matched FROM hagency_agent_v1.agents WHERE id=$1 AND owner_user_id=$2").bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).get_result::<Flag>(&mut *db).await.map_err(hidden)?;
            if !shared.matched {
                return Ok(None);
            }
            #[derive(QueryableByName)]
            struct Id {
                #[diesel(sql_type=Text)]
                id: String,
            }
            let assignment=sql_query("SELECT s.id FROM hagency_agent_v1.management_shared_assignments s JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=s.id JOIN hagency_agent_v1.management_agent_requests r ON r.id=s.request_id WHERE s.agent_id=$1 AND s.manager_user_id=$2 AND s.state='pending_native' AND m.state='matrix_verified' AND r.pool_contribution_id IS NOT NULL AND m.room_id<>$3")
                .bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).bind::<Text,_>(&input.room_id).get_result::<Id>(&mut *db).await.map_err(hidden)?;
            (
                Self::room_agent_db(&mut db, p, agent).await?,
                Self::project_db(&mut db, &input.project_id).await?,
                assignment.id,
            )
        };
        let f = g
            .room(
                &input.room_id,
                &project.space_id,
                &p.mxid,
                Some(&a.puppet_mxid),
            )
            .await?;
        let digest = hash(&format!("{agent}:{}", encode(input)?));
        let b = {
            let mut guard = self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            Self::authorize(db,p,crate::api::now_ms()).await?;
            Self::room_agent_db(db,p,agent).await?;
            let live=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.management_shared_assignments s JOIN hagency_agent_v1.management_agent_matrix m ON m.assignment_id=s.id WHERE s.id=$1 AND s.agent_id=$2 AND s.manager_user_id=$3 AND s.state='pending_native' AND m.state='matrix_verified') AS matched").bind::<Text,_>(&assignment).bind::<Text,_>(agent).bind::<Text,_>(&p.user_id).get_result::<Flag>(db).await?;
            if !live.matched {return Err(Error::Conflict("shared_assignment_changed"));}
            if sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.domain_commands WHERE actor_user_id=$1 AND operation='agent.bind' AND key=$2) AS matched").bind::<Text,_>(&p.user_id).bind::<Text,_>(&input.idempotency_key).get_result::<Flag>(db).await?.matched {return Err(Error::Conflict("idempotency_key_reused"));}
            let (_,pp,rp)=Self::policies(db,&input.project_id,&input.room_id).await?;
            creating(p,&f,&input.room_id,&project.space_id,&pp,&rp,Some(&a.puppet_mxid),crate::api::now_ms())?;
            #[derive(QueryableByName)] struct Saved {#[diesel(sql_type=Text)] digest:String,#[diesel(sql_type=Text)] binding_id:String}
            if let Some(saved)=sql_query("SELECT digest,binding_id FROM hagency_agent_v1.shared_agent_room_commands WHERE actor_user_id=$1 AND key=$2").bind::<Text,_>(&p.user_id).bind::<Text,_>(&input.idempotency_key).get_result::<Saved>(db).await.optional()? {
                if saved.digest!=digest {return Err(Error::Conflict("idempotency_key_reused"));} return row(db,&saved.binding_id).await;
            }
            let old=sql_query("SELECT * FROM hagency_agent_v1.shared_agent_rooms WHERE assignment_id=$1 AND room_id=$2").bind::<Text,_>(&assignment).bind::<Text,_>(&input.room_id).get_result::<SharedRoom>(db).await.optional()?;
            let b=if let Some(old)=old {
                if old.project_id!=input.project_id || old.state=="leaving" {return Err(Error::Conflict("room_binding_scope_conflict"));}
                if old.state=="left" {sql_query("UPDATE hagency_agent_v1.shared_agent_rooms SET state='joining',generation=generation+1 WHERE id=$1 RETURNING *").bind::<Text,_>(&old.id).get_result(db).await?} else {old}
            } else {sql_query("INSERT INTO hagency_agent_v1.shared_agent_rooms(id,assignment_id,agent_id,project_id,room_id,state) VALUES($1,$2,$3,$4,$5,'joining') RETURNING *").bind::<Text,_>(format!("srb_{}",entity_id()?)).bind::<Text,_>(&assignment).bind::<Text,_>(agent).bind::<Text,_>(&input.project_id).bind::<Text,_>(&input.room_id).get_result(db).await?};
            sql_query("INSERT INTO hagency_agent_v1.shared_agent_room_commands VALUES($1,$2,$3,$4)").bind::<Text,_>(&p.user_id).bind::<Text,_>(&input.idempotency_key).bind::<Text,_>(&digest).bind::<Text,_>(&b.id).execute(db).await?;
            Self::audit(db,p,"agent.bind_shared_room",&b.id,crate::api::now_ms()).await?;Ok(b)
        }).await?
        };
        self.reconcile_shared_room(&b.id, g, matrix).await?;
        let mut db = self.db.lock().await?;
        Self::authorize(&mut db, p, crate::api::now_ms()).await?;
        Ok(Some(AgentBinding {
            agent: a,
            binding: row(&mut db, &b.id).await?.binding(),
        }))
    }
    pub(crate) async fn shared_room_candidates(&self, after: &str) -> Result<Vec<String>> {
        #[derive(QueryableByName)]
        struct Id {
            #[diesel(sql_type=Text)]
            id: String,
        }
        let mut db = self.db.lock().await?;
        Ok(sql_query(
            "SELECT id FROM hagency_agent_v1.shared_agent_rooms WHERE id>$1 ORDER BY id LIMIT 20",
        )
        .bind::<Text, _>(after)
        .load::<Id>(&mut *db)
        .await?
        .into_iter()
        .map(|r| r.id)
        .collect())
    }
    pub(crate) async fn reconcile_shared_room(
        &self,
        id: &str,
        g: &Gateway,
        m: &MatrixClient,
    ) -> Result<()> {
        // Each call performs at most one remote mutation. The worker retries
        // uncertain outcomes and observes membership before advancing state.
        let (b, a, scope) = {
            let mut db = self.db.lock().await?;
            let b = row(&mut db, id).await?;
            let a:Agent=sql_query("SELECT id,owner_user_id,puppet_mxid,display_name,state,generation,owner_direct_room_id,execution_device_id FROM hagency_agent_v1.agents WHERE id=$1 AND execution_kind='shared_project'").bind::<Text,_>(&b.agent_id).get_result(&mut *db).await?;
            let scope = meta(&mut db, &b).await?;
            (b, a, scope)
        };
        let f = g
            .room(&b.room_id, &scope.space, &scope.owner, Some(&a.puppet_mxid))
            .await?;
        let invited = if f.puppet_in_room {
            false
        } else {
            g.puppet_invited(&b.room_id, &a.puppet_mxid).await?
        };
        let effect = {
            let mut guard = self.db.lock().await?;
            (*guard).transaction::<_,Error,_>(async|db:&mut AsyncPgConnection|{
                db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
                let current=row(db,id).await?;
                if current.generation!=b.generation||current.state!=b.state{return Ok(None);}
                let fresh=meta(db,&current).await?;
                let invalid=!fresh.live||fresh.owner!=scope.owner||fresh.space!=scope.space
                    ||!f.owner_in_room||!f.owner_in_space||!f.room_in_space
                    ||(current.state=="active"&&!f.puppet_in_room);
                if invalid&&matches!(current.state.as_str(),"joining"|"active") {
                    sql_query("UPDATE hagency_agent_v1.shared_agent_rooms SET state='leaving',generation=generation+1 WHERE id=$1").bind::<Text,_>(id).execute(db).await?;
                }
                let current=row(db,id).await?;
                if matches!(current.state.as_str(),"leaving"|"left") {
                    if f.puppet_in_room||invited {return Ok(Some((current.generation,"leave")));}
                    sql_query("UPDATE hagency_agent_v1.shared_agent_rooms SET state='left' WHERE id=$1 AND state='leaving'").bind::<Text,_>(id).execute(db).await?;
                } else if current.state=="joining" {
                    if f.puppet_in_room {
                        sql_query("UPDATE hagency_agent_v1.shared_agent_rooms SET state='active' WHERE id=$1 AND state='joining'").bind::<Text,_>(id).execute(db).await?;
                    } else if invited {return Ok(Some((current.generation,"join")));}
                    else if f.service_can_invite {return Ok(Some((current.generation,"invite")));}
                }
                Ok(None)
            }).await?
        };
        if let Some((generation, effect)) = effect {
            // Fence stale observations immediately before a remote effect.
            let valid = {
                let mut db = self.db.lock().await?;
                let current = row(&mut db, id).await?;
                current.generation == generation
                    && if effect == "leave" {
                        matches!(current.state.as_str(), "leaving" | "left")
                    } else {
                        current.state == "joining" && meta(&mut db, &current).await?.live
                    }
            };
            if valid {
                match effect {
                    "leave" => m.leave(&a.puppet_mxid, &b.room_id).await?,
                    "join" => m.managed_join(&a, &b.room_id).await?,
                    _ => m.managed_invite(&a, &b.room_id).await?,
                }
            }
        }
        Ok(())
    }
}
