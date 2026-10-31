//! New user-owned Agent domain. Matrix observations are produced only by the trusted
//! homeserver gateway; request bodies cannot supply authorization facts.
use crate::{Error, Result, entity_id, hash, key, store::Principal};
use diesel::{BoolExpressionMethods, ExpressionMethods, IntoSql, QueryDsl, SelectableHelper};

use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Nullable, Text},
};

use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};

use serde::{Deserialize, Serialize};

use std::{collections::BTreeSet, sync::Arc};

#[path = "domain_discovery.rs"]
mod discovery;
pub use discovery::RoomAgent;
#[path = "agent_crypto.rs"]
pub(crate) mod agent_crypto;
#[path = "execution_configuration.rs"]
mod execution_configuration;
#[path = "execution_devices.rs"]
mod execution_devices;
#[path = "owner_direct.rs"]
mod owner_direct;
pub(crate) use execution_devices::require_assigned;
pub use execution_devices::{OwnerDevice, SetExecutionDevice};

#[path = "project_board/mod.rs"]
pub mod project_board;

const MAX_FACT_AGE_MS: i64 = 30_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreationPolicy {
    pub default_allow: bool,
    pub allow: BTreeSet<String>,
    pub deny: BTreeSet<String>,
}

impl Default for CreationPolicy {
    fn default() -> Self {
        Self {
            default_allow: true,
            allow: BTreeSet::new(),
            deny: BTreeSet::new(),
        }
    }
}

impl CreationPolicy {
    fn permits(&self, mxid: &str) -> bool {
        !self.deny.contains(mxid) && (self.default_allow || self.allow.contains(mxid))
    }

    pub fn validate(&self) -> Result<()> {
        members(&self.allow)?;

        members(&self.deny)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomCreationPolicy {
    InheritProject {
        deny: BTreeSet<String>,
    },
    AllowList {
        allow: BTreeSet<String>,
        deny: BTreeSet<String>,
    },
    Disabled,
}

impl Default for RoomCreationPolicy {
    fn default() -> Self {
        Self::InheritProject {
            deny: BTreeSet::new(),
        }
    }
}

impl RoomCreationPolicy {
    fn permits(&self, mxid: &str) -> bool {
        match self {
            Self::InheritProject { deny } => !deny.contains(mxid),
            Self::AllowList { allow, deny } => allow.contains(mxid) && !deny.contains(mxid),
            Self::Disabled => false,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::InheritProject { deny } => members(deny),
            Self::AllowList { allow, deny } => {
                members(allow)?;

                members(deny)
            }

            Self::Disabled => Ok(()),
        }
    }
}

fn members(values: &BTreeSet<String>) -> Result<()> {
    if values.len() > 10_000 {
        return Err(Error::Invalid("policy_too_large"));
    }

    for value in values {
        matrix_id(value, '@')?;
    }

    Ok(())
}

fn matrix_id(value: &str, sigil: char) -> Result<()> {
    if value.len() > 255
        || !value.starts_with(sigil)
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
        || !(value[1..]
            .split_once(':')
            .is_some_and(|(local, server)| !local.is_empty() && !server.is_empty())
            || (sigil == '!'
                && value.len() == 44
                && value[1..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')))
    {
        return Err(Error::Invalid("invalid_matrix_identifier"));
    }

    Ok(())
}

fn fresh(observed: i64, now: i64) -> Result<()> {
    let now = now.max(crate::api::now_ms());

    if observed < 0 || observed > now || now - observed > MAX_FACT_AGE_MS {
        Err(Error::Unavailable("matrix_state_unavailable"))
    } else {
        Ok(())
    }
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| Error::Invalid("invalid_request"))
}

fn decode<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|_| Error::Unavailable("invalid_domain_storage"))
}

/// No Deserialize intentionally: only the trusted Matrix gateway constructs facts.
#[derive(Clone, Debug)]
pub struct AdminFacts {
    pub actor_mxid: String,
    pub room_id: String,
    pub observed_at_ms: i64,
    pub joined: bool,
    pub can_manage_policy: bool,
    pub is_space: bool,
    pub linked_space_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RoomFacts {
    pub owner_mxid: String,
    pub room_id: String,
    pub space_id: String,
    pub observed_at_ms: i64,
    pub owner_in_space: bool,
    pub owner_in_room: bool,
    pub room_in_space: bool,
    pub service_can_invite: bool,
    pub puppet_mxid: Option<String>,
    pub puppet_in_room: bool,
    pub encrypted: bool,
    pub owner_direct_valid: bool,
}

fn administer(p: &Principal, f: &AdminFacts, room: &str, now: i64) -> Result<()> {
    fresh(f.observed_at_ms, now)?;

    if f.actor_mxid != p.mxid || f.room_id != room || !f.joined || !f.can_manage_policy {
        return Err(Error::Unauthorized("room_administration_denied"));
    }

    Ok(())
}

fn membership(p: &Principal, f: &RoomFacts, room: &str, space: &str, now: i64) -> Result<()> {
    fresh(f.observed_at_ms, now)?;

    if f.owner_mxid != p.mxid
        || f.room_id != room
        || f.space_id != space
        || !f.owner_in_room
        || (if space.is_empty() {
            !f.owner_direct_valid
        } else {
            !f.owner_in_space || !f.room_in_space
        })
    {
        return Err(Error::Unauthorized("room_membership_required"));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn creating(
    p: &Principal,
    f: &RoomFacts,
    room: &str,
    space: &str,
    project: &CreationPolicy,
    policy: &RoomCreationPolicy,
    puppet: Option<&str>,
    now: i64,
) -> Result<()> {
    membership(p, f, room, space, now)?;

    if !project.permits(&p.mxid) || !policy.permits(&p.mxid) {
        return Err(Error::Unauthorized("agent_creation_denied"));
    }

    let already_joined =
        puppet.is_some_and(|mxid| f.puppet_in_room && f.puppet_mxid.as_deref() == Some(mxid));

    if !already_joined && !f.service_can_invite {
        return Err(Error::Unauthorized("service_invitation_denied"));
    }

    if f.encrypted && !(space.is_empty() && f.owner_direct_valid) {
        return Err(Error::Conflict("encrypted_room_requires_client_crypto"));
    }

    Ok(())
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
#[derive(diesel::Selectable)]
#[diesel(table_name = crate::schema::agent_tables::projects)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Project {
    #[diesel(sql_type=Bool)]
    pub archived: bool,
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub space_id: String,
    #[diesel(sql_type=Bool)]
    pub active: bool,
    #[diesel(sql_type=Text)]
    pub creation_policy: String,
    #[diesel(sql_type=BigInt)]
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
#[derive(diesel::Selectable)]
#[diesel(table_name = crate::schema::agent_tables::rooms)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Room {
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub project_id: String,
    #[diesel(sql_type=Bool)]
    pub active: bool,
    #[diesel(sql_type=Text)]
    pub creation_policy: String,
    #[diesel(sql_type=BigInt)]
    pub revision: i64,
}

/// Service audience is independent from ownership and execution state.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, diesel::FromSqlRow,
)]
#[serde(rename_all = "lowercase")]
pub enum AgentVisibility {
    Public,
    #[default]
    Private,
}
impl diesel::deserialize::FromSql<Text, diesel::pg::Pg> for AgentVisibility {
    fn from_sql(value: diesel::pg::PgValue<'_>) -> diesel::deserialize::Result<Self> {
        match value.as_bytes() {
            b"public" => Ok(Self::Public),
            b"private" => Ok(Self::Private),
            _ => Err("invalid agent visibility".into()),
        }
    }
}
impl AgentVisibility {
    pub fn is_private(&self) -> bool {
        *self == Self::Private
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
#[derive(diesel::Selectable)]
#[diesel(table_name = crate::schema::agent_tables::agents)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Agent {
    #[diesel(sql_type=Text)]
    pub visibility: AgentVisibility,
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub owner_user_id: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub display_name: String,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=BigInt)]
    pub generation: i64,
    #[diesel(sql_type=Nullable<Text>)]
    pub owner_direct_room_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pub execution_device_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, diesel::Queryable, diesel::QueryableByName)]
#[serde(rename_all = "camelCase")]
#[derive(diesel::Selectable)]
#[diesel(table_name = crate::schema::agent_tables::bindings)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Binding {
    #[diesel(sql_type=Text)]
    pub id: String,
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=Nullable<Text>)]
    pub project_id: Option<String>,
    #[diesel(sql_type=Text)]
    pub room_id: String,
    #[diesel(sql_type=Text)]
    pub state: String,
    #[diesel(sql_type=BigInt)]
    pub generation: i64,
    #[diesel(sql_type=Text)]
    pub scope_kind: String,
    #[diesel(sql_type=Bool)]
    pub owner_service_paused: bool,
    #[diesel(sql_type=Bool)]
    pub thread_auto_reply: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBinding {
    pub agent: Agent,
    pub binding: Binding,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommand {
    pub agent: Agent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<Binding>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateAgent {
    #[serde(default, skip_serializing_if = "AgentVisibility::is_private")]
    pub visibility: AgentVisibility,
    pub display_name: String,
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindRoom {
    pub project_id: String,
    pub room_id: String,
    pub idempotency_key: String,
}

/// Trusted lifecycle worker input. It is not a user credential or public DTO.
#[derive(diesel::Queryable, diesel::QueryableByName)]
pub struct CleanupScope {
    #[diesel(sql_type=Text)]
    pub agent_id: String,
    #[diesel(sql_type=Text)]
    pub owner_user_id: String,
    #[diesel(sql_type=Text)]
    pub owner_mxid: String,
    #[diesel(sql_type=Text)]
    pub puppet_mxid: String,
    #[diesel(sql_type=Text)]
    pub agent_state: String,
    #[diesel(sql_type=BigInt)]
    pub agent_generation: i64,
    #[diesel(sql_type=Nullable<Text>)]
    pub binding_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pub binding_state: Option<String>,
    #[diesel(sql_type=Nullable<BigInt>)]
    pub binding_generation: Option<i64>,
    #[diesel(sql_type=Nullable<Text>)]
    pub room_id: Option<String>,
    #[diesel(sql_type=Nullable<Text>)]
    pub space_id: Option<String>,
}
#[derive(diesel::Queryable, diesel::QueryableByName)]
struct Command {
    #[diesel(sql_type=Text)]
    digest: String,
    #[diesel(sql_type=Text)]
    agent_id: String,
    #[diesel(sql_type=Nullable<Text>)]
    binding_id: Option<String>,
}

#[derive(diesel::Queryable, diesel::QueryableByName)]
struct Flag {
    #[diesel(sql_type=Bool)]
    matched: bool,
}

#[derive(Clone)]
pub struct DomainStore {
    pub(crate) board: project_board::BoardStore,
    db: Arc<crate::database::Database>,
    crypto_db: Arc<crate::database::DedicatedDatabase>,
    controller_db: Arc<crate::database::DedicatedDatabase>,
    server: String,
    namespace: String,
}

impl DomainStore {
    /// Trusted Appservice existence query; cannot allocate an ownerless puppet.
    pub async fn known_puppet(&self, mxid: &str) -> Result<bool> {
        self.db
            .with_connection(async |db| {
                Ok(sql_query(
            "SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.agents WHERE puppet_mxid=$1) AS matched",
        )
        .bind::<Text, _>(mxid)
        .get_result::<Flag>(&mut *db)
        .await?
        .matched)
            })
            .await
    }
    /// Authentication schema must be initialized first. Fixed server identity is never rewritten.
    pub async fn open(url: &str, server: &str, namespace: &str) -> Result<Self> {
        matrix_id(&format!("@service:{server}"), '@')?;

        if !namespace.starts_with("_hagency_")
            || namespace.len() > 64
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(Error::Invalid("invalid_agent_namespace"));
        }

        let mut db = AsyncPgConnection::establish(url)
            .await
            .map_err(|_| Error::Unavailable("database_unavailable"))?;

        db.transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection| {

            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328902)")
                .await?;

            let fixed={
                use crate::schema::agent_tables::deployment as q;
                let params = (server,);
                q::table
                    .select(((((q::version).eq(1)).and((q::server_name).eq(&params.0))),))
                    .filter(q::singleton)
                    .get_result::<Flag>(db)
                    .await
            }?;

            if !fixed.matched {
                return Err(Error::Conflict("deployment_identity_mismatch"));
            }

            let initialized = sql_query(
                "SELECT to_regclass('hagency_agent_v1.domain_deployment') IS NOT NULL AS matched",
            )
            .get_result::<Flag>(db)
            .await?;

            if !initialized.matched {
                db.batch_execute(include_str!("domain_schema.sql")).await?;
                db.batch_execute(include_str!("owner_direct_schema.sql")).await?;
            }
            db.batch_execute(include_str!("configuration_schema.sql")).await?;
            db.batch_execute(include_str!("visibility_schema.sql")).await?;
            db.batch_execute(include_str!("agent_crypto_schema.sql")).await?;
            let pause_initialized=sql_query("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='bindings' AND column_name='owner_service_paused') AS matched").get_result::<Flag>(db).await?;
            if !pause_initialized.matched {
                db.batch_execute("ALTER TABLE hagency_agent_v1.bindings ADD COLUMN owner_service_paused boolean NOT NULL DEFAULT false").await?;
            }
            db.batch_execute("ALTER TABLE hagency_agent_v1.bindings ADD COLUMN IF NOT EXISTS thread_auto_reply boolean NOT NULL DEFAULT false").await?;
            {
                use crate::schema::agent_tables::domain_deployment as q;
                let params = (namespace,);
                diesel::insert_into(q::table)
                    .values((
                        q::singleton.eq(true),
                        q::version.eq(4),
                        q::namespace.eq(&params.0),
                    ))
                    .on_conflict((q::singleton,))
                    .do_nothing()
                    .execute(db)
                    .await
            }?;
            let fixed={
                use crate::schema::agent_tables::domain_deployment as q;
                let params = (namespace,);
                q::table
                    .select(((((q::version).eq(4)).and((q::namespace).eq(&params.0))),))
                    .filter(q::singleton)
                    .get_result::<Flag>(db)
                    .await
            }?;
            if !fixed.matched {return Err(Error::Conflict("domain_schema_incompatible"));}
            let schema=sql_query("SELECT to_regclass('hagency_agent_v1.scope_pauses') IS NOT NULL AND EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='hagency_agent_v1' AND table_name='agents' AND column_name='execution_device_id') AS matched").get_result::<Flag>(db).await?;
            if !schema.matched {return Err(Error::Conflict("domain_schema_incompatible"));}
            Ok(())
        }
).await?;

        // Crypto HTTP holds only its own connection while actual authorization
        // rows stay locked; unrelated domain work must not queue behind it.
        let crypto_db = AsyncPgConnection::establish(url)
            .await
            .map_err(|_| Error::Unavailable("database_unavailable"))?;
        Ok(Self {
            crypto_db: Arc::new(crate::database::DedicatedDatabase::new(crypto_db, url)),
            controller_db: Arc::new(crate::database::DedicatedDatabase::new(db, url)),
            board: project_board::BoardStore::open(url).await?,
            db: Arc::new(crate::database::Database::business(url)?),
            server: server.into(),
            namespace: namespace.into(),
        })
    }

    async fn authorize(db: &mut AsyncPgConnection, p: &Principal, now: i64) -> Result<()> {
        // The transaction lock serializes cross-process domain mutations. Identity/session
        // row locks prevent concurrent account or session revocation during a mutation.
        db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
            .await?;

        Self::authorize_identity(db, p, now).await
    }

    pub(crate) async fn authorize_identity(
        db: &mut AsyncPgConnection,
        p: &Principal,
        now: i64,
    ) -> Result<()> {
        let valid=sql_query("SELECT true AS matched FROM hagency_agent_v1.sessions s JOIN hagency_agent_v1.users u ON u.id=s.user_id WHERE s.id=$1 AND u.id=$2 AND u.mxid=$3 AND s.client_id=$4 AND u.active AND NOT s.revoked AND s.valid_until_ms>greatest($5,(extract(epoch from clock_timestamp())*1000)::bigint) FOR SHARE OF s,u")
   .bind::<Text,_>(&p.session_id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.mxid).bind::<Text,_>(&p.client_id).bind::<BigInt,_>(now).get_result::<Flag>(db).await;

        match valid {
            Ok(_) => (),
            Err(diesel::result::Error::NotFound) => {
                return Err(Error::Unauthorized("authorization_expired"));
            }

            Err(e) => return Err(e.into()),
        }

        match (&p.device_id, p.device_generation) {
            (None, None) => Ok(()),
            (Some(id), Some(generation)) => {
                let current=sql_query("SELECT true AS matched FROM hagency_agent_v1.devices d WHERE d.id=$1 AND d.user_id=$2 AND d.session_id=$3 AND d.generation=$4 AND NOT d.revoked FOR SHARE OF d")
                    .bind::<Text,_>(id).bind::<Text,_>(&p.user_id).bind::<Text,_>(&p.session_id).bind::<BigInt,_>(generation).get_result::<Flag>(db).await;

                match current {
                    Ok(_) => Ok(()),
                    Err(diesel::result::Error::NotFound) => {
                        Err(Error::Unauthorized("device_authorization_expired"))
                    }

                    Err(e) => Err(e.into()),
                }
            }

            _ => Err(Error::Unauthorized("invalid_device_authorization")),
        }?;
        // SQL predicates may have been evaluated before a row-lock wait. All
        // identity/device rows are locked now, so recheck deadline without waiting.
        let expiry=sql_query("SELECT valid_until_ms>greatest($2,(extract(epoch from clock_timestamp())*1000)::bigint) AS matched FROM hagency_agent_v1.sessions WHERE id=$1")
            .bind::<Text,_>(&p.session_id).bind::<BigInt,_>(now).get_result::<Flag>(db).await?;
        if !expiry.matched {
            return Err(Error::Unauthorized("authorization_expired"));
        }
        Ok(())
    }

    async fn audit(
        db: &mut AsyncPgConnection,
        p: &Principal,
        op: &str,
        id: &str,
        now: i64,
    ) -> Result<()> {
        {
            use crate::schema::agent_tables::domain_audit as q;
            let params = (&p.user_id, op, id, now);
            diesel::insert_into(q::table)
                .values((
                    q::actor_user_id.eq(&params.0),
                    q::operation.eq(&params.1),
                    q::object_id.eq(&params.2),
                    q::at_ms.eq(&params.3),
                ))
                .execute(db)
                .await
        }?;

        Ok(())
    }

    async fn project_db(db: &mut AsyncPgConnection, id: &str) -> Result<Project> {
        {
            use crate::schema::agent_tables::projects as q;
            let params = (id,);
            q::table
                .select(crate::domain::Project::as_select())
                .filter((q::id).eq(&params.0))
                .get_result(db)
                .await
        }
        .map_err(hidden)
    }

    async fn room_db(db: &mut AsyncPgConnection, id: &str, project: &str) -> Result<Room> {
        {
            use crate::schema::agent_tables::rooms as q;
            let params = (id, project);
            q::table
                .select(crate::domain::Room::as_select())
                .filter(((q::room_id).eq(&params.0)).and((q::project_id).eq(&params.1)))
                .get_result(db)
                .await
        }
        .map_err(hidden)
    }

    async fn agent_db(db: &mut AsyncPgConnection, p: &Principal, id: &str) -> Result<Agent> {
        {
            use crate::schema::agent_tables::agents as q;
            let params = (id, &p.user_id);
            q::table
                .select(crate::domain::Agent::as_select())
                .filter(((q::id).eq(&params.0)).and((q::owner_user_id).eq(&params.1)))
                .get_result(db)
                .await
        }
        .map_err(hidden)
    }

    async fn binding_db(db: &mut AsyncPgConnection, p: &Principal, id: &str) -> Result<Binding> {
        sql_query("SELECT b.id,b.agent_id,b.project_id,b.room_id,b.state,b.generation,b.scope_kind,b.owner_service_paused,b.thread_auto_reply FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id WHERE b.id=$1 AND a.owner_user_id=$2").bind::<Text,_>(id).bind::<Text,_>(&p.user_id).get_result(db).await.map_err(hidden)
    }

    async fn policies(
        db: &mut AsyncPgConnection,
        project_id: &str,
        room_id: &str,
    ) -> Result<(Project, CreationPolicy, RoomCreationPolicy)> {
        let project = Self::project_db(db, project_id).await?;

        let room = Self::room_db(db, room_id, project_id).await?;

        if !project.active || !room.active {
            return Err(Error::Unauthorized("project_or_room_disabled"));
        }

        let unpaused=sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.scope_pauses WHERE paused AND ((kind='project' AND scope_id=$1) OR (kind='room' AND scope_id=$2))) AS matched")
            .bind::<Text,_>(project_id).bind::<Text,_>(room_id).get_result::<Flag>(db).await?;
        if !unpaused.matched {
            return Err(Error::Unauthorized("administrator_pause_active"));
        }
        let pp = decode(&project.creation_policy)?;

        let rp = decode(&room.creation_policy)?;

        Ok((project, pp, rp))
    }

    async fn binding_policies(
        db: &mut AsyncPgConnection,
        b: &Binding,
    ) -> Result<(String, CreationPolicy, RoomCreationPolicy)> {
        if let Some(project) = &b.project_id {
            {
                use crate::schema::agent_tables::agents as q;
                let params = (&b.agent_id,);
                q::table
                    .select((true.into_sql::<diesel::sql_types::Bool>(),))
                    .filter(((q::id).eq(&params.0)).and((q::visibility).eq("public")))
                    .get_result::<Flag>(db)
                    .await
            }
            .map_err(hidden)?;
            let (p, pp, rp) = Self::policies(db, project, &b.room_id).await?;
            return Ok((p.space_id, pp, rp));
        }
        let valid = {
            use crate::schema::agent_tables::agents as q;
            let params = (&b.agent_id, &b.room_id);
            q::table
                .select((true.into_sql::<diesel::sql_types::Bool>(),))
                .filter(((q::id).eq(&params.0)).and((q::owner_direct_room_id).eq(&params.1)))
                .get_result::<Flag>(db)
                .await
        }
        .map_err(hidden)?;
        if b.scope_kind != "owner_direct" || !valid.matched {
            return Err(Error::Unauthorized("direct_scope_invalid"));
        }
        Ok((
            String::new(),
            CreationPolicy::default(),
            RoomCreationPolicy::default(),
        ))
    }
    pub async fn register_project(
        &self,
        p: &Principal,
        space: &str,
        f: &AdminFacts,
        now: i64,
    ) -> Result<Project> {
        matrix_id(space, '!')?;

        if !f.is_space {
            return Err(Error::Invalid("project_requires_space"));
        }

        self.db.with_connection(async |db| {


        db.transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection| {

            Self::authorize(db, p, now).await?;
            administer(p, f, space, now)?;

            let project=sql_query("INSERT INTO hagency_agent_v1.projects(id,space_id,creation_policy) VALUES($1,$2,$3) ON CONFLICT(space_id) DO UPDATE SET space_id=EXCLUDED.space_id RETURNING id,space_id,active,archived,creation_policy,revision").bind::<Text,_>(format!("prj_{}",entity_id()?)).bind::<Text,_>(space).bind::<Text,_>(encode(&CreationPolicy::default())?).get_result::<Project>(db).await?;

            Self::audit(db, p, "project.register", &project.id, now).await?;
            Ok(project)
        }
).await
    }).await
    }

    pub async fn register_room(
        &self,
        p: &Principal,
        project_id: &str,
        room: &str,
        sf: &AdminFacts,
        rf: &AdminFacts,
        now: i64,
    ) -> Result<Room> {
        matrix_id(room, '!')?;

        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let project = Self::project_db(db, project_id).await?;

                    administer(p, sf, &project.space_id, now)?;
                    administer(p, rf, room, now)?;

                    if !sf.is_space
                        || rf.is_space
                        || rf.linked_space_id.as_deref() != Some(&project.space_id)
                    {
                        return Err(Error::Invalid("room_requires_space_child"));
                    }

                    let existing = {
                        use crate::schema::agent_tables::rooms as q;
                        let params = (room,);
                        q::table
                            .select(crate::domain::Room::as_select())
                            .filter((q::room_id).eq(&params.0))
                            .get_result::<Room>(db)
                            .await
                    }
                    .optional()?;

                    if let Some(existing) = existing {
                        if existing.project_id != project_id {
                            return Err(Error::Conflict("room_already_registered"));
                        }
                        return Ok(existing);
                    }

                    let result = {
                        use crate::schema::agent_tables::rooms as q;
                        let params = (room, project_id, encode(&RoomCreationPolicy::default())?);
                        diesel::insert_into(q::table)
                            .values((
                                q::room_id.eq(&params.0),
                                q::project_id.eq(&params.1),
                                q::creation_policy.eq(&params.2),
                            ))
                            .returning(crate::domain::Room::as_select())
                            .get_result::<Room>(db)
                            .await
                    }?;
                    Self::audit(db, p, "room.register", room, now).await?;
                    Ok(result)
                })
                .await
            })
            .await
    }

    pub async fn set_project_policy(
        &self,
        p: &Principal,
        id: &str,
        expected: i64,
        policy: CreationPolicy,
        f: &AdminFacts,
        now: i64,
    ) -> Result<Project> {
        policy.validate()?;

        let encoded = encode(&policy)?;

        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let project = Self::project_db(db, id).await?;
                    administer(p, f, &project.space_id, now)?;

                    let updated = {
                        use crate::schema::agent_tables::projects as q;
                        let params = (encoded, id, expected);
                        diesel::update(q::table)
                            .filter(((q::id).eq(&params.1)).and((q::revision).eq(&params.2)))
                            .set((
                                q::creation_policy.eq(&params.0),
                                q::revision.eq(q::revision + 1),
                            ))
                            .returning(crate::domain::Project::as_select())
                            .get_result(db)
                            .await
                    }
                    .map_err(revision)?;
                    Self::audit(db, p, "project.creation_policy", id, now).await?;
                    Ok(updated)
                })
                .await
            })
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_room_policy(
        &self,
        p: &Principal,
        room: &str,
        project_id: &str,
        expected: i64,
        policy: RoomCreationPolicy,
        f: &AdminFacts,
        now: i64,
    ) -> Result<Room> {
        policy.validate()?;

        let encoded = encode(&policy)?;

        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    Self::room_db(db, room, project_id).await?;
                    administer(p, f, room, now)?;

                    let updated = {
                        use crate::schema::agent_tables::rooms as q;
                        let params = (encoded, room, project_id, expected);
                        diesel::update(q::table)
                            .filter(
                                (((q::room_id).eq(&params.1)).and((q::project_id).eq(&params.2)))
                                    .and((q::revision).eq(&params.3)),
                            )
                            .set((
                                q::creation_policy.eq(&params.0),
                                q::revision.eq(q::revision + 1),
                            ))
                            .returning(crate::domain::Room::as_select())
                            .get_result(db)
                            .await
                    }
                    .map_err(revision)?;
                    Self::audit(db, p, "room.creation_policy", room, now).await?;
                    Ok(updated)
                })
                .await
            })
            .await
    }

    pub async fn command_status(
        &self,
        p: &Principal,
        operation: &str,
        command_key: &str,
        now: i64,
    ) -> Result<AgentCommand> {
        if !matches!(operation, "agent.create" | "agent.bind") {
            return Err(Error::Invalid("unsupported_command"));
        }
        key(command_key)?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let command = {
                        use crate::schema::agent_tables::domain_commands as q;
                        let params = (&p.user_id, operation, command_key);
                        q::table
                            .select((q::digest, q::agent_id, q::binding_id))
                            .filter(
                                (((q::actor_user_id).eq(&params.0))
                                    .and((q::operation).eq(&params.1)))
                                .and((q::key).eq(&params.2)),
                            )
                            .get_result::<Command>(db)
                            .await
                    }
                    .optional()?
                    .ok_or(Error::NotFound("command_not_found"))?;
                    Ok(AgentCommand {
                        agent: Self::agent_db(db, p, &command.agent_id).await?,
                        binding: match command.binding_id {
                            Some(id) => Some(Self::binding_db(db, p, &id).await?),
                            None => None,
                        },
                    })
                })
                .await
            })
            .await
    }
    async fn replay(
        db: &mut AsyncPgConnection,
        p: &Principal,
        op: &str,
        k: &str,
        digest: &str,
    ) -> Result<Option<AgentBinding>> {
        let c = {
            use crate::schema::agent_tables::domain_commands as q;
            let params = (&p.user_id, op, k);
            q::table
                .select((q::digest, q::agent_id, q::binding_id))
                .filter(
                    (((q::actor_user_id).eq(&params.0)).and((q::operation).eq(&params.1)))
                        .and((q::key).eq(&params.2)),
                )
                .get_result::<Command>(db)
                .await
        }
        .optional()?;

        match c {
            None => Ok(None),
            Some(c) => {
                if c.digest != digest {
                    return Err(Error::Conflict("idempotency_key_reused"));
                }

                let binding_id = c
                    .binding_id
                    .ok_or(Error::Unavailable("invalid_domain_storage"))?;

                Ok(Some(AgentBinding {
                    agent: Self::agent_db(db, p, &c.agent_id).await?,
                    binding: Self::binding_db(db, p, &binding_id).await?,
                }))
            }
        }
    }

    async fn command(
        db: &mut AsyncPgConnection,
        p: &Principal,
        op: &str,
        k: &str,
        digest: &str,
        agent: &str,
        binding: &str,
    ) -> Result<()> {
        {
            use crate::schema::agent_tables::domain_commands as q;
            let params = (&p.user_id, op, k, digest, agent, binding);
            diesel::insert_into(q::table)
                .values((
                    q::actor_user_id.eq(&params.0),
                    q::operation.eq(&params.1),
                    q::key.eq(&params.2),
                    q::digest.eq(&params.3),
                    q::agent_id.eq(&params.4),
                    q::binding_id.eq(&params.5),
                ))
                .execute(db)
                .await
        }?;

        Ok(())
    }

    /// Allocate only permanent owner-bound identity. Room admission is a separate command.
    pub async fn create_agent(
        &self,
        p: &Principal,
        request: CreateAgent,
        now: i64,
    ) -> Result<Agent> {
        key(&request.idempotency_key)?;
        if request.display_name.trim().is_empty()
            || request.display_name.chars().count() > 64
            || request.display_name.chars().any(char::is_control)
        {
            return Err(Error::Invalid("invalid_agent_name"));
        }
        let digest = hash(&encode(&request)?);
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,now).await?;
            if let Some(command)={
                use crate::schema::agent_tables::domain_commands as q;
                let params = (&p.user_id, &request.idempotency_key);
                q::table
                    .select((q::digest, q::agent_id, q::binding_id))
                    .filter(
                        (((q::actor_user_id).eq(&params.0)).and((q::operation).eq("agent.create")))
                            .and((q::key).eq(&params.1)),
                    )
                    .get_result::<Command>(db)
                    .await
            }.optional()? {
                if command.digest!=digest {return Err(Error::Conflict("idempotency_key_reused"));}
                return Self::agent_db(db,p,&command.agent_id).await;
            }
            let device=p.device_id.as_deref().ok_or(Error::Unauthorized("device_authorization_required"))?;
            {
                use crate::schema::agent_tables::devices as q;
                let params = (device, &p.user_id);
                q::table
                    .select((true.into_sql::<diesel::sql_types::Bool>(),))
                    .filter(
                        (((q::id).eq(&params.0)).and((q::user_id).eq(&params.1)))
                            .and(diesel::dsl::not(q::revoked)),
                    )
                    .for_share()
                    .get_result::<Flag>(db)
                    .await
            }.map_err(hidden)?;
            let id=format!("agt_{}",entity_id()?);let mxid=format!("@{}{id}:{}",self.namespace,self.server);
            let agent={
                use crate::schema::agent_tables::agents as q;
                let params = (
                    &id,
                    &p.user_id,
                    mxid,
                    request.display_name.trim(),
                    device,
                    request.visibility.as_str(),
                );
                diesel::insert_into(q::table)
                    .values((
                        q::id.eq(&params.0),
                        q::owner_user_id.eq(&params.1),
                        q::puppet_mxid.eq(&params.2),
                        q::display_name.eq(&params.3),
                        q::state.eq("creating"),
                        q::execution_device_id.eq(&params.4),
                        q::visibility.eq(&params.5),
                    ))
                    .returning(crate::domain::Agent::as_select())
                    .get_result::<Agent>(db)
                    .await
            }?;
            sql_query("INSERT INTO hagency_agent_v1.domain_commands(actor_user_id,operation,key,digest,agent_id,binding_id) VALUES($1,'agent.create',$2,$3,$4,NULL)").bind::<Text,_>(&p.user_id).bind::<Text,_>(&request.idempotency_key).bind::<Text,_>(digest).bind::<Text,_>(&id).execute(db).await?;
            Self::audit(db,p,"agent.create",&id,now).await?;Ok(agent)
        }).await
    }).await
    }
    pub async fn identity_provisioning_candidates(
        &self,
        after: &str,
        limit: i64,
    ) -> Result<Vec<Agent>> {
        if !(1..=128).contains(&limit) {
            return Err(Error::Invalid("invalid_limit"));
        }
        self.db.with_connection(async |db| {

        Ok(sql_query("SELECT a.id,a.owner_user_id,a.puppet_mxid,a.display_name,a.state,a.generation,a.owner_direct_room_id,a.execution_device_id,a.visibility FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id WHERE a.state='creating' AND u.active ORDER BY (a.id<=$1),a.id LIMIT $2").bind::<Text,_>(after).bind::<BigInt,_>(limit).load(&mut *db).await?)
    }).await
    }
    /// Only after the trusted worker verified the exact puppet through Matrix.
    pub async fn confirm_identity_provisioned(&self, id: &str, generation: i64) -> Result<()> {
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            let n=sql_query("UPDATE hagency_agent_v1.agents a SET state='active' WHERE id=$1 AND generation=$2 AND state='creating' AND EXISTS(SELECT 1 FROM hagency_agent_v1.users u WHERE u.id=a.owner_user_id AND u.active)").bind::<Text,_>(id).bind::<BigInt,_>(generation).execute(db).await?;
            if n!=1 {return Err(Error::Conflict("provisioning_intent_stale"));}Ok(())
        }).await
    }).await
    }

    async fn insert_binding(
        db: &mut AsyncPgConnection,
        agent: &str,
        project: &str,
        room: &str,
    ) -> Result<Binding> {
        Ok({
            use crate::schema::agent_tables::bindings as q;
            let params = (format!("bnd_{}", entity_id()?), agent, project, room);
            diesel::insert_into(q::table)
                .values((
                    q::id.eq(&params.0),
                    q::agent_id.eq(&params.1),
                    q::project_id.eq(&params.2),
                    q::room_id.eq(&params.3),
                    q::state.eq("joining"),
                ))
                .returning(crate::domain::Binding::as_select())
                .get_result(db)
                .await
        }?)
    }

    pub async fn bind_room(
        &self,
        p: &Principal,
        agent_id: &str,
        request: BindRoom,
        f: &RoomFacts,
        now: i64,
    ) -> Result<AgentBinding> {
        key(agent_id)?;

        key(&request.project_id)?;

        key(&request.idempotency_key)?;

        matrix_id(&request.room_id, '!')?;

        let digest = hash(&format!("{agent_id}:{}", encode(&request)?));

        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;

                    {
                        use crate::schema::agent_tables::agents as q;
                        let params = (agent_id, &p.user_id);
                        q::table
                            .select((true.into_sql::<diesel::sql_types::Bool>(),))
                            .filter(((q::id).eq(&params.0)).and((q::owner_user_id).eq(&params.1)))
                            .for_update()
                            .get_result::<Flag>(db)
                            .await
                    }
                    .map_err(hidden)?;
                    let agent = Self::agent_db(db, p, agent_id).await?;
                    if agent.visibility != AgentVisibility::Public {
                        return Err(Error::Unauthorized("private_agent_owner_direct_only"));
                    }
                    if let Some(replay) =
                        Self::replay(db, p, "agent.bind", &request.idempotency_key, &digest).await?
                    {
                        return Ok(replay);
                    }

                    if !matches!(agent.state.as_str(), "creating" | "active" | "suspended") {
                        return Err(Error::Conflict("agent_retired"));
                    }

                    let (project, pp, rp) =
                        Self::policies(db, &request.project_id, &request.room_id).await?;
                    creating(
                        p,
                        f,
                        &request.room_id,
                        &project.space_id,
                        &pp,
                        &rp,
                        Some(&agent.puppet_mxid),
                        now,
                    )?;

                    let existing = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (agent_id, &request.room_id);
                        q::table
                            .select(crate::domain::Binding::as_select())
                            .filter(((q::agent_id).eq(&params.0)).and((q::room_id).eq(&params.1)))
                            .get_result::<Binding>(db)
                            .await
                    }
                    .optional()?;

                    if let Some(b) = &existing {
                        if b.scope_kind != "project"
                            || b.project_id.as_deref() != Some(request.project_id.as_str())
                        {
                            return Err(Error::Conflict("room_binding_scope_conflict"));
                        }
                        let pause = {
                            use crate::schema::agent_tables::bindings as q;
                            let params = (&b.id,);
                            q::table
                                .select((diesel::dsl::not(
                                    (q::admin_project_paused).or(q::admin_room_paused),
                                ),))
                                .filter((q::id).eq(&params.0))
                                .get_result::<Flag>(db)
                                .await
                        }?;
                        if !pause.matched {
                            return Err(Error::Unauthorized("administrator_pause_active"));
                        }
                    }

                    let binding = match existing {
                        Some(b)
                            if matches!(b.state.as_str(), "joining" | "active" | "suspended") =>
                        {
                            b
                        }
                        Some(b) if b.state == "left" => {
                            use crate::schema::agent_tables::bindings as q;
                            let params = (&b.id,);
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
                        }?,
                        Some(_) => return Err(Error::Conflict("binding_not_bindable")),
                        None => {
                            Self::insert_binding(
                                db,
                                agent_id,
                                &request.project_id,
                                &request.room_id,
                            )
                            .await?
                        }
                    };

                    Self::command(
                        db,
                        p,
                        "agent.bind",
                        &request.idempotency_key,
                        &digest,
                        agent_id,
                        &binding.id,
                    )
                    .await?;
                    Self::audit(db, p, "agent.bind", &binding.id, now).await?;
                    Ok(AgentBinding { agent, binding })
                })
                .await
            })
            .await
    }

    /// Background cleanup only. User session expiry must not strand irrevocable
    /// retirement or room departure; these rows authorize only convergence of an
    /// already requested transition, never creation, transfer or resumption.
    pub async fn cleanup_scopes(&self, limit: usize) -> Result<Vec<CleanupScope>> {
        self.cleanup_scopes_after("", "", limit).await
    }
    pub async fn cleanup_scopes_after(
        &self,
        agent_cursor: &str,
        binding_cursor: &str,
        limit: usize,
    ) -> Result<Vec<CleanupScope>> {
        if limit == 0 || limit > 1000 {
            return Err(Error::Invalid("invalid_cleanup_limit"));
        }
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            Ok(sql_query("SELECT a.id AS agent_id,a.owner_user_id,u.mxid AS owner_mxid,a.puppet_mxid,a.state AS agent_state,a.generation AS agent_generation,b.id AS binding_id,b.state AS binding_state,b.generation AS binding_generation,b.room_id,coalesce(p.space_id,'') AS space_id FROM hagency_agent_v1.agents a JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.bindings b ON b.agent_id=a.id AND b.state IN ('leaving','revoked') LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id WHERE (b.id IS NOT NULL OR (a.state='retiring' AND NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings pending WHERE pending.agent_id=a.id AND pending.state<>'left'))) AND (a.id,coalesce(b.id,''))>($2,$3) ORDER BY a.id,coalesce(b.id,'') LIMIT $1").bind::<BigInt,_>(limit as i64).bind::<Text,_>(agent_cursor).bind::<Text,_>(binding_cursor).load(db).await?)
        }).await
    }).await
    }
    /// Session-scoped PostgreSQL lock elects one Matrix membership controller
    /// across replicas. Recheck after connection replacement without stacking
    /// PostgreSQL's reentrant lock count on the same connection.
    pub async fn try_membership_controller(&self) -> Result<bool> {
        let mut db = self.controller_db.lock().await?;
        Ok(
            sql_query("SELECT CASE WHEN EXISTS (SELECT 1 FROM pg_locks WHERE locktype='advisory' AND pid=pg_backend_pid() AND classid=(5210750088328905::bigint >> 32)::oid AND objid=(5210750088328905::bigint & 4294967295)::oid AND objsubid=1 AND granted) THEN true ELSE pg_try_advisory_lock(5210750088328905) END AS matched")
                .get_result::<Flag>(&mut *db)
                .await?
                .matched,
        )
    }
    /// Bounded cursor scan of committed desired membership, including terminal
    /// departures. A late Matrix join cannot escape reconciliation after a crash.
    pub async fn reconciliation_scopes(
        &self,
        after: &str,
        limit: usize,
    ) -> Result<Vec<CleanupScope>> {
        if limit == 0 || limit > 1000 {
            return Err(Error::Invalid("invalid_cleanup_limit"));
        }
        self.db.with_connection(async |db| {

        Ok(sql_query("SELECT a.id AS agent_id,a.owner_user_id,u.mxid AS owner_mxid,a.puppet_mxid,a.state AS agent_state,a.generation AS agent_generation,b.id AS binding_id,b.state AS binding_state,b.generation AS binding_generation,b.room_id,coalesce(p.space_id,'') AS space_id FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id WHERE b.id>$1 AND b.state IN ('joining','active','suspended','leaving','left','revoked') ORDER BY b.id LIMIT $2")
            .bind::<Text,_>(after).bind::<BigInt,_>(limit as i64).load(&mut *db).await?)
    }).await
    }
    /// Commit a fresh, trusted loss of membership as a durable departure.
    /// This never resumes a binding and ignores creation-only policy changes.
    /// Other bindings of the same Agent retain independent running authority.
    pub async fn retire_membership_loss_trusted(
        &self,
        id: &str,
        generation: i64,
        facts: &RoomFacts,
        now: i64,
    ) -> Result<bool> {
        self.retire_membership_loss(id, generation, facts, false, now)
            .await
    }

    async fn retire_membership_loss(
        &self,
        id: &str,
        generation: i64,
        facts: &RoomFacts,
        require_joined: bool,
        now: i64,
    ) -> Result<bool> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                        .await?;
                    Self::fence_membership_loss(
                        db,
                        id,
                        Some(generation),
                        facts,
                        require_joined,
                        now,
                    )
                    .await
                })
                .await
            })
            .await
    }

    async fn observe_owner_membership(
        &self,
        p: &Principal,
        id: &str,
        facts: &RoomFacts,
        now: i64,
    ) -> Result<()> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let binding = Self::binding_db(db, p, id).await?;
                    Self::fence_membership_loss(
                        db,
                        id,
                        Some(binding.generation),
                        facts,
                        false,
                        now,
                    )
                    .await?;
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Shared with transport's separately committed observation transaction.
    /// Caller holds the domain/transport advisory transaction lock. Returning a
    /// later authorization error in the same transaction would undo this fence.
    pub(crate) async fn fence_membership_loss(
        db: &mut AsyncPgConnection,
        id: &str,
        generation: Option<i64>,
        f: &RoomFacts,
        require_joined: bool,
        now: i64,
    ) -> Result<bool> {
        #[derive(diesel::Queryable, diesel::QueryableByName)]
        struct Scope {
            #[diesel(sql_type=Text)]
            owner_user_id: String,
            #[diesel(sql_type=Text)]
            owner_mxid: String,
            #[diesel(sql_type=Text)]
            puppet_mxid: String,
            #[diesel(sql_type=Text)]
            room_id: String,
            #[diesel(sql_type=Text)]
            space_id: String,
            #[diesel(sql_type=Text)]
            state: String,
            #[diesel(sql_type=BigInt)]
            generation: i64,
            #[diesel(sql_type=Bool)]
            active: bool,
        }
        let s = sql_query("SELECT a.owner_user_id,u.mxid AS owner_mxid,a.puppet_mxid,b.room_id,coalesce(p.space_id,'') AS space_id,b.state,b.generation,(u.active AND ((b.scope_kind='project' AND a.visibility='public' AND p.active AND r.active) OR (b.scope_kind='owner_direct' AND a.owner_direct_room_id=b.room_id))) AS active FROM hagency_agent_v1.bindings b JOIN hagency_agent_v1.agents a ON a.id=b.agent_id JOIN hagency_agent_v1.users u ON u.id=a.owner_user_id LEFT JOIN hagency_agent_v1.projects p ON p.id=b.project_id LEFT JOIN hagency_agent_v1.rooms r ON r.room_id=b.room_id AND r.project_id=b.project_id WHERE b.id=$1")
            .bind::<Text,_>(id).get_result::<Scope>(db).await.map_err(hidden)?;
        fresh(f.observed_at_ms, now)?;
        if generation.is_some_and(|g| g != s.generation)
            || f.owner_mxid != s.owner_mxid
            || f.puppet_mxid.as_deref() != Some(&s.puppet_mxid)
            || f.room_id != s.room_id
            || f.space_id != s.space_id
        {
            return Err(Error::Conflict("stale_membership_observation"));
        }
        // Missing puppet membership is normal before JOIN, but not after the
        // trusted provisioner completed it and requests final activation.
        if !matches!(s.state.as_str(), "joining" | "active" | "suspended")
            || (s.active
                && f.owner_in_room
                && ((s.state == "joining" && !require_joined) || f.puppet_in_room)
                && (if s.space_id.is_empty() {
                    f.owner_direct_valid
                } else {
                    f.owner_in_space && f.room_in_space
                }))
        {
            return Ok(false);
        }
        {
            use crate::schema::agent_tables::bindings as q;
            let params = (id,);
            diesel::update(q::table)
                .filter((q::id).eq(&params.0))
                .set((q::state.eq("leaving"), q::generation.eq(q::generation + 1)))
                .execute(db)
                .await
        }?;
        {
            use crate::schema::agent_tables::domain_audit as q;
            let params = (&s.owner_user_id, id, now.max(crate::api::now_ms()));
            diesel::insert_into(q::table)
                .values((
                    q::actor_user_id.eq(&params.0),
                    q::operation.eq("worker.binding.membership_leave"),
                    q::object_id.eq(&params.1),
                    q::at_ms.eq(&params.2),
                ))
                .execute(db)
                .await
        }?;
        Ok(true)
    }
    pub async fn departure_desired_trusted(&self, id: &str, generation: i64) -> Result<bool> {
        self.db.with_connection(async |db| {

        Ok(sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE id=$1 AND generation=$2 AND state IN ('left','leaving','revoked')) AS matched").bind::<Text,_>(id).bind::<BigInt,_>(generation).get_result::<Flag>(&mut *db).await?.matched)
    }).await
    }
    /// Verify a previously committed creation intent against current authority.
    /// This consumes no user session and cannot create or resume any binding.
    pub async fn verify_provisioning_trusted(
        &self,
        id: &str,
        generation: i64,
        f: &RoomFacts,
        activate: bool,
        now: i64,
    ) -> Result<Agent> {
        // Commit loss independently: a refused final activation must not undo
        // departure after the remote JOIN has already completed.
        self.retire_membership_loss(id, generation, f, activate, now)
            .await?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                        .await?;
                    let b = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        q::table
                            .select(crate::domain::Binding::as_select())
                            .filter((q::id).eq(&params.0))
                            .get_result::<Binding>(db)
                            .await
                    }
                    .map_err(hidden)?;
                    let a = {
                        use crate::schema::agent_tables::agents as q;
                        let params = (&b.agent_id,);
                        q::table
                            .select(crate::domain::Agent::as_select())
                            .filter((q::id).eq(&params.0))
                            .get_result::<Agent>(db)
                            .await
                    }?;
                    let (space, pp, rp) = Self::binding_policies(db, &b).await?;
                    #[derive(diesel::Queryable, diesel::QueryableByName)]
                    struct Owner {
                        #[diesel(sql_type=Text)]
                        mxid: String,
                        #[diesel(sql_type=Bool)]
                        active: bool,
                    }
                    let owner = {
                        use crate::schema::agent_tables::users as q;
                        let params = (&a.owner_user_id,);
                        q::table
                            .select((q::mxid, q::active))
                            .filter((q::id).eq(&params.0))
                            .get_result::<Owner>(db)
                            .await
                    }?;
                    fresh(f.observed_at_ms, now)?;
                    let enabled = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        q::table
                            .select((diesel::dsl::not(
                                (q::admin_project_paused).or(q::admin_room_paused),
                            ),))
                            .filter((q::id).eq(&params.0))
                            .get_result::<Flag>(db)
                            .await
                    }?;
                    if b.generation != generation
                        || b.state != "joining"
                        || !matches!(a.state.as_str(), "creating" | "active" | "suspended")
                        || !owner.active
                        || !enabled.matched
                    {
                        return Err(Error::Conflict("provisioning_intent_stale"));
                    }
                    if f.owner_mxid != owner.mxid
                        || f.room_id != b.room_id
                        || f.space_id != space
                        || f.puppet_mxid.as_deref() != Some(&a.puppet_mxid)
                        || !f.owner_in_room
                        || (if space.is_empty() {
                            !f.owner_direct_valid
                        } else {
                            !f.owner_in_space || !f.room_in_space
                        })
                        || !pp.permits(&owner.mxid)
                        || !rp.permits(&owner.mxid)
                    {
                        return Err(Error::Unauthorized("provisioning_authority_revoked"));
                    }
                    if f.encrypted && b.scope_kind != "owner_direct" {
                        return Err(Error::Conflict("encrypted_room_requires_client_crypto"));
                    }
                    if !f.puppet_in_room && !f.service_can_invite && !f.owner_direct_valid {
                        return Err(Error::Unauthorized("service_invitation_denied"));
                    }
                    if activate {
                        if !f.puppet_in_room {
                            return Err(Error::Conflict("puppet_join_not_confirmed"));
                        }
                        {
                            use crate::schema::agent_tables::bindings as q;
                            let params = (id,);
                            diesel::update(q::table)
                                .filter((q::id).eq(&params.0))
                                .set((q::state.eq("active"), q::owner_service_paused.eq(false)))
                                .execute(db)
                                .await
                        }?;
                        {
                            use crate::schema::agent_tables::agents as q;
                            let params = (&a.id,);
                            diesel::update(q::table)
                                .filter(((q::id).eq(&params.0)).and((q::state).eq("creating")))
                                .set((q::state.eq("active"),))
                                .execute(db)
                                .await
                        }?;
                        {
                            use crate::schema::agent_tables::domain_audit as q;
                            let params = (&a.owner_user_id, id, now.max(crate::api::now_ms()));
                            diesel::insert_into(q::table)
                                .values((
                                    q::actor_user_id.eq(&params.0),
                                    q::operation.eq("worker.binding.activate"),
                                    q::object_id.eq(&params.1),
                                    q::at_ms.eq(&params.2),
                                ))
                                .execute(db)
                                .await
                        }?;
                    }
                    Ok(a)
                })
                .await
            })
            .await
    }
    /// Trusted worker only; never mapped to a user-provided authorization DTO.
    /// Confirm actual Matrix departure of the exact immutable puppet and binding.
    pub async fn confirm_left_trusted(
        &self,
        id: &str,
        generation: i64,
        f: &RoomFacts,
        now: i64,
    ) -> Result<Binding> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                    db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)")
                        .await?;
                    let b = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        q::table
                            .select(crate::domain::Binding::as_select())
                            .filter((q::id).eq(&params.0))
                            .get_result::<Binding>(db)
                            .await
                    }
                    .map_err(hidden)?;
                    let a = {
                        use crate::schema::agent_tables::agents as q;
                        let params = (&b.agent_id,);
                        q::table
                            .select(crate::domain::Agent::as_select())
                            .filter((q::id).eq(&params.0))
                            .get_result::<Agent>(db)
                            .await
                    }?;
                    let space = match &b.project_id {
                        Some(project) => Self::project_db(db, project).await?.space_id,
                        None => String::new(),
                    };
                    #[derive(diesel::Queryable, diesel::QueryableByName)]
                    struct Owner {
                        #[diesel(sql_type=Text)]
                        mxid: String,
                    }
                    let owner = {
                        use crate::schema::agent_tables::users as q;
                        let params = (&a.owner_user_id,);
                        q::table
                            .select((q::mxid,))
                            .filter((q::id).eq(&params.0))
                            .get_result::<Owner>(db)
                            .await
                    }?;
                    fresh(f.observed_at_ms, now)?;
                    if b.generation != generation
                        || f.room_id != b.room_id
                        || f.space_id != space
                        || f.owner_mxid != owner.mxid
                        || f.puppet_mxid.as_deref() != Some(&a.puppet_mxid)
                        || f.puppet_in_room
                    {
                        return Err(Error::Conflict("puppet_departure_not_confirmed"));
                    }
                    if b.state == "left" {
                        return Ok(b);
                    }
                    if !matches!(b.state.as_str(), "leaving" | "revoked") {
                        return Err(Error::Conflict("binding_not_leaving"));
                    }
                    let result = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.0))
                            .set((q::state.eq("left"), q::owner_service_paused.eq(false)))
                            .returning(crate::domain::Binding::as_select())
                            .get_result(db)
                            .await
                    }?;
                    {
                        use crate::schema::agent_tables::domain_audit as q;
                        let params = (&a.owner_user_id, id, now.max(crate::api::now_ms()));
                        diesel::insert_into(q::table)
                            .values((
                                q::actor_user_id.eq(&params.0),
                                q::operation.eq("worker.binding.left"),
                                q::object_id.eq(&params.1),
                                q::at_ms.eq(&params.2),
                            ))
                            .execute(db)
                            .await
                    }?;
                    Ok(result)
                })
                .await
            })
            .await
    }
    /// Only finalize previously committed retirement after every departure was
    /// verified. Permanent owner and puppet records remain intact forever.
    pub async fn confirm_retired_trusted(
        &self,
        id: &str,
        generation: i64,
        now: i64,
    ) -> Result<Agent> {
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            db.batch_execute("SELECT pg_advisory_xact_lock(5210750088328904)").await?;
            let a={
                use crate::schema::agent_tables::agents as q;
                let params = (id,);
                q::table
                    .select(crate::domain::Agent::as_select())
                    .filter((q::id).eq(&params.0))
                    .get_result::<Agent>(db)
                    .await
            }.map_err(hidden)?;
            if a.generation!=generation {return Err(Error::Conflict("stale_agent_generation"));}
            if a.state=="retired" {return Ok(a);}
            if a.state!="retiring" {return Err(Error::Conflict("agent_not_retiring"));}
            let departed=sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE agent_id=$1 AND state<>'left') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?;
            if !departed.matched {return Err(Error::Conflict("agent_cleanup_incomplete"));}
            let result={
                use crate::schema::agent_tables::agents as q;
                let params = (id,);
                diesel::update(q::table)
                    .filter((q::id).eq(&params.0))
                    .set((q::state.eq("retired"),))
                    .returning(crate::domain::Agent::as_select())
                    .get_result(db)
                    .await
            }?;
            {
                use crate::schema::agent_tables::domain_audit as q;
                let params = (&a.owner_user_id, id, now.max(crate::api::now_ms()));
                diesel::insert_into(q::table)
                    .values((
                        q::actor_user_id.eq(&params.0),
                        q::operation.eq("worker.agent.retired"),
                        q::object_id.eq(&params.1),
                        q::at_ms.eq(&params.2),
                    ))
                    .execute(db)
                    .await
            }?;Ok(result)
        }).await
    }).await
    }
    /// Called after trusted gateway has provisioned the exact puppet and confirmed join.
    /// expected_generation fences stale lifecycle jobs after pause, leave, or retirement.
    pub async fn activate_binding(
        &self,
        p: &Principal,
        id: &str,
        expected_generation: i64,
        f: &RoomFacts,
        now: i64,
    ) -> Result<Binding> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let b = Self::binding_db(db, p, id).await?;
                    let a = Self::agent_db(db, p, &b.agent_id).await?;

                    if b.generation != expected_generation {
                        return Err(Error::Conflict("stale_binding_generation"));
                    }

                    if b.state == "active" {
                        return Ok(b);
                    }

                    let pause = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        q::table
                            .select((diesel::dsl::not(
                                (q::admin_project_paused).or(q::admin_room_paused),
                            ),))
                            .filter((q::id).eq(&params.0))
                            .get_result::<Flag>(db)
                            .await
                    }?;
                    if !pause.matched {
                        return Err(Error::Unauthorized("administrator_pause_active"));
                    }

                    if b.state != "joining"
                        || !matches!(a.state.as_str(), "creating" | "active" | "suspended")
                    {
                        return Err(Error::Conflict("binding_not_joining"));
                    }

                    let (space, pp, rp) = Self::binding_policies(db, &b).await?;
                    creating(
                        p,
                        f,
                        &b.room_id,
                        &space,
                        &pp,
                        &rp,
                        Some(&a.puppet_mxid),
                        now,
                    )?;

                    if !f.puppet_in_room || f.puppet_mxid.as_deref() != Some(&a.puppet_mxid) {
                        return Err(Error::Conflict("puppet_join_not_confirmed"));
                    }

                    let b = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.0))
                            .set((q::state.eq("active"), q::owner_service_paused.eq(false)))
                            .returning(crate::domain::Binding::as_select())
                            .get_result(db)
                            .await
                    }?;

                    {
                        use crate::schema::agent_tables::agents as q;
                        let params = (&a.id,);
                        diesel::update(q::table)
                            .filter(((q::id).eq(&params.0)).and((q::state).eq("creating")))
                            .set((q::state.eq("active"),))
                            .execute(db)
                            .await
                    }?;
                    Self::audit(db, p, "binding.activate", id, now).await?;
                    Ok(b)
                })
                .await
            })
            .await
    }

    pub async fn runnable(
        &self,
        p: &Principal,
        id: &str,
        f: &RoomFacts,
        now: i64,
    ) -> Result<AgentBinding> {
        self.observe_owner_membership(p, id, f, now).await?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;

                    let b = Self::binding_db(db, p, id).await?;

                    let a = Self::agent_db(db, p, &b.agent_id).await?;

                    if b.state != "active" || a.state != "active" {
                        return Err(Error::Unauthorized("agent_not_running"));
                    }

                    let (space, _, _) = Self::binding_policies(db, &b).await?;

                    membership(p, f, &b.room_id, &space, now)?;

                    if !f.puppet_in_room || f.puppet_mxid.as_deref() != Some(&a.puppet_mxid) {
                        return Err(Error::Unauthorized("puppet_not_joined"));
                    }

                    if f.encrypted && b.scope_kind != "owner_direct" {
                        return Err(Error::Conflict("encrypted_room_requires_client_crypto"));
                    }

                    Ok(AgentBinding {
                        agent: a,
                        binding: b,
                    })
                })
                .await
            })
            .await
    }

    /// Host gateway lookup. HTTP handlers must verify current Space membership
    /// before exposing project metadata or policy exception lists.
    pub async fn project(&self, p: &Principal, id: &str, now: i64) -> Result<Project> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;

                    Self::project_db(db, id).await
                })
                .await
            })
            .await
    }

    /// Host gateway lookup; current room membership is checked before HTTP exposure.
    pub async fn room(
        &self,
        p: &Principal,
        project_id: &str,
        room_id: &str,
        now: i64,
    ) -> Result<Room> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;

                    Self::room_db(db, room_id, project_id).await
                })
                .await
            })
            .await
    }

    /// Host gateway discovery input. Filter this list with fresh Matrix membership
    /// before returning it to clients; authentication alone is not discoverability.
    pub async fn projects(&self, p: &Principal, now: i64) -> Result<Vec<Project>> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    Ok({
                        use crate::schema::agent_tables::projects as q;
                        q::table
                            .select(crate::domain::Project::as_select())
                            .filter(q::active)
                            .order((q::id.asc(),))
                            .load(db)
                            .await
                    }?)
                })
                .await
            })
            .await
    }

    pub async fn agent(&self, p: &Principal, id: &str, now: i64) -> Result<Agent> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;

                    Self::agent_db(db, p, id).await
                })
                .await
            })
            .await
    }

    pub async fn binding(&self, p: &Principal, id: &str, now: i64) -> Result<Binding> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;

                    Self::binding_db(db, p, id).await
                })
                .await
            })
            .await
    }

    /// Only the Agent owner can opt a project Room into automatic thread replies.
    pub async fn set_thread_auto_reply(
        &self,
        p: &Principal,
        id: &str,
        enabled: bool,
        now: i64,
    ) -> Result<Binding> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let binding = Self::binding_db(db, p, id).await?;
                    if binding.scope_kind != "project" {
                        return Err(Error::Invalid("project_binding_required"));
                    }
                    if !matches!(binding.state.as_str(), "active" | "suspended") {
                        return Err(Error::Conflict("binding_not_active"));
                    }
                    Self::audit(db, p, "binding.reply_policy", id, now).await?;
                    Ok({
                        use crate::schema::agent_tables::bindings as q;
                        let params = (enabled, id);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.1))
                            .set((q::thread_auto_reply.eq(&params.0),))
                            .returning(crate::domain::Binding::as_select())
                            .get_result(db)
                            .await
                    }?)
                })
                .await
            })
            .await
    }

    pub async fn agents(&self, p: &Principal, now: i64) -> Result<Vec<Agent>> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    Ok({
                        use crate::schema::agent_tables::agents as q;
                        let params = (&p.user_id,);
                        q::table
                            .select(crate::domain::Agent::as_select())
                            .filter((q::owner_user_id).eq(&params.0))
                            .order((q::id.asc(),))
                            .load(db)
                            .await
                    }?)
                })
                .await
            })
            .await
    }

    /// Demotion requires leaving every shared Room first; no hidden live services.
    pub async fn set_visibility(
        &self,
        p: &Principal,
        id: &str,
        visibility: AgentVisibility,
        now: i64,
    ) -> Result<Agent> {
        key(id)?;
        self.db.with_connection(async |db| {

        db.transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
            Self::authorize(db, p, now).await?;
            {
                use crate::schema::agent_tables::agents as q;
                let params = (id, &p.user_id);
                q::table
                    .select((true.into_sql::<diesel::sql_types::Bool>(),))
                    .filter(((q::id).eq(&params.0)).and((q::owner_user_id).eq(&params.1)))
                    .for_update()
                    .get_result::<Flag>(db)
                    .await
            }.map_err(hidden)?;
            let agent = Self::agent_db(db, p, id).await?;
            if matches!(agent.state.as_str(), "retiring" | "retired") { return Err(Error::Conflict("agent_retired")); }
            if visibility == AgentVisibility::Private {
                let bound = sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE agent_id=$1 AND scope_kind='project' AND state<>'left') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?;
                if bound.matched { return Err(Error::Conflict("leave_shared_rooms_before_private")); }
            }
            {
                use crate::schema::agent_tables::agents as q;
                let params = (visibility.as_str(), id);
                diesel::update(q::table)
                    .filter((q::id).eq(&params.1))
                    .set((q::visibility.eq(&params.0),))
                    .execute(db)
                    .await
            }?;
            Self::audit(db, p, "agent.visibility", id, now).await?;
            Self::agent_db(db, p, id).await
        }).await
    }).await
    }

    pub async fn bindings(&self, p: &Principal, agent_id: &str, now: i64) -> Result<Vec<Binding>> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    Self::agent_db(db, p, agent_id).await?;
                    Ok({
                        use crate::schema::agent_tables::bindings as q;
                        let params = (agent_id,);
                        q::table
                            .select(crate::domain::Binding::as_select())
                            .filter((q::agent_id).eq(&params.0))
                            .order((q::id.asc(),))
                            .load(db)
                            .await
                    }?)
                })
                .await
            })
            .await
    }

    /// Global owner pause invalidates generations in every binding. Administrators
    /// manage only Project/Room pauses and cannot take over an Agent identity.
    pub async fn suspend_agent(&self, p: &Principal, id: &str, now: i64) -> Result<Agent> {
        self.agent_transition(p, id, "suspended", now).await
    }

    pub async fn resume_agent(&self, p: &Principal, id: &str, now: i64) -> Result<Agent> {
        self.agent_transition(p, id, "active", now).await
    }

    async fn agent_transition(
        &self,
        p: &Principal,
        id: &str,
        target: &str,
        now: i64,
    ) -> Result<Agent> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let a = Self::agent_db(db, p, id).await?;

                    if a.state == target {
                        return Ok(a);
                    }
                    if !matches!(a.state.as_str(), "active" | "suspended") {
                        return Err(Error::Conflict("agent_transition_denied"));
                    }

                    let a = {
                        use crate::schema::agent_tables::agents as q;
                        let params = (target, id);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.1))
                            .set((q::state.eq(&params.0), q::generation.eq(q::generation + 1)))
                            .returning(crate::domain::Agent::as_select())
                            .get_result(db)
                            .await
                    }?;

                    {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        diesel::update(q::table)
                            .filter(((q::agent_id).eq(&params.0)).and(q::state.eq_any(vec![
                                "joining",
                                "active",
                                "suspended",
                            ])))
                            .set((q::generation.eq(q::generation + 1),))
                            .execute(db)
                            .await
                    }?;

                    Self::audit(
                        db,
                        p,
                        if target == "active" {
                            "agent.resume"
                        } else {
                            "agent.suspend"
                        },
                        id,
                        now,
                    )
                    .await?;
                    Ok(a)
                })
                .await
            })
            .await
    }

    pub async fn suspend_binding(&self, p: &Principal, id: &str, now: i64) -> Result<Binding> {
        self.owner_binding_transition(p, id, "suspended", now).await
    }

    pub async fn leave_binding(&self, p: &Principal, id: &str, now: i64) -> Result<Binding> {
        self.owner_binding_transition(p, id, "leaving", now).await
    }

    async fn owner_binding_transition(
        &self,
        p: &Principal,
        id: &str,
        target: &str,
        now: i64,
    ) -> Result<Binding> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let b = Self::binding_db(db, p, id).await?;

                    if target == "suspended" && b.state == "suspended" {
                        {
                            use crate::schema::agent_tables::bindings as q;
                            let params = (id,);
                            diesel::update(q::table)
                                .filter((q::id).eq(&params.0))
                                .set((q::owner_service_paused.eq(true),))
                                .execute(db)
                                .await
                        }?;
                        return Self::binding_db(db, p, id).await;
                    }
                    if b.state == target
                        || (target == "leaving" && matches!(b.state.as_str(), "left" | "revoked"))
                    {
                        return Ok(b);
                    }

                    if !matches!(b.state.as_str(), "joining" | "active" | "suspended") {
                        return Err(Error::Conflict("binding_transition_denied"));
                    }

                    let updated = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (target, id);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.1))
                            .set((
                                q::state.eq(&params.0),
                                q::owner_service_paused.eq(params.0 == "suspended"),
                                q::generation.eq(q::generation + 1),
                            ))
                            .returning(crate::domain::Binding::as_select())
                            .get_result(db)
                            .await
                    }?;
                    Self::audit(
                        db,
                        p,
                        if target == "suspended" {
                            "binding.suspend"
                        } else {
                            "binding.leave"
                        },
                        id,
                        now,
                    )
                    .await?;
                    Ok(updated)
                })
                .await
            })
            .await
    }

    /// Resume an existing paused binding rechecks memberships; creation-only bans do
    /// not implicitly cancel an existing authorization. Explicit leave/rebind is creation.
    pub async fn resume_binding(
        &self,
        p: &Principal,
        id: &str,
        f: &RoomFacts,
        now: i64,
    ) -> Result<Binding> {
        self.observe_owner_membership(p, id, f, now).await?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let b = Self::binding_db(db, p, id).await?;
                    let a = Self::agent_db(db, p, &b.agent_id).await?;
                    if b.state == "active" {
                        return Ok(b);
                    }
                    if b.state != "suspended" || a.state != "active" {
                        return Err(Error::Conflict("binding_not_resumable"));
                    }

                    let pause = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        q::table
                            .select((diesel::dsl::not(
                                (q::admin_project_paused).or(q::admin_room_paused),
                            ),))
                            .filter((q::id).eq(&params.0))
                            .get_result::<Flag>(db)
                            .await
                    }?;
                    if !pause.matched {
                        return Err(Error::Unauthorized("administrator_pause_active"));
                    }

                    let (space, _, _) = Self::binding_policies(db, &b).await?;
                    membership(p, f, &b.room_id, &space, now)?;

                    if !f.puppet_in_room || f.puppet_mxid.as_deref() != Some(&a.puppet_mxid) {
                        return Err(Error::Conflict("puppet_join_not_confirmed"));
                    }
                    if f.encrypted && b.scope_kind != "owner_direct" {
                        return Err(Error::Conflict("encrypted_room_requires_client_crypto"));
                    }

                    let result = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.0))
                            .set((
                                q::state.eq("active"),
                                q::owner_service_paused.eq(false),
                                q::generation.eq(q::generation + 1),
                            ))
                            .returning(crate::domain::Binding::as_select())
                            .get_result(db)
                            .await
                    }?;
                    Self::audit(db, p, "binding.resume", id, now).await?;
                    Ok(result)
                })
                .await
            })
            .await
    }

    pub async fn service_state(
        &self,
        p: &Principal,
        project_id: &str,
        room_id: Option<&str>,
        now: i64,
    ) -> Result<serde_json::Value> {
        self.db.with_connection(async |db| {

        db.transaction::<_,Error,_>(async |db:&mut AsyncPgConnection| {
            Self::authorize(db,p,now).await?;
            let project=Self::project_db(db,project_id).await?;
            let mut revision=project.revision;
            let mut creation_policy=project.creation_policy;
            if let Some(room)=room_id {let room=Self::room_db(db,room,project_id).await?;revision=room.revision;creation_policy=room.creation_policy;}
            let project_paused=sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.scope_pauses WHERE kind='project' AND scope_id=$1 AND paused) AS matched").bind::<Text,_>(project_id).get_result::<Flag>(db).await?.matched;
            let room_paused=match room_id {Some(room)=>sql_query("SELECT EXISTS(SELECT 1 FROM hagency_agent_v1.scope_pauses WHERE kind='room' AND scope_id=$1 AND paused) AS matched").bind::<Text,_>(room).get_result::<Flag>(db).await?.matched,None=>false};
            Ok(serde_json::json!({"projectId":project_id,"roomId":room_id,"projectPaused":project_paused,"roomPaused":room_paused,"servicePaused":project_paused||room_paused,"revision":revision,"creationPolicy":creation_policy}))
        }).await
    }).await
    }
    pub async fn suspend_project_bindings(
        &self,
        p: &Principal,
        project_id: &str,
        f: &AdminFacts,
        now: i64,
    ) -> Result<usize> {
        self.db.with_connection(async |db| {


        db.transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection|{

            Self::authorize(db, p, now).await?;
            let project = Self::project_db(db, project_id).await?;
            administer(p, f, &project.space_id, now)?;

            sql_query("INSERT INTO hagency_agent_v1.scope_pauses(kind,scope_id,paused) VALUES('project',$1,true) ON CONFLICT(kind,scope_id) DO UPDATE SET paused=excluded.paused").bind::<Text,_>(project_id).execute(db).await?;
            let count={
                use crate::schema::agent_tables::bindings as q;
                let params = (project_id,);
                diesel::update(q::table).filter(((q::project_id).eq(&params.0)).and((q::state.eq_any(vec!["active", "joining"])).or(((q::state).eq("suspended")).and(diesel::dsl::not(q::admin_project_paused))))).set((q::state.eq("suspended"), q::admin_project_paused.eq(true), q::generation.eq(q::generation + 1),)).execute(db).await
            }?;
            Self::audit(db, p, "project.suspend_bindings", project_id, now).await?;
            Ok(count)
        }
).await
    }).await
    }

    pub async fn suspend_room_bindings(
        &self,
        p: &Principal,
        project_id: &str,
        room_id: &str,
        f: &AdminFacts,
        now: i64,
    ) -> Result<usize> {
        self.db.with_connection(async |db| {


        db.transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection|{

            Self::authorize(db, p, now).await?;
            Self::room_db(db, room_id, project_id).await?;
            administer(p, f, room_id, now)?;

            sql_query("INSERT INTO hagency_agent_v1.scope_pauses(kind,scope_id,paused) VALUES('room',$1,true) ON CONFLICT(kind,scope_id) DO UPDATE SET paused=excluded.paused").bind::<Text,_>(room_id).execute(db).await?;
            let count={
                use crate::schema::agent_tables::bindings as q;
                let params = (project_id, room_id);
                diesel::update(q::table)
                    .filter(
                        (((q::project_id).eq(&params.0)).and((q::room_id).eq(&params.1))).and(
                            (q::state.eq_any(vec!["active", "joining"]))
                                .or(((q::state).eq("suspended")).and(diesel::dsl::not(q::admin_room_paused))),
                        ),
                    )
                    .set((
                        q::state.eq("suspended"),
                        q::admin_room_paused.eq(true),
                        q::generation.eq(q::generation + 1),
                    ))
                    .execute(db)
                    .await
            }?;
            Self::audit(db, p, "room.suspend_bindings", room_id, now).await?;
            Ok(count)
        }
).await
    }).await
    }

    /// Administrator releases their pause without implicitly restarting local execution.
    pub async fn clear_project_pause(
        &self,
        p: &Principal,
        project_id: &str,
        f: &AdminFacts,
        now: i64,
    ) -> Result<usize> {
        self.db.with_connection(async |db| {


        db.transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection|{

            Self::authorize(db, p, now).await?;
            let project = Self::project_db(db, project_id).await?;
            administer(p, f, &project.space_id, now)?;

            sql_query("INSERT INTO hagency_agent_v1.scope_pauses(kind,scope_id,paused) VALUES('project',$1,false) ON CONFLICT(kind,scope_id) DO UPDATE SET paused=excluded.paused").bind::<Text,_>(project_id).execute(db).await?;
            let count={
                use crate::schema::agent_tables::bindings as q;
                let params = (project_id,);
                diesel::update(q::table)
                    .filter(((q::project_id).eq(&params.0)).and(q::admin_project_paused))
                    .set((q::admin_project_paused.eq(false),))
                    .execute(db)
                    .await
            }?;
            Self::audit(db, p, "project.clear_pause", project_id, now).await?;
            Ok(count)
        }
).await
    }).await
    }

    pub async fn clear_room_pause(
        &self,
        p: &Principal,
        project_id: &str,
        room_id: &str,
        f: &AdminFacts,
        now: i64,
    ) -> Result<usize> {
        self.db.with_connection(async |db| {


        db.transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection|{

            Self::authorize(db, p, now).await?;
            Self::room_db(db, room_id, project_id).await?;
            administer(p, f, room_id, now)?;

            sql_query("INSERT INTO hagency_agent_v1.scope_pauses(kind,scope_id,paused) VALUES('room',$1,false) ON CONFLICT(kind,scope_id) DO UPDATE SET paused=excluded.paused").bind::<Text,_>(room_id).execute(db).await?;
            let count={
                use crate::schema::agent_tables::bindings as q;
                let params = (project_id, room_id);
                diesel::update(q::table)
                    .filter(
                        (((q::project_id).eq(&params.0)).and((q::room_id).eq(&params.1)))
                            .and(q::admin_room_paused),
                    )
                    .set((q::admin_room_paused.eq(false),))
                    .execute(db)
                    .await
            }?;
            Self::audit(db, p, "room.clear_pause", room_id, now).await?;
            Ok(count)
        }
).await
    }).await
    }

    pub async fn retire_agent(&self, p: &Principal, id: &str, now: i64) -> Result<Agent> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let a = Self::agent_db(db, p, id).await?;
                    if matches!(a.state.as_str(), "retiring" | "retired") {
                        return Ok(a);
                    }

                    let a = {
                        use crate::schema::agent_tables::agents as q;
                        let params = (id,);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.0))
                            .set((q::state.eq("retiring"), q::generation.eq(q::generation + 1)))
                            .returning(crate::domain::Agent::as_select())
                            .get_result(db)
                            .await
                    }?;

                    {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        diesel::update(q::table)
                            .filter(
                                ((q::agent_id).eq(&params.0)).and(diesel::dsl::not(
                                    q::state.eq_any(vec!["left", "revoked"]),
                                )),
                            )
                            .set((
                                q::state.eq("revoked"),
                                q::owner_service_paused.eq(false),
                                q::generation.eq(q::generation + 1),
                            ))
                            .execute(db)
                            .await
                    }?;
                    Self::audit(db, p, "agent.retire", id, now).await?;
                    Ok(a)
                })
                .await
            })
            .await
    }

    /// Trusted lifecycle worker uses observed departure of the exact puppet; no
    /// public API may turn an unverified leave into completed cleanup.
    pub async fn confirm_left(
        &self,
        p: &Principal,
        id: &str,
        generation: i64,
        f: &RoomFacts,
        now: i64,
    ) -> Result<Binding> {
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async move |db: &mut AsyncPgConnection| {
                    Self::authorize(db, p, now).await?;
                    let b = Self::binding_db(db, p, id).await?;
                    let a = Self::agent_db(db, p, &b.agent_id).await?;
                    fresh(f.observed_at_ms, now)?;

                    if b.generation != generation
                        || f.room_id != b.room_id
                        || f.puppet_mxid.as_deref() != Some(&a.puppet_mxid)
                        || f.puppet_in_room
                    {
                        return Err(Error::Conflict("puppet_departure_not_confirmed"));
                    }

                    if b.state == "left" {
                        return Ok(b);
                    }
                    if !matches!(b.state.as_str(), "leaving" | "revoked") {
                        return Err(Error::Conflict("binding_not_leaving"));
                    }

                    let b = {
                        use crate::schema::agent_tables::bindings as q;
                        let params = (id,);
                        diesel::update(q::table)
                            .filter((q::id).eq(&params.0))
                            .set((q::state.eq("left"), q::owner_service_paused.eq(false)))
                            .returning(crate::domain::Binding::as_select())
                            .get_result(db)
                            .await
                    }?;
                    Self::audit(db, p, "binding.left", id, now).await?;
                    Ok(b)
                })
                .await
            })
            .await
    }

    pub async fn confirm_retired(
        &self,
        p: &Principal,
        id: &str,
        generation: i64,
        now: i64,
    ) -> Result<Agent> {
        self.db.with_connection(async |db| {


        db.transaction::<_,Error,_>(async move |db:&mut AsyncPgConnection|{

            Self::authorize(db, p, now).await?;
            let a = Self::agent_db(db, p, id).await?;
            if a.generation != generation {
                return Err(Error::Conflict("stale_agent_generation"));
            }
            if a.state == "retired" {
                return Ok(a);
            }
            if a.state != "retiring" {
                return Err(Error::Conflict("agent_not_retiring"));
            }

            let departed=sql_query("SELECT NOT EXISTS(SELECT 1 FROM hagency_agent_v1.bindings WHERE agent_id=$1 AND state<>'left') AS matched").bind::<Text,_>(id).get_result::<Flag>(db).await?;
            if !departed.matched {
                return Err(Error::Conflict("agent_cleanup_incomplete"));
            }

            let a={
                use crate::schema::agent_tables::agents as q;
                let params = (id,);
                diesel::update(q::table)
                    .filter((q::id).eq(&params.0))
                    .set((q::state.eq("retired"),))
                    .returning(crate::domain::Agent::as_select())
                    .get_result(db)
                    .await
            }?;
            Self::audit(db, p, "agent.retired", id, now).await?;
            Ok(a)
        }
).await
    }).await
    }
}

fn hidden(e: diesel::result::Error) -> Error {
    if e == diesel::result::Error::NotFound {
        Error::Unauthorized("domain_object_not_authorized")
    } else {
        e.into()
    }
}

fn revision(e: diesel::result::Error) -> Error {
    if e == diesel::result::Error::NotFound {
        Error::Conflict("stale_policy_revision")
    } else {
        e.into()
    }
}

use diesel::OptionalExtension;

#[cfg(test)]
#[path = "domain_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::CreateBoundAgent;

impl DomainStore {
    pub(crate) async fn archive_project(
        &self,
        p: &Principal,
        id: &str,
        expected: i64,
        archived: bool,
        facts: &AdminFacts,
        now: i64,
    ) -> Result<Project> {
        key(id)?;
        self.db
            .with_connection(async |db| {
                db.transaction::<_, Error, _>(async |db| {
                    Self::authorize(db, p, crate::api::now_ms()).await?;
                    let project = {
                        use crate::schema::agent_tables::projects as q;
                        let params = (id,);
                        q::table
                            .select(crate::domain::Project::as_select())
                            .filter((q::id).eq(&params.0))
                            .for_update()
                            .get_result::<Project>(db)
                            .await
                    }
                    .map_err(hidden)?;
                    administer(p, facts, &project.space_id, now)?;
                    if !facts.is_space {
                        return Err(Error::Unauthorized("project_administration_required"));
                    }
                    // Live membership/permission observations expire while waiting for storage too.
                    administer(p, facts, &project.space_id, crate::api::now_ms())?;
                    {
                        use crate::schema::agent_tables::projects as q;
                        let params = (archived, id, expected);
                        diesel::update(q::table)
                            .filter(((q::id).eq(&params.1)).and((q::revision).eq(&params.2)))
                            .set((q::archived.eq(&params.0), q::revision.eq(q::revision + 1)))
                            .returning(crate::domain::Project::as_select())
                            .get_result(db)
                            .await
                    }
                    .map_err(revision)
                })
                .await
            })
            .await
    }
}
