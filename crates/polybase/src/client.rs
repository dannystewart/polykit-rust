//! Configured Supabase HTTP client.
//!
//! The [`Client`] is the single source of truth for the Supabase URL, anon key, optional storage
//! bucket, and an underlying [`reqwest::Client`]. Subsystems (auth refresh, edge calls, PostgREST
//! pushes/pulls, storage) all consume an `Arc<Client>` rather than reading globals.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::errors::PolyError;

/// Static configuration for the Supabase project this client talks to.
///
/// `encryption_secret` is optional here — encryption can also be built separately via
/// [`crate::encryption::Encryption::new`]. Storing it on the config makes single-shot
/// `Client::configure_full(...)` ergonomic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// e.g. `https://abcdefg.supabase.co`
    pub supabase_url: String,
    /// Public anon key.
    pub supabase_anon_key: String,
    /// Optional encryption secret (HKDF base material).
    #[serde(default)]
    pub encryption_secret: Option<String>,
    /// Optional default Storage bucket name.
    #[serde(default)]
    pub storage_bucket: Option<String>,
}

impl ClientConfig {
    /// Validate the URL/anon-key combo for non-emptiness.
    pub fn validate(&self) -> Result<(), PolyError> {
        if self.supabase_url.trim().is_empty() {
            return Err(PolyError::NotConfigured("supabase_url is empty".into()));
        }
        if self.supabase_anon_key.trim().is_empty() {
            return Err(PolyError::NotConfigured("supabase_anon_key is empty".into()));
        }
        Ok(())
    }

    /// Default Storage bucket name; fallback `attachments` matches Tauri Prism convention.
    pub fn storage_bucket_or_default(&self) -> &str {
        self.storage_bucket.as_deref().unwrap_or("attachments")
    }
}

/// Shared, cheap-to-clone Supabase client.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

#[derive(Debug)]
struct ClientInner {
    config: ClientConfig,
    http: reqwest::Client,
}

impl Client {
    /// Build a new client with a fresh internal `reqwest::Client`.
    pub fn new(config: ClientConfig) -> Result<Self, PolyError> {
        config.validate()?;
        let http = reqwest::Client::builder().build().map_err(PolyError::from)?;
        Ok(Self { inner: Arc::new(ClientInner { config, http }) })
    }

    /// Build a client wrapping a caller-supplied `reqwest::Client` (useful for shared connection
    /// pools or for injecting a test client).
    pub fn with_http(config: ClientConfig, http: reqwest::Client) -> Result<Self, PolyError> {
        config.validate()?;
        Ok(Self { inner: Arc::new(ClientInner { config, http }) })
    }

    /// Borrow the static configuration for this client.
    pub fn config(&self) -> &ClientConfig {
        &self.inner.config
    }

    /// Borrow the underlying `reqwest::Client` for callers that need to make raw HTTP requests
    /// against Supabase outside the typed wrappers (auth refresh, custom Storage operations).
    pub fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    /// Build a fully-qualified PostgREST table URL: `{base}/rest/v1/{table}`.
    pub fn rest_url(&self, table: &str) -> String {
        let base = self.inner.config.supabase_url.trim_end_matches('/');
        format!("{base}/rest/v1/{table}")
    }

    /// Build a fully-qualified Edge Function URL: `{base}/functions/v1/{function}`.
    pub fn functions_url(&self, function: &str) -> String {
        let base = self.inner.config.supabase_url.trim_end_matches('/');
        format!("{base}/functions/v1/{function}")
    }

    /// Build a fully-qualified Storage URL: `{base}/storage/v1/object/{bucket}/{path}`.
    pub fn storage_object_url(&self, bucket: &str, path: &str) -> String {
        let base = self.inner.config.supabase_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        format!("{base}/storage/v1/object/{bucket}/{path}")
    }

    /// Build a fully-qualified Auth REST URL: `{base}/auth/v1/{path}`.
    pub fn auth_url(&self, path: &str) -> String {
        let base = self.inner.config.supabase_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        format!("{base}/auth/v1/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ClientConfig {
        ClientConfig {
            supabase_url: "https://example.supabase.co/".into(),
            supabase_anon_key: "anon".into(),
            encryption_secret: None,
            storage_bucket: None,
        }
    }

    #[test]
    fn url_helpers_strip_trailing_slash() {
        let c = Client::new(cfg()).unwrap();
        assert_eq!(c.rest_url("messages"), "https://example.supabase.co/rest/v1/messages");
        assert_eq!(
            c.functions_url("messages-write"),
            "https://example.supabase.co/functions/v1/messages-write"
        );
        assert_eq!(
            c.storage_object_url("attachments", "/u/1/a.png"),
            "https://example.supabase.co/storage/v1/object/attachments/u/1/a.png"
        );
        assert_eq!(c.auth_url("token"), "https://example.supabase.co/auth/v1/token");
    }

    #[test]
    fn empty_url_rejected() {
        let mut bad = cfg();
        bad.supabase_url.clear();
        assert!(matches!(Client::new(bad), Err(PolyError::NotConfigured(_))));
    }

    #[test]
    fn empty_anon_rejected() {
        let mut bad = cfg();
        bad.supabase_anon_key.clear();
        assert!(matches!(Client::new(bad), Err(PolyError::NotConfigured(_))));
    }

    #[test]
    fn storage_bucket_default_is_attachments() {
        let c = Client::new(cfg()).unwrap();
        assert_eq!(c.config().storage_bucket_or_default(), "attachments");
    }
}
