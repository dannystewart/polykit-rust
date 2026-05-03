//! Pull pipeline: bulk read + version probe + merge against local store.
//!
//! The pull layer is intentionally thin: read JSON rows, hand them to a registered factory,
//! and let the reconcile layer decide what to do per row.
//!
//! Crate-internal until exercised by [`crate::sync::Coordinator`] reconciliation flows.

use serde_json::Value;

use crate::client::Client;
use crate::errors::{PolyError, PullError};

/// Pulls rows out of Supabase via PostgREST.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct Puller {
    client: Client,
}

#[allow(dead_code)]
impl Puller {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Read all columns for the given ids on a table. Empty `ids` returns an empty vec.
    pub(crate) async fn read_by_ids(
        &self,
        table: &str,
        ids: &[String],
        access_token: &str,
    ) -> Result<Vec<Value>, PolyError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let in_clause = format!("in.({})", ids.join(","));
        let url = self.client.rest_url(table);
        let resp = self
            .client
            .http()
            .get(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .query(&[("id", in_clause.as_str()), ("select", "*")])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(PolyError::Pull(PullError::Failed {
                table: table.into(),
                message: format!("HTTP {}: {body}", status.as_u16()),
            }));
        }
        let value: Value = resp.json().await?;
        Ok(value.as_array().cloned().unwrap_or_default())
    }

    /// Read just `(id, version, deleted)` for reconcile planning.
    pub(crate) async fn read_versions(
        &self,
        table: &str,
        access_token: &str,
    ) -> Result<Vec<Value>, PolyError> {
        let url = self.client.rest_url(table);
        let resp = self
            .client
            .http()
            .get(url)
            .header("apikey", &self.client.config().supabase_anon_key)
            .header("Authorization", format!("Bearer {access_token}"))
            .query(&[("select", "id,version,deleted")])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(PolyError::Pull(PullError::Failed {
                table: table.into(),
                message: format!("HTTP {}: {body}", status.as_u16()),
            }));
        }
        let value: Value = resp.json().await?;
        Ok(value.as_array().cloned().unwrap_or_default())
    }
}
