//! AES-256-GCM with HKDF-SHA256, wire-compatible with PolyBase Swift.
//!
//! Wire-format invariants (must remain compatible with Swift PolyBase):
//! - String ciphertext: `enc:` prefix + base64(nonce || ciphertext || tag)
//! - Binary ciphertext: `ENC\0` header (0x45 0x4E 0x43 0x00) + nonce || ciphertext || tag
//! - Key derivation: HKDF-SHA256 with uppercased UUID as salt, info = `supabase-encryption`
//! - Plaintext pass-through: values without prefix/header are returned unchanged
//!
//! Lifted from Tauri Prism's `src-tauri/src/crypto/encryption.rs` with minor refactoring to keep
//! state in an `Arc<Encryption>` rather than a `Mutex<Option<Encryption>>` global.

use std::sync::Arc;

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{Engine as _, engine::general_purpose};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::contract::{BINARY_ENCRYPTION_HEADER, HKDF_INFO, STRING_ENCRYPTION_PREFIX};

/// Errors produced by the encryption layer.
#[derive(Debug, thiserror::Error, Clone)]
pub enum EncryptionError {
    /// Encryption secret was empty (configuration error).
    #[error("encryption secret is empty")]
    EmptySecret,
    /// AES-GCM, HKDF, or base64 operation failed.
    #[error("encryption operation failed: {0}")]
    OperationFailed(String),
}

/// AES-GCM encryption configured with an app-level secret. Cheap to clone (`Arc`).
#[derive(Debug, Clone)]
pub struct Encryption {
    inner: Arc<EncryptionInner>,
}

#[derive(Debug)]
struct EncryptionInner {
    app_secret: Vec<u8>,
    secret_fingerprint: String,
}

impl Encryption {
    /// Build a new encryptor from an app-level secret string. The secret is fingerprinted (first
    /// 12 hex chars of SHA-256) for inclusion in error logs to help diagnose key drift.
    pub fn new(secret: &str) -> Result<Self, EncryptionError> {
        if secret.is_empty() {
            return Err(EncryptionError::EmptySecret);
        }
        let app_secret = secret.as_bytes().to_vec();
        let secret_fingerprint = make_secret_fingerprint(&app_secret);
        Ok(Self { inner: Arc::new(EncryptionInner { app_secret, secret_fingerprint }) })
    }

    /// First 12 hex chars of SHA-256(secret) — safe to log; useful for diagnosing key drift.
    pub fn fingerprint(&self) -> &str {
        &self.inner.secret_fingerprint
    }

    /// True when the input begins with the canonical string-encryption prefix.
    pub fn is_encrypted(&self, text: &str) -> bool {
        text.starts_with(STRING_ENCRYPTION_PREFIX)
    }

    /// True when the input begins with the canonical binary-encryption magic header.
    pub fn is_data_encrypted(&self, data: &[u8]) -> bool {
        data.starts_with(&BINARY_ENCRYPTION_HEADER)
    }

    /// Convert any user-id string into a UUID for HKDF salting. Native UUID strings parse
    /// directly; non-UUID inputs are folded into a v5 namespace UUID for stability.
    pub fn key_user_uuid(user_id: &str) -> Uuid {
        user_id.parse().unwrap_or_else(|_| {
            Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("polybase:{user_id}").as_bytes())
        })
    }

    /// Encrypt a string for a specific user. Empty input returns empty.
    pub fn encrypt(&self, plaintext: &str, user_id: Uuid) -> Result<String, EncryptionError> {
        if plaintext.is_empty() {
            return Ok(plaintext.to_string());
        }

        let key = self.derive_key(user_id)?;
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let cipher = Aes256Gcm::new(&key);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| EncryptionError::OperationFailed(format!("encrypt: {e}")))?;

        let mut combined = nonce_bytes.to_vec();
        combined.extend_from_slice(&ciphertext);
        Ok(format!("{STRING_ENCRYPTION_PREFIX}{}", general_purpose::STANDARD.encode(&combined)))
    }

    /// Decrypt a string for a specific user. Returns the input unchanged if not prefixed.
    pub fn decrypt(&self, ciphertext: &str, user_id: Uuid) -> Result<String, EncryptionError> {
        if !self.is_encrypted(ciphertext) {
            return Ok(ciphertext.to_string());
        }

        let key = self.derive_key(user_id)?;
        let base64_string = &ciphertext[STRING_ENCRYPTION_PREFIX.len()..];
        let combined = general_purpose::STANDARD
            .decode(base64_string)
            .map_err(|e| EncryptionError::OperationFailed(format!("base64: {e}")))?;

        if combined.len() < 12 {
            return Err(EncryptionError::OperationFailed("ciphertext too short".into()));
        }

        let nonce = Nonce::from_slice(&combined[..12]);
        let payload = &combined[12..];
        let cipher = Aes256Gcm::new(&key);

        let decrypted = cipher
            .decrypt(nonce, payload)
            .map_err(|e| EncryptionError::OperationFailed(format!("decrypt: {e}")))?;

        String::from_utf8(decrypted)
            .map_err(|e| EncryptionError::OperationFailed(format!("utf8: {e}")))
    }

    /// Decrypt and additionally report whether the input was actually ciphertext (i.e. carried
    /// the `enc:` prefix). Apps can use this to schedule re-push of legacy plaintext rows
    /// (the healing pattern from Swift PolyBase).
    pub fn decrypt_with_healing(
        &self,
        ciphertext: &str,
        user_id: Uuid,
    ) -> Result<(String, bool), EncryptionError> {
        let was_encrypted = self.is_encrypted(ciphertext);
        let decrypted = self.decrypt(ciphertext, user_id)?;
        Ok((decrypted, was_encrypted))
    }

    /// Same as [`Self::decrypt_with_healing`] but accepts an `Option`-shaped column. Returns
    /// `None` when the input is `None`, matching the ergonomics of decoding nullable fields
    /// straight off a row deserializer.
    pub fn decrypt_optional_with_healing(
        &self,
        ciphertext: Option<&str>,
        user_id: Uuid,
    ) -> Result<Option<(String, bool)>, EncryptionError> {
        match ciphertext {
            Some(text) => Ok(Some(self.decrypt_with_healing(text, user_id)?)),
            None => Ok(None),
        }
    }

    /// Encrypt a binary blob.
    pub fn encrypt_data(&self, data: &[u8], user_id: Uuid) -> Result<Vec<u8>, EncryptionError> {
        if data.is_empty() {
            return Ok(data.to_vec());
        }

        let key = self.derive_key(user_id)?;
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let cipher = Aes256Gcm::new(&key);

        let ciphertext = cipher
            .encrypt(nonce, data)
            .map_err(|e| EncryptionError::OperationFailed(format!("encrypt_data: {e}")))?;

        let mut result = BINARY_ENCRYPTION_HEADER.to_vec();
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Decrypt a binary blob; returns input unchanged if no magic header.
    pub fn decrypt_data(&self, data: &[u8], user_id: Uuid) -> Result<Vec<u8>, EncryptionError> {
        if !self.is_data_encrypted(data) {
            return Ok(data.to_vec());
        }

        let key = self.derive_key(user_id)?;
        let combined = &data[BINARY_ENCRYPTION_HEADER.len()..];

        if combined.len() < 12 {
            return Err(EncryptionError::OperationFailed("encrypted data too short".into()));
        }

        let nonce = Nonce::from_slice(&combined[..12]);
        let payload = &combined[12..];
        let cipher = Aes256Gcm::new(&key);

        cipher
            .decrypt(nonce, payload)
            .map_err(|e| EncryptionError::OperationFailed(format!("decrypt_data: {e}")))
    }

    fn derive_key(&self, user_id: Uuid) -> Result<Key<Aes256Gcm>, EncryptionError> {
        let salt = user_id.to_string().to_uppercase().into_bytes();
        let hkdf = Hkdf::<Sha256>::new(Some(&salt), &self.inner.app_secret);
        let mut okm = [0u8; 32];
        hkdf.expand(HKDF_INFO.as_bytes(), &mut okm)
            .map_err(|e| EncryptionError::OperationFailed(format!("hkdf expand: {e}")))?;
        Ok(*Key::<Aes256Gcm>::from_slice(&okm))
    }
}

fn make_secret_fingerprint(secret_data: &[u8]) -> String {
    let digest = Sha256::digest(secret_data);
    digest.iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc() -> Encryption {
        Encryption::new("test-secret-1234").unwrap()
    }

    fn user() -> Uuid {
        Encryption::key_user_uuid("11111111-2222-3333-4444-555555555555")
    }

    #[test]
    fn empty_secret_rejected() {
        assert!(matches!(Encryption::new(""), Err(EncryptionError::EmptySecret)));
    }

    #[test]
    fn round_trip_string() {
        let e = enc();
        let u = user();
        let cipher = e.encrypt("hello world", u).unwrap();
        assert!(cipher.starts_with("enc:"));
        let plain = e.decrypt(&cipher, u).unwrap();
        assert_eq!(plain, "hello world");
    }

    #[test]
    fn empty_string_passes_through() {
        let e = enc();
        let u = user();
        assert_eq!(e.encrypt("", u).unwrap(), "");
        assert_eq!(e.decrypt("", u).unwrap(), "");
    }

    #[test]
    fn decrypt_unprefixed_returns_input() {
        let e = enc();
        let u = user();
        assert_eq!(e.decrypt("not encrypted", u).unwrap(), "not encrypted");
    }

    #[test]
    fn round_trip_binary() {
        let e = enc();
        let u = user();
        let blob = b"\x00\x01\x02\x03binary data".to_vec();
        let cipher = e.encrypt_data(&blob, u).unwrap();
        assert!(cipher.starts_with(b"ENC\0"));
        let plain = e.decrypt_data(&cipher, u).unwrap();
        assert_eq!(plain, blob);
    }

    #[test]
    fn binary_passthrough_without_header() {
        let e = enc();
        let u = user();
        let blob = b"raw bytes".to_vec();
        assert_eq!(e.decrypt_data(&blob, u).unwrap(), blob);
    }

    #[test]
    fn decrypt_with_healing_flags_legacy_plaintext() {
        let e = enc();
        let u = user();
        let (text, was_encrypted) = e.decrypt_with_healing("legacy plaintext", u).unwrap();
        assert_eq!(text, "legacy plaintext");
        assert!(!was_encrypted);
    }

    #[test]
    fn decrypt_optional_with_healing_handles_none_and_some() {
        let e = enc();
        let u = user();

        assert!(e.decrypt_optional_with_healing(None, u).unwrap().is_none());

        let cipher = e.encrypt("payload", u).unwrap();
        let (text, was_encrypted) =
            e.decrypt_optional_with_healing(Some(cipher.as_str()), u).unwrap().unwrap();
        assert_eq!(text, "payload");
        assert!(was_encrypted);

        let (text, was_encrypted) =
            e.decrypt_optional_with_healing(Some("legacy"), u).unwrap().unwrap();
        assert_eq!(text, "legacy");
        assert!(!was_encrypted);
    }

    #[test]
    fn user_uuid_falls_back_to_v5_for_non_uuid_input() {
        let id_a = Encryption::key_user_uuid("not a uuid");
        let id_b = Encryption::key_user_uuid("not a uuid");
        let id_c = Encryption::key_user_uuid("also not a uuid");
        assert_eq!(id_a, id_b);
        assert_ne!(id_a, id_c);
    }
}
