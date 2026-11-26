//! Every 503 the service answers is one of two things, and a computer that
//! hosts Agents has to tell them apart (rule 4 of
//! `docs/design/hagency-time-limits.md`). Either the service could not serve
//! the request now, and the computer asks again; or the operator's policy
//! does not serve the route, and the computer takes that as a decision and
//! ends the work it was for. A status does not say which, so Hagency Desktop
//! keeps the list of policy codes, and this file keeps the same list.
//!
//! A new code fails the test below until it is put on one of the two lists.
//!
//! - `POLICY` is only for a code that is answered because a switch is off.
//!   Add it to the computer's list in the same change, or the computer asks
//!   a route that is off again until its lease ends.
//! - Everything else is `COULD_NOT_SERVE`: a failure that can pass by itself.
//!
//! The worse mistake is a passing failure on `POLICY`: a computer ends an
//! Agent's work over it, and nothing gives the message to the Agent again.
use std::{collections::BTreeSet, fs, path::Path};

/// The operator's policy does not serve the route. A computer reads these as
/// decisions.
const POLICY: &[&str] = &[
    "agent_admission_unavailable",
    "agent_budget_issuance_unavailable",
    "agent_executor_unavailable",
    "agent_matrix_provisioning_unavailable",
    "agent_runtime_provisioning_unavailable",
    "calendar_grants_unavailable",
    "management_not_configured",
    "parent_settlement_unavailable",
    "pool_agents_unavailable",
    "pool_grant_succession_unavailable",
    "pool_publication_first_unavailable",
    "project_reservations_unavailable",
    "project_setup_unavailable",
    "shared_budget_execution_issuance_unavailable",
    "shared_demand_issuance_unavailable",
    "shared_demand_retry_unavailable",
    "shared_durable_intake_unavailable",
    "shared_encryption_unavailable",
    "shared_execution_issuance_unavailable",
    "shared_execution_unavailable",
    "shared_successor_issuance_unavailable",
    "shared_thread_context_issuance_unavailable",
];

/// The service could not serve the request now. A computer asks again.
const COULD_NOT_SERVE: &[&str] = &[
    "agent_budget_history_capacity",
    "agent_budget_ticket_capacity",
    "agent_crypto_busy",
    "agent_crypto_identity_mismatch",
    "agent_crypto_in_progress",
    "agent_crypto_outcome_unknown",
    "agent_crypto_response_too_large",
    "agent_executor_session_expired",
    "agent_settlement_capacity",
    "agent_settlement_ticket_capacity",
    "allocation_projection_invalid",
    "allocation_projection_too_large",
    "approval_queue_full",
    "appservice_queue_full",
    "appservice_roundtrip_unconfirmed",
    "appservice_startup_not_ready",
    "board_membership_expired",
    "board_migration_connect_timeout",
    "board_storage_unavailable",
    "cleanup_scopes_unavailable",
    "configuration_storage_invalid",
    "database_migration_task_failed",
    "database_reconnect_timeout",
    "database_unavailable",
    "dedicated_database_busy",
    "domain_unavailable",
    "domain_writer_busy",
    "encoding_failed",
    "encrypted_candidate_capacity",
    "encrypted_publication_unknown",
    "entity_id_unavailable",
    "event_received_clock_changed",
    "fixture_outage",
    "identity_unavailable",
    "invalid_agent_crypto_receive_response",
    "invalid_agent_crypto_response",
    "invalid_allocation_record",
    "invalid_domain_storage",
    "invalid_identity_response",
    "invalid_matrix_create",
    "invalid_matrix_origin",
    "invalid_matrix_power_levels",
    "invalid_matrix_response",
    "invalid_parent_record",
    "invalid_project_record",
    "invalid_project_response",
    "invalid_room_response",
    "invalid_saved_event",
    "invalid_stored_attachment",
    "invalid_stored_file",
    "matrix_outcome_unknown",
    "matrix_rate_limited",
    "matrix_request_failed",
    "matrix_response_too_large",
    "matrix_state_timeout",
    "matrix_state_unavailable",
    "matrix_unavailable",
    "missing_sender_claim",
    "owner_notice_queue_full",
    "owner_queue_full",
    "parent_liability_overflow",
    "pasion_unavailable",
    "pool_agent_service_join_pending",
    "project_setup_binding_missing",
    "project_setup_membership_outcome_unknown",
    "project_setup_service_unavailable",
    "readiness_unavailable",
    "request_notice_payload_invalid",
    "request_notice_payload_missing",
    "shared_intake_capacity",
    "shared_intake_clock_changed",
    "shared_intake_retry_required",
    "shared_result_ticket_capacity",
    "shared_usage_overflow",
    "shared_work_queue_full",
    "test_database_unavailable",
    "transport_unavailable",
    "worker_stopping",
];

/// The code of every `Unavailable("…")` in the service's own source. Test
/// files are left out: their fixtures answer codes of their own.
fn answered(dir: &Path, found: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "tests") {
                answered(&path, found);
            }
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs")
            || name.ends_with("_tests.rs")
            || name.ends_with("_test.rs")
            || name == "tests.rs"
        {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let mut rest = text.as_str();
        while let Some(at) = rest.find("Unavailable(") {
            rest = &rest[at + "Unavailable(".len()..];
            if let Some(code) = rest
                .trim_start()
                .strip_prefix('"')
                .and_then(|literal| literal.split('"').next())
            {
                found.insert(code.to_owned());
            }
        }
    }
}

#[test]
fn every_503_code_is_a_policy_answer_or_a_failure_to_serve() {
    let mut found = BTreeSet::new();
    answered(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    let policy: BTreeSet<String> = POLICY.iter().map(|code| (*code).to_owned()).collect();
    let other: BTreeSet<String> = COULD_NOT_SERVE
        .iter()
        .map(|code| (*code).to_owned())
        .collect();
    assert_eq!(policy.len(), POLICY.len(), "a policy code is listed twice");
    assert_eq!(other.len(), COULD_NOT_SERVE.len(), "a code is listed twice");
    assert!(
        policy.is_disjoint(&other),
        "a code is on both lists: {:?}",
        policy.intersection(&other).collect::<Vec<_>>()
    );
    let unclassified: Vec<&String> = found
        .iter()
        .filter(|code| !policy.contains(*code) && !other.contains(*code))
        .collect();
    assert!(
        unclassified.is_empty(),
        "put each new 503 code on POLICY or COULD_NOT_SERVE (see the top of this file): {unclassified:?}"
    );
    let gone: Vec<&String> = policy
        .union(&other)
        .filter(|code| !found.contains(*code))
        .collect();
    assert!(
        gone.is_empty(),
        "no longer answered; remove here and, for a policy code, from the computer's list: {gone:?}"
    );
}
