//! The fixed Agent team and what each role declares it can do (Annex 1 §1.2.1(4)). Stages route to
//! roles by `Stage::role()`; the team must declare every role that routing uses, so no stage can go
//! to a role nobody declared. A Plan's Modules may only need capabilities the Implementer declares,
//! since the Implementer executes every Module.

use crate::orchestrator_flow::{Role, Stage};
use crate::orchestrator_modules::Module;

const ROLES: [(&str, Role); 3] = [
    ("coordinator", Role::Coordinator),
    ("implementer", Role::Implementer),
    ("reviewer", Role::Reviewer),
];
const CAPABILITY_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Team {
    /// Declared capabilities per role, in `ROLES` order.
    capabilities: [Vec<String>; 3],
}

fn index(role: Role) -> usize {
    ROLES
        .iter()
        .position(|(_, r)| *r == role)
        .expect("every role is listed")
}

impl Team {
    /// A team from `(role, capabilities)` pairs as configured. Every role that a stage routes to must
    /// be declared exactly once, with at least one capability.
    pub fn new<'a>(
        declared: impl IntoIterator<Item = (&'a str, Vec<String>)>,
    ) -> Result<Self, String> {
        let mut capabilities: [Option<Vec<String>>; 3] = [None, None, None];
        for (name, caps) in declared {
            let (_, role) = ROLES
                .iter()
                .find(|(n, _)| *n == name)
                .ok_or_else(|| format!("unknown role `{name}`"))?;
            let slot = &mut capabilities[index(*role)];
            if slot.is_some() {
                return Err(format!("role `{name}` declared twice"));
            }
            let caps: Vec<String> = caps.into_iter().map(|c| c.trim().to_owned()).collect();
            if caps.is_empty()
                || caps.len() > CAPABILITY_LIMIT
                || caps.iter().any(|c| c.is_empty() || c.len() > 64)
            {
                return Err(format!(
                    "role `{name}` needs 1 to {CAPABILITY_LIMIT} non-empty capabilities"
                ));
            }
            *slot = Some(caps);
        }
        for stage in [
            Stage::Spec,
            Stage::Plan,
            Stage::Execute,
            Stage::Test,
            Stage::Review,
            Stage::Release,
        ] {
            let role = stage.role();
            if capabilities[index(role)].is_none() {
                let name = ROLES[index(role)].0;
                return Err(format!(
                    "stage `{}` routes to `{name}`, which the team does not declare",
                    stage.phase()
                ));
            }
        }
        Ok(Self {
            capabilities: capabilities.map(Option::unwrap_or_default),
        })
    }

    pub fn capabilities(&self, role: Role) -> &[String] {
        &self.capabilities[index(role)]
    }

    /// Capabilities the Modules need that the Implementer does not declare, in first-seen order.
    pub fn uncovered(&self, modules: &[Module]) -> Vec<String> {
        let have = self.capabilities(Role::Implementer);
        let mut missing: Vec<String> = Vec::new();
        for need in modules.iter().flat_map(|m| &m.capabilities) {
            let need = need.trim();
            if !have.iter().any(|h| h.eq_ignore_ascii_case(need))
                && !missing.iter().any(|m| m.eq_ignore_ascii_case(need))
            {
                missing.push(need.to_owned());
            }
        }
        missing
    }

    /// The team line the Plan stage message carries, so the Coordinator plans within it.
    pub fn plan_note(&self) -> String {
        format!(
            "执行团队：实现角色（implementer）声明的能力为 {}。每个 Module 的 capabilities 只能从中选择，否则计划不算完成。",
            self.capabilities(Role::Implementer).join("、")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }
    fn team() -> Team {
        Team::new([
            ("coordinator", caps(&["planning"])),
            ("implementer", caps(&["rust", "http", "docker"])),
            ("reviewer", caps(&["code-review"])),
        ])
        .unwrap()
    }
    fn module(id: &str, needs: &[&str]) -> Module {
        Module {
            id: id.into(),
            title: "t".into(),
            spec: "s".into(),
            tests: caps(&["cargo test"]),
            capabilities: caps(needs),
        }
    }

    #[test]
    fn the_team_must_declare_every_role_routing_uses() {
        assert_eq!(
            team().capabilities(Role::Implementer),
            ["rust", "http", "docker"]
        );
        let missing_reviewer = Team::new([
            ("coordinator", caps(&["planning"])),
            ("implementer", caps(&["rust"])),
        ]);
        assert_eq!(
            missing_reviewer,
            Err("stage `review` routes to `reviewer`, which the team does not declare".into())
        );
        assert!(
            Team::new([("tester", caps(&["x"]))])
                .unwrap_err()
                .contains("unknown role")
        );
        let twice = Team::new([("coordinator", caps(&["a"])), ("coordinator", caps(&["b"]))]);
        assert!(twice.unwrap_err().contains("declared twice"));
        let empty = Team::new([
            ("coordinator", caps(&["a"])),
            ("implementer", caps(&[" "])),
            ("reviewer", caps(&["c"])),
        ]);
        assert!(empty.unwrap_err().contains("non-empty capabilities"));
    }

    #[test]
    fn modules_may_need_only_what_the_implementer_declares() {
        let t = team();
        assert!(
            t.uncovered(&[module("M1", &["rust"]), module("M2", &["HTTP", "docker"])])
                .is_empty()
        );
        assert_eq!(
            t.uncovered(&[
                module("M1", &["rust", "python"]),
                module("M2", &["python", "k8s"])
            ]),
            ["python", "k8s"]
        );
        // The Reviewer's capabilities do not cover a Module.
        assert_eq!(
            t.uncovered(&[module("M1", &["code-review"])]),
            ["code-review"]
        );
        assert!(t.plan_note().contains("rust、http、docker"));
    }
}
