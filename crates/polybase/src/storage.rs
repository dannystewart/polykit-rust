//! Supabase Storage REST adapter.
//!
//! This module is a thin wrapper over the `/storage/v1/object/` REST surface. It consumes a
//! [`Client`] and the active session's access token so authorization headers are unified across
//! polybase subsystems.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::client::Client;
use crate::encryption::Encryption;
use crate::errors::{PolyError, StorageError};

/// Single entry returned by [`Bucket::list`] (mirrors Supabase Storage `objects` row).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObjectEntry {
    /// Object name, relative to the requested prefix.
    pub name: String,
    /// Storage object id (UUID), or `None` for prefix-only entries (subdirectories).
    #[serde(default)]
    pub id: Option<String>,
    /// Size in bytes, when reported by Storage.
    #[serde(default)]
    pub metadata: Option<ObjectMetadata>,
    /// ISO-8601 creation timestamp.
    #[serde(default)]
    pub created_at: Option<String>,
    /// ISO-8601 last-update timestamp.
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Metadata reported alongside an [`ObjectEntry`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObjectMetadata {
    /// Object size in bytes, when known.
    #[serde(default)]
    pub size: Option<u64>,
    /// MIME type, when known.
    #[serde(default)]
    pub mimetype: Option<String>,
}

/// Sort options for [`Bucket::list`].
#[derive(Debug, Clone)]
pub struct ListSort {
    /// Column to sort on, e.g. `name`, `updated_at`.
    pub column: String,
    /// `asc` or `desc`.
    pub order: String,
}

impl ListSort {
    /// Convenience constructor accepting any string-like input.
    pub fn new(column: impl Into<String>, order: impl Into<String>) -> Self {
        Self { column: column.into(), order: order.into() }
    }
}

/// Pagination + sort options for [`Bucket::list`]. All fields are optional.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// Page size; Supabase defaults to 100 when omitted, capped at 1000.
    pub limit: Option<u32>,
    /// Skip this many entries.
    pub offset: Option<u32>,
    /// Filename substring filter passed straight through to Storage.
    pub search: Option<String>,
    /// Sort column + direction.
    pub sort_by: Option<ListSort>,
}

/// Response payload returned by [`Bucket::create_signed_url`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SignedUrl {
    /// Fully-qualified, time-limited URL the caller can hand to a download client.
    #[serde(rename = "signedURL")]
    pub url: String,
}

/// Adapter for one Supabase Storage bucket.
///
/// Optionally carries an [`Encryption`] engine bound to a specific user (via
/// [`Bucket::with_encryption`]). When set, [`Bucket::upload`] auto-encrypts plaintext bytes
/// before sending them to Supabase Storage, and [`Bucket::download`] auto-decrypts any
/// payload that carries the [`crate::contract::BINARY_ENCRYPTION_HEADER`] (`ENC\0`) magic
/// header. Plaintext-on-disk objects (legacy, or bytes uploaded by a non-encrypted client)
/// pass through download untouched.
///
/// Without an attached encryption engine, both directions act as raw byte passthroughs —
/// the historical behavior. This is the right mode for public assets (e.g. avatars in a
/// public bucket) where encryption is intentionally not desired.
#[derive(Debug, Clone)]
pub struct Bucket {
    client: Client,
    bucket: String,
    encryption: Option<(Encryption, Uuid)>,
}

impl Bucket {
    /// Build an adapter for the named bucket on the given client. No encryption is attached
    /// by default; call [`Bucket::with_encryption`] to make upload/download transparent over
    /// the wire encryption format.
    pub fn new(client: Client, bucket: impl Into<String>) -> Self {
        Self { client, bucket: bucket.into(), encryption: None }
    }

    /// Attach an encryption engine + user id to make upload/download transparent. With this
    /// set:
    ///
    /// - [`Bucket::upload`] encrypts plaintext bytes before POST (single-encrypts; re-uploading
    ///   already-encrypted bytes will double-encrypt — callers handing us raw bytes are
    ///   expected to give us plaintext).
    /// - [`Bucket::download`] decrypts payloads that begin with [`BINARY_ENCRYPTION_HEADER`].
    ///   Payloads without the header are returned unchanged (handles legacy plaintext objects
    ///   gracefully).
    pub fn with_encryption(mut self, encryption: Encryption, user_id: Uuid) -> Self {
        self.encryption = Some((encryption, user_id));
        self
    }

    /// The bucket name.
    pub fn name(&self) -> &str {
        &self.bucket
    }

    /// True when this bucket has an encryption engine attached.
    pub fn is_encrypted(&self) -> bool {
        self.encryption.is_some()
    }

    /// Upload bytes to a path within this bucket. `upsert = true` mirrors Prism's behavior
    /// of allowing reuploads; pass `false` to fail on conflict.
    ///
    /// If [`Bucket::with_encryption`] was called, the bytes are encrypted before upload.
    /// Otherwise they are sent raw.
    pub async fn upload(
        &self,
        path: &str,
        bytes: Bytes,
        content_type: &str,
        access_token: &str,
        upsert: bool,
    ) -> Result<(), PolyError> {
        let body = self.maybe_encrypt(path, bytes)?;
        let url = self.client.storage_object_url(&self.bucket, path);
        let resp = self
            .client
            .http()
            .post(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", content_type)
            .header("x-upsert", if upsert { "true" } else { "false" })
            .body(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(PolyError::Storage(StorageError::Failed {
                bucket: self.bucket.clone(),
                path: path.into(),
                message: format!("HTTP {}: {message}", status.as_u16()),
            }));
        }
        Ok(())
    }

    /// Download bytes from a path. Returns `StorageError::NotFound` for HTTP 404.
    ///
    /// If [`Bucket::with_encryption`] was called and the payload starts with
    /// [`BINARY_ENCRYPTION_HEADER`] (`ENC\0`), the bytes are decrypted before being returned.
    /// Payloads without the header (and downloads with no encryption attached) are returned
    /// unchanged.
    pub async fn download(&self, path: &str, access_token: &str) -> Result<Bytes, PolyError> {
        let url = self.client.storage_object_url(&self.bucket, path);
        let resp = self
            .client
            .http()
            .get(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PolyError::Storage(StorageError::NotFound {
                bucket: self.bucket.clone(),
                path: path.into(),
            }));
        }
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(PolyError::Storage(StorageError::Failed {
                bucket: self.bucket.clone(),
                path: path.into(),
                message: format!("HTTP {}: {message}", status.as_u16()),
            }));
        }
        let bytes = resp.bytes().await?;
        self.maybe_decrypt(path, bytes)
    }

    /// Encrypt `bytes` if an encryption engine is attached; otherwise pass through unchanged.
    fn maybe_encrypt(&self, path: &str, bytes: Bytes) -> Result<Bytes, PolyError> {
        let Some((enc, user)) = &self.encryption else {
            return Ok(bytes);
        };
        let ciphertext = enc.encrypt_data(&bytes, *user).map_err(|e| {
            PolyError::Storage(StorageError::Failed {
                bucket: self.bucket.clone(),
                path: path.into(),
                message: format!("encrypt: {e}"),
            })
        })?;
        Ok(Bytes::from(ciphertext))
    }

    /// Decrypt `bytes` if an encryption engine is attached AND the payload carries the
    /// `ENC\0` magic header. Otherwise pass through unchanged so legacy plaintext objects
    /// and non-encrypted buckets keep working.
    fn maybe_decrypt(&self, path: &str, bytes: Bytes) -> Result<Bytes, PolyError> {
        let Some((enc, user)) = &self.encryption else {
            return Ok(bytes);
        };
        if !enc.is_data_encrypted(&bytes) {
            return Ok(bytes);
        }
        let plaintext = enc.decrypt_data(&bytes, *user).map_err(|e| {
            PolyError::Storage(StorageError::Failed {
                bucket: self.bucket.clone(),
                path: path.into(),
                message: format!("decrypt: {e}"),
            })
        })?;
        Ok(Bytes::from(plaintext))
    }

    /// Delete a single object.
    pub async fn delete(&self, path: &str, access_token: &str) -> Result<(), PolyError> {
        let url = self.client.storage_object_url(&self.bucket, path);
        let resp = self
            .client
            .http()
            .delete(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() && status != reqwest::StatusCode::NOT_FOUND {
            let message = resp.text().await.unwrap_or_default();
            return Err(PolyError::Storage(StorageError::Failed {
                bucket: self.bucket.clone(),
                path: path.into(),
                message: format!("HTTP {}: {message}", status.as_u16()),
            }));
        }
        Ok(())
    }

    /// List objects under `prefix`. Results are paginated; large buckets need to call this in
    /// a loop with [`ListOptions::offset`].
    ///
    /// Hits `POST /storage/v1/object/list/{bucket}` per the Supabase Storage REST contract.
    pub async fn list(
        &self,
        prefix: &str,
        options: ListOptions,
        access_token: &str,
    ) -> Result<Vec<ObjectEntry>, PolyError> {
        let base = self.client.config().supabase_url.trim_end_matches('/');
        let url = format!("{base}/storage/v1/object/list/{}", self.bucket);

        let mut body = serde_json::Map::new();
        body.insert("prefix".into(), serde_json::Value::String(prefix.to_owned()));
        if let Some(limit) = options.limit {
            body.insert("limit".into(), serde_json::Value::from(limit));
        }
        if let Some(offset) = options.offset {
            body.insert("offset".into(), serde_json::Value::from(offset));
        }
        if let Some(search) = options.search.as_deref() {
            body.insert("search".into(), serde_json::Value::String(search.to_owned()));
        }
        if let Some(sort) = options.sort_by {
            body.insert("sortBy".into(), json!({ "column": sort.column, "order": sort.order }));
        }

        let resp = self
            .client
            .http()
            .post(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .json(&serde_json::Value::Object(body))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(PolyError::Storage(StorageError::Failed {
                bucket: self.bucket.clone(),
                path: prefix.into(),
                message: format!("HTTP {}: {message}", status.as_u16()),
            }));
        }

        let entries: Vec<ObjectEntry> = resp.json().await?;
        Ok(entries)
    }

    /// Mint a time-limited signed download URL for a private bucket object.
    ///
    /// `expires_in_seconds` is forwarded straight to Supabase Storage; values up to one week
    /// are commonly accepted but the upper bound is project-dependent.
    pub async fn create_signed_url(
        &self,
        path: &str,
        expires_in_seconds: u32,
        access_token: &str,
    ) -> Result<String, PolyError> {
        let base = self.client.config().supabase_url.trim_end_matches('/');
        let trimmed = path.trim_start_matches('/');
        let url = format!("{base}/storage/v1/object/sign/{}/{trimmed}", self.bucket);

        let resp = self
            .client
            .http()
            .post(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .json(&json!({ "expiresIn": expires_in_seconds }))
            .send()
            .await?;

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PolyError::Storage(StorageError::NotFound {
                bucket: self.bucket.clone(),
                path: path.into(),
            }));
        }
        if !status.is_success() {
            let message = resp.text().await.unwrap_or_default();
            return Err(PolyError::Storage(StorageError::Failed {
                bucket: self.bucket.clone(),
                path: path.into(),
                message: format!("HTTP {}: {message}", status.as_u16()),
            }));
        }

        let signed: SignedUrl = resp.json().await?;
        // Storage returns a relative path; resolve against the project base URL.
        let relative = signed.url.trim_start_matches('/');
        Ok(format!("{base}/storage/v1/{relative}"))
    }
}

#[cfg(test)]
mod tests {
    //! These tests exercise the encrypt-on-upload / decrypt-on-download transformations
    //! that [`Bucket::maybe_encrypt`] and [`Bucket::maybe_decrypt`] perform locally. The
    //! HTTP path itself is exercised end-to-end by the host app's smoke tests against a
    //! real Supabase project.
    use super::*;
    use crate::client::{Client, ClientConfig};

    fn client() -> Client {
        Client::new(ClientConfig {
            supabase_url: "https://example.supabase.co".into(),
            supabase_anon_key: "anon".into(),
            encryption_secret: None,
            storage_bucket: Some("test-bucket".into()),
        })
        .expect("client")
    }

    fn user() -> Uuid {
        Encryption::key_user_uuid("11111111-2222-3333-4444-555555555555")
    }

    fn enc() -> Encryption {
        Encryption::new("test-secret-1234").expect("enc")
    }

    #[test]
    fn upload_passthrough_when_no_encryption() {
        let bucket = Bucket::new(client(), "test-bucket");
        let raw = Bytes::from_static(b"hello world");
        let out = bucket.maybe_encrypt("a.bin", raw.clone()).expect("passthrough");
        assert_eq!(out, raw, "no encryption attached → bytes unchanged");
        assert!(!out.starts_with(b"ENC\0"));
    }

    #[test]
    fn upload_encrypts_when_encryption_attached() {
        let bucket = Bucket::new(client(), "test-bucket").with_encryption(enc(), user());
        let raw = Bytes::from_static(b"plaintext attachment");
        let cipher = bucket.maybe_encrypt("a.bin", raw.clone()).expect("encrypt");
        assert!(cipher.starts_with(b"ENC\0"), "encrypted upload must carry ENC\\0 header");
        assert_ne!(cipher, raw);
    }

    #[test]
    fn download_passthrough_when_no_encryption() {
        let bucket = Bucket::new(client(), "test-bucket");
        let cipher = enc().encrypt_data(b"payload", user()).expect("encrypt");
        let out = bucket.maybe_decrypt("a.bin", Bytes::from(cipher.clone())).expect("passthrough");
        assert_eq!(out, Bytes::from(cipher), "no encryption attached → bytes unchanged");
    }

    #[test]
    fn download_passthrough_when_payload_lacks_header() {
        let bucket = Bucket::new(client(), "test-bucket").with_encryption(enc(), user());
        let raw = Bytes::from_static(b"legacy plaintext object");
        let out = bucket.maybe_decrypt("a.bin", raw.clone()).expect("passthrough");
        assert_eq!(out, raw, "no ENC\\0 header → bytes unchanged");
    }

    #[test]
    fn download_decrypts_when_payload_has_header() {
        let bucket = Bucket::new(client(), "test-bucket").with_encryption(enc(), user());
        let plain = b"plaintext attachment";
        let cipher = enc().encrypt_data(plain, user()).expect("encrypt");
        let out = bucket.maybe_decrypt("a.bin", Bytes::from(cipher)).expect("decrypt");
        assert_eq!(&out[..], plain);
    }

    #[test]
    fn round_trip_via_helpers() {
        let bucket = Bucket::new(client(), "test-bucket").with_encryption(enc(), user());
        let plain = Bytes::from_static(b"\x00\x01\x02 binary safe");
        let cipher = bucket.maybe_encrypt("a.bin", plain.clone()).expect("encrypt");
        assert!(cipher.starts_with(b"ENC\0"));
        let back = bucket.maybe_decrypt("a.bin", cipher).expect("decrypt");
        assert_eq!(back, plain, "encrypt-then-decrypt must round-trip exactly");
    }

    #[test]
    fn download_decrypt_failure_surfaces_storage_error() {
        // Bucket attached to a DIFFERENT user than the one that encrypted the bytes.
        let other_user = Encryption::key_user_uuid("22222222-2222-3333-4444-555555555555");
        let bucket = Bucket::new(client(), "test-bucket").with_encryption(enc(), other_user);
        let cipher = enc().encrypt_data(b"payload", user()).expect("encrypt");
        let err = bucket.maybe_decrypt("a.bin", Bytes::from(cipher)).expect_err("must fail");
        match err {
            PolyError::Storage(StorageError::Failed { message, .. }) => {
                assert!(message.contains("decrypt"), "error should mention decrypt: {message}");
            }
            other => panic!("expected StorageError::Failed, got {other:?}"),
        }
    }
}
