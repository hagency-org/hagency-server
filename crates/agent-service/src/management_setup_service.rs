//! The existing typed confirmation owns service membership too. Joining the
//! configured service is idempotent; it never creates rooms or agent bindings.
use super::*;
use crate::matrix_client::MatrixClient;

impl DomainStore {
    async fn setup_service_authority(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: &ConfirmProjectSetup,
    ) -> Result<Option<ProjectSetup>> {
        let digest = digest(id, input)?;
        let mut db = self.db.lock().await?;
        (*db)
            .transaction::<_, Error, _>(async |db: &mut AsyncPgConnection| {
                Self::authorize(db, p, crate::api::now_ms()).await?;
                let s = setup_id(db, id).await?;
                let plan = plan(db, &s.project_id).await?;
                if s.manager_mxid != p.mxid || plan.manager_user_id != p.user_id {
                    return Err(Error::Unauthorized("project_manager_required"));
                }
                if replay::<ProjectReadiness>(
                    db,
                    p,
                    &input.command_id,
                    "project.setup.confirm",
                    &digest,
                )
                .await?
                .is_some()
                {
                    return Ok(None);
                }
                if s.revision != input.expected_revision {
                    return Err(Error::Conflict("management_revision_changed"));
                }
                if s.state == "cancelled" {
                    return Err(Error::Conflict("project_setup_cancelled"));
                }
                if s.space_id.as_ref().is_some_and(|v| v != &input.space_id)
                    || s.room_id.as_ref().is_some_and(|v| v != &input.room_id)
                {
                    return Err(Error::Conflict("project_matrix_link_immutable"));
                }
                let b = binding(&s)?;
                let a = authority(db, &s.project_id)
                    .await?
                    .filter(|v| v.active)
                    .ok_or(Error::Conflict("project_administrator_required"))?;
                if a.revision != b.authority_revision || a.mxid != b.administrator_mxid {
                    return Err(Error::Conflict("project_administrator_changed"));
                }
                let now = clock(db).await?;
                let capacity =
                    allocations::setup_capacity(db, &plan.id, Some(&b.allocation_id), now)
                        .await?
                        .filter(|_| policy.project_reservations())
                        .ok_or(Error::Conflict("live_reservation_required"))?;
                if capacity.receipt_digest != b.allocation_receipt_digest
                    || plan.revision != b.project_revision
                {
                    return Err(Error::Conflict("project_setup_changed"));
                }
                Ok(Some(s))
            })
            .await
    }

    pub(crate) async fn accept_management_setup_service(
        &self,
        p: &Principal,
        policy: &ManagementPolicy,
        id: &str,
        input: &ConfirmProjectSetup,
        gateway: &Gateway,
        matrix: Option<&MatrixClient>,
    ) -> Result<bool> {
        enabled(policy)?;
        key(id)?;
        key(&input.command_id)?;
        matrix_id(&input.space_id, '!')?;
        matrix_id(&input.room_id, '!')?;
        if input.expected_revision <= 0 || input.space_id == input.room_id {
            return Err(Error::Invalid("invalid_project_setup"));
        }
        let mut joined_any = false;
        let result = async {
            for target in [&input.space_id, &input.room_id] {
                let Some(s) = self.setup_service_authority(p, policy, id, input).await? else {
                    return Ok(());
                };
                // One actual state snapshot per room. Verify manager control
                // before inspecting details; the caller cannot authorize joins
                // by supplying unrelated room IDs or purported Matrix facts.
                let started = crate::api::now_ms();
                let (sf, rf, facts, ss, rs) = gateway
                    .project_setup(&input.space_id, &input.room_id, &p.mxid)
                    .await?;
                let now = crate::api::now_ms();
                administer(p, &sf, &input.space_id, now)?;
                administer(p, &rf, &input.room_id, now)?;
                membership(p, &facts, &input.room_id, &input.space_id, now)?;
                if !sf.is_space
                    || rf.is_space
                    || rf.linked_space_id.as_deref() != Some(input.space_id.as_str())
                    || facts.encrypted
                {
                    return Err(Error::Conflict("project_matrix_not_ready"));
                }
                for events in [&ss, &rs] {
                    joined(events, &binding(&s)?.administrator_mxid)?;
                    if state(events, "m.room.join_rules", "")?["content"]["join_rule"] != "invite"
                        || events.iter().any(|e| e["type"] == "m.room.encryption")
                    {
                        return Err(Error::Conflict("project_private_room_required"));
                    }
                }
                if state(&rs, "m.space.parent", &input.space_id)?["content"]["via"]
                    .as_array()
                    .is_none_or(|v| v.is_empty())
                {
                    return Err(Error::Conflict("project_matrix_proof_missing"));
                }
                if s.mode == "create" {
                    marker(&ss, &s, "space", &input.space_id)?;
                    marker(&rs, &s, "room", &input.space_id)?;
                }
                let events = if target == &input.space_id { &ss } else { &rs };
                let membership = &state(events, "m.room.member", gateway.service_mxid())?["content"]
                    ["membership"];
                if membership != "invite" && membership != "join" {
                    return Err(Error::Conflict("project_service_invitation_required"));
                }
                // A revoked grant, replaced admin, cancelled setup or retired
                // device during the state read prevents the next Matrix effect.
                let Some(current) = self.setup_service_authority(p, policy, id, input).await?
                else {
                    return Ok(());
                };
                if current.definition_digest != s.definition_digest {
                    return Err(Error::Conflict("project_setup_changed"));
                }
                fresh(started, crate::api::now_ms())?;
                if membership == "invite" {
                    let matrix =
                        matrix.ok_or(Error::Unavailable("project_setup_service_unavailable"))?;
                    if matrix.service_mxid() != gateway.service_mxid() {
                        return Err(Error::Unavailable("project_setup_service_unavailable"));
                    }
                    // Set before awaiting: lost join responses retain the same
                    // pending confirmation. Retry reobserves membership and
                    // cannot create a replacement room or puppet.
                    joined_any = true;
                    matrix.join_invited_service(target).await?;
                    self.setup_service_authority(p, policy, id, input).await?;
                }
            }
            Ok(())
        }
        .await;
        result.map_err(|e| {
            if joined_any {
                Error::Unavailable("project_setup_membership_outcome_unknown")
            } else {
                e
            }
        })?;
        Ok(joined_any)
    }
}
