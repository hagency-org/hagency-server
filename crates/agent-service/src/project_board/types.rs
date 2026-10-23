//! Wire contract shared conceptually with the native Client; no runner capabilities.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const STATES: [&str; 5] = ["todo", "in_progress", "blocked", "in_review", "done"];
pub const PRIORITIES: [&str; 4] = ["low", "normal", "high", "urgent"];
pub const CAPABILITY: &str = "project-board-v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fields {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "normal")]
    pub priority: String,
    #[serde(default)]
    pub assignee_mxid: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub due_date: Option<String>,
}
impl Default for Fields {
    fn default() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            priority: normal(),
            assignee_mxid: None,
            labels: Vec::new(),
            due_date: None,
        }
    }
}
fn normal() -> String {
    "normal".into()
}
impl Fields {
    pub fn validate(&mut self) -> Result<()> {
        self.title = self.title.trim().into();
        if self.title.is_empty()
            || self.title.chars().count() > 240
            || self.title.chars().any(char::is_control)
            || self.description.len() > 32768
            || !PRIORITIES.contains(&self.priority.as_str())
            || self.labels.len() > 12
            || self.assignee_mxid.as_ref().is_some_and(|s| s.len() > 255)
        {
            return Err(Error::Invalid("invalid_board_fields"));
        }
        let mut labels = std::collections::BTreeSet::new();
        for label in &self.labels {
            let label = label.trim();
            if label.is_empty() || label.chars().count() > 32 || label.chars().any(char::is_control)
            {
                return Err(Error::Invalid("invalid_board_labels"));
            }
            labels.insert(label.to_owned());
        }
        self.labels = labels.into_iter().collect();
        if let Some(date) = &self.due_date {
            validate_date(date)?;
        }
        Ok(())
    }
}
pub fn validate_date(date: &str) -> Result<()> {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return Err(Error::Invalid("invalid_board_date"));
    }
    let year: u32 = date[..4]
        .parse()
        .map_err(|_| Error::Invalid("invalid_board_date"))?;
    let month: u32 = date[5..7]
        .parse()
        .map_err(|_| Error::Invalid("invalid_board_date"))?;
    let day: u32 = date[8..]
        .parse()
        .map_err(|_| Error::Invalid("invalid_board_date"))?;
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    if year == 0 || day == 0 || day > max {
        return Err(Error::Invalid("invalid_board_date"));
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    pub idempotency_key: String,
    pub fields: Fields,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    pub idempotency_key: String,
    pub expected_version: i64,
    pub fields: Fields,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Move {
    pub idempotency_key: String,
    pub expected_version: i64,
    pub state: String,
    /// Exact neighbours are allowed only when the lane revision is still current.
    pub expected_sequence: Option<i64>,
    pub before: Option<String>,
    pub after: Option<String>,
    #[serde(default)]
    pub position: Position,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[default]
    Append,
    Prepend,
    Between,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Versioned {
    pub idempotency_key: String,
    pub expected_version: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Comment {
    pub idempotency_key: String,
    pub body: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub state: Option<String>,
    pub assignee: Option<String>,
    pub label: Option<String>,
    pub query: Option<String>,
    #[serde(default)]
    pub overdue: bool,
    #[serde(default)]
    pub archived: bool,
    pub cursor: Option<String>,
    pub sequence: Option<i64>,
    pub limit: Option<i64>,
}
impl Query {
    pub fn validate(&self) -> Result<()> {
        if self
            .state
            .as_ref()
            .is_some_and(|s| !STATES.contains(&s.as_str()))
            || self.query.as_ref().is_some_and(|s| s.chars().count() > 120)
            || self.assignee.as_ref().is_some_and(|s| s.len() > 255)
            || self.label.as_ref().is_some_and(|s| s.chars().count() > 32)
            || self.limit.is_some_and(|n| !(1..=100).contains(&n))
            || self.sequence.is_some_and(|n| n < 0)
        {
            return Err(Error::Invalid("invalid_board_query"));
        }
        if let Some(cursor) = &self.cursor {
            crate::key(cursor)?;
        }
        Ok(())
    }
}
