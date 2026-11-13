//! The Module breakdown a Plan reply must carry (Annex 1 Chapter 2): the task split into Modules,
//! each with its own Spec, test requirements and the capabilities it needs. The Coordinator writes it
//! as one fenced block so it is read exactly, never guessed from prose:
//!
//! ````text
//! ```gitfab-modules
//! [{"id":"M1","title":"…","spec":"…","tests":["cargo test --locked"],"capabilities":["rust"]}]
//! ```
//! ````
//!
//! A Plan reply without a valid block is not a finished Plan; see [`crate::orchestrator_flow::observed`].

use serde::{Deserialize, Serialize};

const FENCE: &str = "```gitfab-modules";
/// Modules in one Plan.
pub const MAX_MODULES: usize = 16;
/// Bytes of one Module's spec; the whole block also stays under the stage handoff limit.
const SPEC_LIMIT: usize = 4 * 1024;
const ITEM_LIMIT: usize = 256;
const LIST_LIMIT: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Module {
    pub id: String,
    pub title: String,
    pub spec: String,
    pub tests: Vec<String>,
    pub capabilities: Vec<String>,
}

/// The Modules of a Plan reply, or why the reply has none that can be used.
pub fn modules(reply: &str) -> Result<Vec<Module>, &'static str> {
    let start = reply.find(FENCE).ok_or("plan_modules_missing")?;
    let rest = &reply[start + FENCE.len()..];
    if rest.contains(FENCE) {
        return Err("plan_modules_repeated");
    }
    let body = rest.strip_prefix('\r').unwrap_or(rest);
    let body = body.strip_prefix('\n').ok_or("plan_modules_malformed")?;
    let end = body.find("```").ok_or("plan_modules_malformed")?;
    let list: Vec<Module> =
        serde_json::from_str(&body[..end]).map_err(|_| "plan_modules_malformed")?;
    if list.is_empty() || list.len() > MAX_MODULES {
        return Err("plan_modules_count");
    }
    let mut seen = std::collections::HashSet::new();
    for m in &list {
        let id_ok = !m.id.is_empty()
            && m.id.len() <= 32
            && m.id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !id_ok || !seen.insert(m.id.as_str()) {
            return Err("plan_module_id");
        }
        if m.title.trim().is_empty() || m.title.len() > ITEM_LIMIT {
            return Err("plan_module_title");
        }
        if m.spec.trim().is_empty() || m.spec.len() > SPEC_LIMIT {
            return Err("plan_module_spec");
        }
        let list_ok = |items: &[String]| {
            !items.is_empty()
                && items.len() <= LIST_LIMIT
                && items
                    .iter()
                    .all(|i| !i.trim().is_empty() && i.len() <= ITEM_LIMIT)
        };
        if !list_ok(&m.tests) {
            return Err("plan_module_tests");
        }
        if !list_ok(&m.capabilities) {
            return Err("plan_module_capabilities");
        }
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(block: &str) -> String {
        format!("实施步骤：……\n\n```gitfab-modules\n{block}\n```\n\n测试方法：……")
    }
    const ONE: &str = r#"[{"id":"M1","title":"任务存储","spec":"内存存储，ID 递增不复用","tests":["cargo test --locked store"],"capabilities":["rust"]}]"#;

    #[test]
    fn a_plan_names_each_module_with_its_spec_tests_and_capabilities() {
        let two = r#"[{"id":"M1","title":"存储","spec":"s","tests":["t"],"capabilities":["rust"]},
                      {"id":"M2","title":"HTTP","spec":"s2","tests":["cases.json"],"capabilities":["rust","http"]}]"#;
        let list = modules(&plan(two)).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].capabilities, ["rust", "http"]);
        assert_eq!(
            modules(&plan(ONE)).unwrap()[0].tests,
            ["cargo test --locked store"]
        );
    }

    #[test]
    fn a_plan_without_a_usable_module_block_is_refused() {
        assert_eq!(modules("只有文字计划"), Err("plan_modules_missing"));
        assert_eq!(modules(&plan("not json")), Err("plan_modules_malformed"));
        assert_eq!(modules(&plan("[]")), Err("plan_modules_count"));
        let missing_tests =
            r#"[{"id":"M1","title":"t","spec":"s","tests":[],"capabilities":["rust"]}]"#;
        assert_eq!(modules(&plan(missing_tests)), Err("plan_module_tests"));
        let no_caps = r#"[{"id":"M1","title":"t","spec":"s","tests":["x"],"capabilities":[" "]}]"#;
        assert_eq!(modules(&plan(no_caps)), Err("plan_module_capabilities"));
        let blank_spec =
            r#"[{"id":"M1","title":"t","spec":"  ","tests":["x"],"capabilities":["rust"]}]"#;
        assert_eq!(modules(&plan(blank_spec)), Err("plan_module_spec"));
        let dup = r#"[{"id":"M1","title":"a","spec":"s","tests":["x"],"capabilities":["r"]},{"id":"M1","title":"b","spec":"s","tests":["x"],"capabilities":["r"]}]"#;
        assert_eq!(modules(&plan(dup)), Err("plan_module_id"));
        let extra = r#"[{"id":"M1","title":"a","spec":"s","tests":["x"],"capabilities":["r"],"owner":"me"}]"#;
        assert_eq!(modules(&plan(extra)), Err("plan_modules_malformed"));
        assert_eq!(
            modules(&format!("{}\n{}", plan(ONE), plan(ONE))),
            Err("plan_modules_repeated")
        );
        let unclosed = format!("```gitfab-modules\n{ONE}\n");
        assert_eq!(modules(&unclosed), Err("plan_modules_malformed"));
    }
}
