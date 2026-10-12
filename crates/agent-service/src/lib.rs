//! New Agent-domain identity. No Fleet credentials or legacy storage imports.
pub mod api;
#[cfg(test)]
mod api_discovery_tests;
mod api_transport;
pub mod appservice;
mod database;
pub mod domain;
pub mod gateway;
pub mod identity;
pub mod matrix_client;
pub mod store;
pub mod transport;
mod workers;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Unauthorized(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Unavailable(&'static str),
    #[error("storage_unavailable")]
    Database(#[from] diesel::result::Error),
}
impl Error {
    pub fn status(&self) -> u16 {
        match self {
            Self::Invalid(_) => 400,
            Self::Unauthorized(_) => 401,
            Self::Conflict(_) => 409,
            Self::NotFound(_) => 404,
            Self::Unavailable(_) | Self::Database(_) => 503,
        }
    }
}
/// Public entity identity, ordered within this process even in the same millisecond.
/// Lowercase Crockford Base32 also fits Matrix user localparts.
pub(crate) fn entity_id() -> Result<String> {
    static IDS: std::sync::Mutex<ulid::Generator> = std::sync::Mutex::new(ulid::Generator::new());
    let mut ids = IDS
        .lock()
        .map_err(|_| Error::Unavailable("entity_id_unavailable"))?;
    ids.generate()
        .map(|id| id.to_string().to_ascii_lowercase())
        .map_err(|_| Error::Unavailable("entity_id_unavailable"))
}
/// Bearer credentials and worker claims require independent full-entropy randomness.
pub(crate) fn secret_token() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}
pub(crate) fn hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(value.as_bytes()))
}
pub(crate) fn key(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::Invalid("invalid_identifier"));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn assert_entity_id(value: &str, prefix: &str) {
    let suffix = value.strip_prefix(prefix).expect("entity prefix");
    assert_eq!(suffix.len(), 26);
    assert!(
        suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || b"abcdefghjkmnpqrstvwxyz".contains(&b))
    );
    ulid::Ulid::from_string(suffix).expect("valid ULID");
    key(value).expect("valid API identifier");
}

#[cfg(test)]
mod id_tests {
    #[test]
    fn entity_ids_are_lowercase_and_monotonic() {
        let mut previous = String::new();
        for _ in 0..256 {
            let next = super::entity_id().unwrap();
            super::assert_entity_id(&format!("agt_{next}"), "agt_");
            assert!(next > previous);
            let mxid = format!("@_hagency_agt_{next}:example.test");
            assert!(mxid.len() < 255);
            previous = next;
        }
    }

    #[test]
    fn credentials_remain_independent_256_bit_random_values() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..32 {
            let secret = super::secret_token();
            assert_eq!(secret.len(), 64);
            assert_eq!(hex::decode(&secret).unwrap().len(), 32);
            assert!(seen.insert(secret));
        }
    }
}
