//! Canonical encoding (Annex 3 §8): object keys sorted by code point, no insignificant whitespace, UTF-8.
//! Numbers are restricted to integers, so the encoding never depends on float formatting.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Error;

pub fn canonical(value: &Value) -> Result<String, Error> {
    let mut out = String::new();
    write(value, "$", &mut out)?;
    Ok(out)
}

/// The signed payload: the event without its top-level `signature`.
pub fn payload(event: &Value) -> Value {
    let mut payload = event.clone();
    if let Value::Object(map) = &mut payload {
        map.remove("signature");
    }
    payload
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn payload_digest(event: &Value) -> Result<String, Error> {
    Ok(sha256(canonical(&payload(event))?.as_bytes()))
}

fn write(value: &Value, at: &str, out: &mut String) -> Result<(), Error> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => out.push_str(&value.to_string()),
        Value::Number(n) if n.is_i64() || n.is_u64() => out.push_str(&n.to_string()),
        Value::Number(_) => return Err(Error::NonInteger(at.to_owned())),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, &format!("{at}[{i}]"), out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write(&map[key], &format!("{at}.{key}"), out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn key_order_and_whitespace_do_not_change_the_encoding() {
        let a: Value =
            serde_json::from_str(r#"{ "b": 1, "a": {"y": [2, "x"], "x": null} }"#).unwrap();
        let b = json!({"a": {"x": null, "y": [2, "x"]}, "b": 1});
        assert_eq!(
            canonical(&a).unwrap(),
            r#"{"a":{"x":null,"y":[2,"x"]},"b":1}"#
        );
        assert_eq!(canonical(&a).unwrap(), canonical(&b).unwrap());
    }

    #[test]
    fn floats_are_refused_with_their_path() {
        let err = canonical(&json!({"m": [{"token_delta": 1.5}]})).unwrap_err();
        assert_eq!(err, Error::NonInteger("$.m[0].token_delta".into()));
        assert!(canonical(&json!({"n": 1.0})).is_err());
    }

    #[test]
    fn payload_digest_ignores_only_the_top_level_signature() {
        let signed = json!({"a": 1, "signature": {"sig": "x"}});
        assert_eq!(
            payload_digest(&signed).unwrap(),
            payload_digest(&json!({"a": 1})).unwrap()
        );
        assert_ne!(
            payload_digest(&json!({"a": 1, "b": {"signature": 1}})).unwrap(),
            payload_digest(&json!({"a": 1})).unwrap()
        );
    }
}
