//! Supabase Storage REST adapter.
//!
//! This module is a thin wrapper over the `/storage/v1/object/` REST surface. It consumes a
//! [`Client`] and the active session's access token so authorization headers are unified across
//! polybase subsystems.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::Client;
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
#[derive(Debug, Clone)]
pub struct Bucket {
    client: Client,
    bucket: String,
}

impl Bucket {
    /// Build an adapter for the named bucket on the given client.
    pub fn new(client: Client, bucket: impl Into<String>) -> Self {
        Self { client, bucket: bucket.into() }
    }

    /// The bucket name.
    pub fn name(&self) -> &str {
        &self.bucket
    }

    /// Upload bytes to a path within this bucket. `upsert = true` mirrors Tauri Prism's behavior
    /// of allowing reuploads; pass `false` to fail on conflict.
    pub async fn upload(
        &self,
        path: &str,
        bytes: Bytes,
        content_type: &str,
        access_token: &str,
        upsert: bool,
    ) -> Result<(), PolyError> {
        let url = self.client.storage_object_url(&self.bucket, path);
        let resp = self
            .client
            .http()
            .post(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", content_type)
            .header("x-upsert", if upsert { "true" } else { "false" })
            .body(bytes)
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
        Ok(bytes)
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
