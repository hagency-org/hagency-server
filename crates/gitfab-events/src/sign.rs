//! Ed25519 signing and offline verification (Annex 3 §8). The signature covers the canonical payload, the
//! event without its top-level `signature`. Private keys stay in their key file; events carry only the
//! key id and a public key reference.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Value, json};

use crate::{Error, canonical};

pub struct Key {
    pub id: String,
    signing: SigningKey,
}

impl Key {
    pub fn generate(id: &str) -> Result<Self, Error> {
        check_key_id(id)?;
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| Error::Key(e.to_string()))?;
        Ok(Self {
            id: id.to_owned(),
            signing: SigningKey::from_bytes(&seed),
        })
    }

    /// A key file holds the base64 32-byte seed. It must not be readable by group or others.
    pub fn load(path: &Path, id: &str) -> Result<Self, Error> {
        check_key_id(id)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)
                .map_err(|e| Error::Key(e.to_string()))?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                return Err(Error::Key(format!(
                    "{} is readable by others (mode {:o}); use 0600",
                    path.display(),
                    mode & 0o777
                )));
            }
        }
        let text = std::fs::read_to_string(path).map_err(|e| Error::Key(e.to_string()))?;
        let seed: [u8; 32] =
            decode(text.trim()).map_err(|_| Error::Key("not a base64 32-byte seed".into()))?;
        Ok(Self {
            id: id.to_owned(),
            signing: SigningKey::from_bytes(&seed),
        })
    }

    pub fn seed_base64(&self) -> String {
        STANDARD.encode(self.signing.to_bytes())
    }

    pub fn public_base64(&self) -> String {
        STANDARD.encode(self.signing.verifying_key().to_bytes())
    }

    /// Returns the event with a fresh signature object; any previous signature is replaced.
    pub fn sign(&self, event: &Value, public_key_ref: &str) -> Result<Value, Error> {
        let payload = canonical::payload(event);
        let bytes = canonical::canonical(&payload)?;
        let signature = self.signing.sign(bytes.as_bytes());
        let mut signed = payload;
        signed
            .as_object_mut()
            .ok_or_else(|| Error::Json("an event is a JSON object".into()))?
            .insert(
                "signature".into(),
                json!({
                    "algorithm": "ed25519",
                    "key_id": self.id,
                    "public_key_ref": public_key_ref,
                    "payload_digest": canonical::sha256(bytes.as_bytes()),
                    "signature_base64": STANDARD.encode(signature.to_bytes()),
                }),
            );
        Ok(signed)
    }
}

/// Public keys by key id, read from `<dir>/<key_id>.pub` files.
#[derive(Default)]
pub struct Keyring(BTreeMap<String, VerifyingKey>);

impl Keyring {
    pub fn load_dir(dir: &Path) -> Result<Self, Error> {
        let mut ring = Self::default();
        let entries =
            std::fs::read_dir(dir).map_err(|e| Error::Key(format!("{}: {e}", dir.display())))?;
        for entry in entries {
            let path = entry.map_err(|e| Error::Key(e.to_string()))?.path();
            let (Some(id), Some("pub")) = (
                path.file_stem().and_then(|s| s.to_str()),
                path.extension().and_then(|s| s.to_str()),
            ) else {
                continue;
            };
            let text = std::fs::read_to_string(&path).map_err(|e| Error::Key(e.to_string()))?;
            ring.insert(id, text.trim())?;
        }
        Ok(ring)
    }

    pub fn insert(&mut self, id: &str, public_base64: &str) -> Result<(), Error> {
        check_key_id(id)?;
        let bytes: [u8; 32] = decode(public_base64)
            .map_err(|_| Error::Key(format!("{id}: not a base64 32-byte public key")))?;
        let key = VerifyingKey::from_bytes(&bytes)
            .map_err(|_| Error::Key(format!("{id}: not an Ed25519 public key")))?;
        self.0.insert(id.to_owned(), key);
        Ok(())
    }

    /// Verifies a signed event. Distinguishes a changed payload, an unknown key and a bad signature.
    pub fn verify(&self, event: &Value) -> Result<(), Error> {
        let sig = event
            .get("signature")
            .filter(|s| !s.is_null())
            .ok_or_else(|| Error::Missing("signature".into()))?;
        let field = |name: &str| {
            sig.get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Missing(format!("signature.{name}")))
        };
        let (key_id, signed_digest, signature_base64) = (
            field("key_id")?,
            field("payload_digest")?,
            field("signature_base64")?,
        );
        let bytes = canonical::canonical(&canonical::payload(event))?;
        let actual = canonical::sha256(bytes.as_bytes());
        if actual != signed_digest {
            return Err(Error::PayloadChanged {
                signed: signed_digest.to_owned(),
                actual,
            });
        }
        let key = self
            .0
            .get(key_id)
            .ok_or_else(|| Error::UnknownKey(key_id.to_owned()))?;
        let raw: [u8; 64] =
            decode(signature_base64).map_err(|_| Error::BadSignature(key_id.to_owned()))?;
        key.verify_strict(bytes.as_bytes(), &Signature::from_bytes(&raw))
            .map_err(|_| Error::BadSignature(key_id.to_owned()))
    }
}

fn decode<const N: usize>(text: &str) -> Result<[u8; N], ()> {
    STANDARD
        .decode(text)
        .map_err(drop)?
        .try_into()
        .map_err(drop)
}

/// Key ids name files, so they are restricted to a safe file-name alphabet.
fn check_key_id(id: &str) -> Result<(), Error> {
    let ok = !id.is_empty()
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(Error::Key(format!("invalid key id {id:?}")))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::schema::{self, tests::memory};

    fn ring(key: &Key) -> Keyring {
        let mut ring = Keyring::default();
        ring.insert(&key.id, &key.public_base64()).unwrap();
        ring
    }

    #[test]
    fn signed_event_verifies_and_passes_the_schema() {
        let key = Key::generate("gitfab-1").unwrap();
        let signed = key.sign(&memory("m1", 5), "keys/gitfab-1.pub").unwrap();
        ring(&key).verify(&signed).unwrap();
        schema::validate(&signed).unwrap();
    }

    #[test]
    fn the_three_failures_are_told_apart() {
        let key = Key::generate("gitfab-1").unwrap();
        let signed = key.sign(&memory("m1", 5), "keys/gitfab-1.pub").unwrap();

        let mut changed = signed.clone();
        changed["token_delta"] = json!(6);
        assert_eq!(
            ring(&key).verify(&changed).unwrap_err().code(),
            "payload_changed"
        );

        let other = Key::generate("gitfab-1").unwrap();
        assert_eq!(
            ring(&other).verify(&signed).unwrap_err().code(),
            "bad_signature"
        );

        assert_eq!(
            Keyring::default().verify(&signed).unwrap_err().code(),
            "unknown_key"
        );

        let mut forged = signed.clone();
        let sig = forged["signature"]["signature_base64"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut raw = STANDARD.decode(sig).unwrap();
        raw[0] ^= 1;
        forged["signature"]["signature_base64"] = json!(STANDARD.encode(raw));
        assert_eq!(
            ring(&key).verify(&forged).unwrap_err().code(),
            "bad_signature"
        );
    }

    #[test]
    fn key_ids_cannot_escape_the_key_directory() {
        for id in ["", "../k", "a/b", ".hidden"] {
            assert!(Key::generate(id).is_err(), "{id}");
        }
    }
}
