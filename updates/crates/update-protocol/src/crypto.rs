use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("invalid contract: {0}")]
    Invalid(String),
    #[error("untrusted signing key or invalid signature")]
    Signature,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, ProtocolError>;
pub type KeyRing = BTreeMap<String, String>;

pub trait Signable: Serialize {
    const DOMAIN: &'static str;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Signed<T> {
    pub key_id: String,
    pub body: T,
    pub signature: String,
}

pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(serde_jcs::to_vec(value)?)
}
pub fn signed_bytes<T: Signable>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = format!("update-system/{}/v1\n", T::DOMAIN).into_bytes();
    bytes.extend(canonical(value)?);
    Ok(bytes)
}
pub fn sign<T: Signable>(body: T, key_id: String, key: &SigningKey) -> Result<Signed<T>> {
    let signature = STANDARD.encode(key.sign(&signed_bytes(&body)?).to_bytes());
    Ok(Signed {
        key_id,
        body,
        signature,
    })
}
pub fn verify<T: Signable>(signed: &Signed<T>, keys: &KeyRing) -> Result<()> {
    let encoded = keys.get(&signed.key_id).ok_or(ProtocolError::Signature)?;
    let raw = STANDARD
        .decode(encoded)
        .map_err(|_| ProtocolError::Signature)?;
    let bytes: [u8; 32] = raw.try_into().map_err(|_| ProtocolError::Signature)?;
    let key = VerifyingKey::from_bytes(&bytes).map_err(|_| ProtocolError::Signature)?;
    let signature = STANDARD
        .decode(&signed.signature)
        .map_err(|_| ProtocolError::Signature)?;
    let signature = Signature::from_slice(&signature).map_err(|_| ProtocolError::Signature)?;
    key.verify_strict(&signed_bytes(&signed.body)?, &signature)
        .map_err(|_| ProtocolError::Signature)
}

pub fn identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(ProtocolError::Invalid("unsafe identifier".into()));
    }
    Ok(())
}
pub fn hash(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(ProtocolError::Invalid("expected lowercase SHA-256".into()));
    }
    Ok(())
}

pub fn verify_package(bytes: &[u8], signature: &str, public_key: &str) -> Result<()> {
    use minisign_verify::{PublicKey, Signature};
    fn decode_text(value: &str) -> Result<String> {
        if value.trim_start().starts_with("untrusted comment:") {
            return Ok(value.to_owned());
        }
        String::from_utf8(
            STANDARD
                .decode(value.trim())
                .map_err(|_| ProtocolError::Signature)?,
        )
        .map_err(|_| ProtocolError::Signature)
    }
    let key = if public_key.trim_start().starts_with("untrusted comment:") {
        PublicKey::decode(public_key)
    } else {
        PublicKey::from_base64(public_key.trim())
            .or_else(|_| PublicKey::decode(&decode_text(public_key).unwrap_or_default()))
    }
    .map_err(|_| ProtocolError::Signature)?;
    let signature =
        Signature::decode(&decode_text(signature)?).map_err(|_| ProtocolError::Signature)?;
    key.verify(bytes, &signature, true)
        .map_err(|_| ProtocolError::Signature)
}

pub fn validate_trust(keys: &KeyRing, package_public_key: &str) -> Result<()> {
    if keys.is_empty() {
        return Err(ProtocolError::Signature);
    }
    for (id, encoded) in keys {
        identifier(id)?;
        let bytes: [u8; 32] = STANDARD
            .decode(encoded)
            .map_err(|_| ProtocolError::Signature)?
            .try_into()
            .map_err(|_| ProtocolError::Signature)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| ProtocolError::Signature)?;
        if key.is_weak() {
            return Err(ProtocolError::Signature);
        }
    }
    use minisign_verify::PublicKey;
    let parsed = PublicKey::from_base64(package_public_key.trim())
        .or_else(|_| PublicKey::decode(package_public_key))
        .or_else(|_| {
            PublicKey::decode(
                &String::from_utf8(
                    STANDARD
                        .decode(package_public_key.trim())
                        .unwrap_or_default(),
                )
                .unwrap_or_default(),
            )
        });
    parsed.map_err(|_| ProtocolError::Signature)?;
    Ok(())
}
