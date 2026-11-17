use super::*;
use crate::management_shared_attempt_types::{SharedAttemptCommand, SharedAttemptProcessReceipt};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-budget-execution-v3.json"
    ))
    .unwrap()
}

#[test]
fn budget_execution_v3_canonical_hashes_match_without_reinterpreting_v2() {
    let f = fixture();
    let c: SharedBudgetAttemptCommand = serde_json::from_value(f["command"].clone()).unwrap();
    c.validate("example.test").unwrap();
    let r: SharedBudgetAttemptProcessReceipt =
        serde_json::from_value(f["receipt"].clone()).unwrap();
    r.validate_for(&c).unwrap();
    let start: SharedBudgetAttemptStartInput = serde_json::from_value(f["start"].clone()).unwrap();
    start.validate().unwrap();
    let o: SharedDemandOffer = serde_json::from_value(f["offer"].clone()).unwrap();
    let d: SharedBudgetDemandStartInput = serde_json::from_value(f["demandStart"].clone()).unwrap();
    d.validate_for(&o).unwrap();
    for (value, expected) in [
        (serde_json::to_value(&c).unwrap(), "commandDigest"),
        (serde_json::to_value(&r).unwrap(), "receiptDigest"),
        (serde_json::to_value(&start).unwrap(), "startDigest"),
        (serde_json::to_value(&d).unwrap(), "demandStartDigest"),
        (
            serde_json::to_value(&c.execution_budget).unwrap(),
            "executionBudgetDigest",
        ),
    ] {
        assert_eq!(allocation_digest(&value).unwrap(), f[expected]);
    }
    assert!(c.issued_at_ms > c.original_runtime_command.expires_at_ms);
    assert_eq!(
        c.original_runtime_command.reserve_command.requested_tokens,
        1000
    );
    assert_eq!(c.execution_budget.effective_ceiling_tokens, 1256);
    assert_eq!(c.budget_observation.retained_tokens, 1060);
    assert_eq!(c.budget_observation.remaining_tokens, 196);
    assert!(serde_json::from_value::<SharedAttemptCommand>(f["command"].clone()).is_err());
    assert!(serde_json::from_value::<SharedAttemptProcessReceipt>(f["receipt"].clone()).is_err());
    let old: Value = serde_json::from_str(include_str!(
        "../tests/fixtures/management-shared-attempt-v2.json"
    ))
    .unwrap();
    let old_command: SharedAttemptCommand = serde_json::from_value(old["command"].clone()).unwrap();
    old_command.validate("example.test").unwrap();
    assert_eq!(
        allocation_digest(&old_command).unwrap(),
        old["commandDigest"]
    );
    assert!(serde_json::from_value::<SharedBudgetAttemptCommand>(old["command"].clone()).is_err());
}

#[test]
fn budget_execution_v3_rejects_genesis_foreign_pin_increased_old_wire_and_exhaustion() {
    let f = fixture();
    let c: SharedBudgetAttemptCommand = serde_json::from_value(f["command"].clone()).unwrap();
    let mut bad = c.clone();
    bad.execution_budget.budget_revision = 1;
    assert!(bad.validate("example.test").is_err());
    let mut bad = c.clone();
    bad.execution_budget.original_reserve_receipt_digest = "f".repeat(64);
    assert!(bad.validate("example.test").is_err());
    let mut bad = c.clone();
    bad.budget_observation.allocated_tokens = 1257;
    bad.budget_observation.remaining_tokens = 197;
    assert!(bad.validate("example.test").is_err());
    let mut bad = c.clone();
    bad.budget_observation.retained_tokens = 1256;
    bad.budget_observation.remaining_tokens = 0;
    assert!(bad.validate("example.test").is_err());
    let mut bad = c;
    bad.expires_at_ms += 1;
    assert!(bad.validate("example.test").is_err());
    let mut raw = f["command"].clone();
    raw["ignoredApproval"] = json!(true);
    assert!(serde_json::from_value::<SharedBudgetAttemptCommand>(raw).is_err());
    let mut raw = f["executionBudget"].clone();
    raw["budgetRevision"] = json!(2.5);
    assert!(serde_json::from_value::<SharedExecutionBudget>(raw).is_err());
}

#[test]
fn budget_execution_v3_reassessment_preserves_exact_offer_block_and_control() {
    let f = fixture();
    let o: SharedDemandOffer = serde_json::from_value(f["offer"].clone()).unwrap();
    let d: SharedBudgetDemandStartInput = serde_json::from_value(f["demandStart"].clone()).unwrap();
    d.validate_for(&o).unwrap();
    assert_eq!(
        d.original_block.as_ref().unwrap().payload_digest,
        allocation_digest(&f["originalBlockPayload"]).unwrap()
    );
    let mut bad = d.clone();
    bad.expected_control_revision += 2;
    assert!(bad.validate_for(&o).is_err());
    let mut bad = d.clone();
    bad.start.predecessor.command_id = "unrelated_head".into();
    assert!(bad.validate_for(&o).is_err());
    let mut bad = d;
    bad.original_block.as_mut().unwrap().reason = "usage_unrepresentable".into();
    assert!(bad.validate_for(&o).is_err());
}

#[test]
fn budget_execution_v3_no_start_description_pins_exact_work_input_and_old_budget() {
    let f = fixture();
    let c: SharedBudgetAttemptCommand = serde_json::from_value(f["command"].clone()).unwrap();
    let w: SharedWorkItem = serde_json::from_value(f["promotedWork"].clone()).unwrap();
    let r: SharedBudgetWorkRefusal = serde_json::from_value(f["refusal"].clone()).unwrap();
    r.validate_for(&c, &w, &w.body_digest).unwrap();
    assert_eq!(allocation_digest(&r).unwrap(), f["refusalDigest"]);
    let mut bad = r.clone();
    bad.state = "stopped".into();
    assert!(bad.validate_for(&c, &w, &w.body_digest).is_err());
    let mut bad = r.clone();
    bad.input_digest = "e".repeat(64);
    assert!(bad.validate_for(&c, &w, &w.body_digest).is_err());
    let mut bad = r.clone();
    bad.execution_budget.budget_revision += 1;
    assert!(bad.validate_for(&c, &w, &w.body_digest).is_err());
    let mut bad = r;
    bad.current_budget_digest = bad.execution_budget.budget_digest.clone();
    assert!(bad.validate_for(&c, &w, &w.body_digest).is_err());
}
