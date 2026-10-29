//! Requirement 15.7: a Host output ceiling of 0 is an invalid output-limit
//! policy. The Run is refused before any upstream HTTP request (including
//! the catalog lookup) and never falls back to the builtin 8192.
mod support;

use std::sync::Arc;

use anyhow::Result;
use ara_agent_lite_core::{
    execute,
    model::{CatalogModelHost, ModelHost, ModelSelection, Route},
};
use async_trait::async_trait;
use support::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct ZeroCeiling;

#[async_trait]
impl ModelHost for ZeroCeiling {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        CatalogModelHost.resolve(route, endpoint, token, run).await
    }
    fn output_ceiling(&self) -> Result<Option<u32>> {
        Ok(Some(0))
    }
}

#[tokio::test]
async fn zero_host_ceiling_is_refused_before_any_http_request() {
    let root = TempDir::new();
    let provider = Provider::new(vec![text("must not be requested")]);
    let mut request = request(root.path(), &provider, &[], 2, "Report done.");
    request.max_output = 8192;
    request.model_host = Arc::new(ZeroCeiling);
    let (sender, mut receiver) = mpsc::channel(16);
    let error = execute(request, CancellationToken::new(), sender).await.err().expect("refused");
    assert!(error.to_string().contains("output ceiling must be positive"), "{error}");
    assert_eq!(provider.request_count(), 0, "no chat request");
    assert_eq!(provider.catalog_requests.load(std::sync::atomic::Ordering::SeqCst), 0, "no catalog request");
    assert!(receiver.try_recv().is_err(), "no event, so no 8192 fallback was recorded");
    assert!(!root.path().join("session.json").exists(), "Session untouched");
}
