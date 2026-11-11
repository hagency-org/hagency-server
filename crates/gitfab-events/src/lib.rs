//! GitFab acceptance events (Annex 3 §6–§8): canonical encoding, schema checks, Ed25519 signatures and the
//! token ledger. Events are append-only JSONL; nothing here rewrites a recorded event.
#![forbid(unsafe_code)]

pub mod canonical;
pub mod identity;
pub mod journal;
pub mod ledger;
pub mod schema;
pub mod sign;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error("non-integer number at {0}; events carry integers only")]
    NonInteger(String),
    #[error("unknown schema_version {0:?}")]
    UnknownSchema(String),
    #[error("missing required field {0}")]
    Missing(String),
    #[error("unknown field {0}")]
    UnknownField(String),
    #[error("field {field} {problem}")]
    Invalid { field: String, problem: String },
    #[error("payload changed: digest {actual} does not match signed {signed}")]
    PayloadChanged { signed: String, actual: String },
    #[error("unknown signing key {0:?}")]
    UnknownKey(String),
    #[error("signature does not verify for key {0:?}")]
    BadSignature(String),
    #[error("signature required for {0} events")]
    MissingSignature(String),
    #[error("no identity event attests this {0} event")]
    MissingIdentity(String),
    #[error("identity event attests no event in this set")]
    OrphanIdentity,
    #[error("{0} was already recorded with different content")]
    Conflict(String),
    #[error("journal: {0}")]
    Io(String),
    #[error("key file: {0}")]
    Key(String),
}

impl Error {
    /// Stable code for reports and scripts.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Json(_) => "invalid_json",
            Self::NonInteger(_) => "non_integer",
            Self::UnknownSchema(_) => "unknown_schema",
            Self::Missing(_) => "missing_field",
            Self::UnknownField(_) => "unknown_field",
            Self::Invalid { .. } => "invalid_field",
            Self::PayloadChanged { .. } => "payload_changed",
            Self::UnknownKey(_) => "unknown_key",
            Self::BadSignature(_) => "bad_signature",
            Self::MissingSignature(_) => "missing_signature",
            Self::MissingIdentity(_) => "missing_identity",
            Self::OrphanIdentity => "orphan_identity",
            Self::Conflict(_) => "conflict",
            Self::Io(_) => "io",
            Self::Key(_) => "key_file",
        }
    }
}

pub(crate) fn invalid(field: &str, problem: impl Into<String>) -> Error {
    Error::Invalid {
        field: field.to_owned(),
        problem: problem.into(),
    }
}
