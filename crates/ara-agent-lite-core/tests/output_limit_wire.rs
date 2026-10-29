//! Property 21 (requirements 15.2, 15.3, 15.4): the effective output limit is
//! the minimum of the participating inputs, its sources are exactly the
//! inputs equal to it, and the controlled provider receives that value as
//! `max_tokens` (a compaction summary receives `min(effective, 4096)`).
mod support;

use std::sync::Arc;

use anyhow::Result;
use ara_agent_lite_core::{
    model::{CatalogModelHost, ModelHost, ModelSelection, Route},
    output_limit::{
        admission_bound, effective, for_summary, EffectiveOutputLimit, OutputLimitSource, BUILTIN_CEILING_IS_HARD,
        BUILTIN_OUTPUT_CEILING, SUMMARY_OUTPUT_CEILING,
    },
};
use async_trait::async_trait;
use proptest::prelude::*;
use serde_json::{json, Value};
use support::*;

/// Reference model of `effective` from the property text.
fn expected(requested: u32, host: Option<u32>, model: u64) -> EffectiveOutputLimit {
    let mut inputs =
        vec![(OutputLimitSource::Request, u64::from(requested)), (OutputLimitSource::ModelMaxTokens, model)];
    if let Some(ceiling) = host {
        inputs.push((OutputLimitSource::HostPolicy, u64::from(ceiling)));
    }
    if host.is_none() || BUILTIN_CEILING_IS_HARD {
        inputs.push((OutputLimitSource::Builtin8192, BUILTIN_OUTPUT_CEILING));
    }
    let tokens = inputs.iter().map(|(_, value)| *value).min().unwrap();
    let mut sources: Vec<_> = inputs.iter().filter(|(_, value)| *value == tokens).map(|(source, _)| *source).collect();
    sources.sort();
    EffectiveOutputLimit { tokens, sources }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn effective_limit_is_the_minimum_of_participating_inputs(
        requested in prop_oneof![1u32..=16384, Just(8192u32), Just(4096u32)],
        host in prop::option::of(prop_oneof![1u32..=65536, Just(8192u32), Just(4096u32)]),
        model in prop_oneof![1u64..=65536, Just(8192u64), Just(4096u64)],
    ) {
        let limit = effective(requested, host, model).unwrap();
        prop_assert_eq!(&limit, &expected(requested, host, model));
        if host.is_none() {
            prop_assert_eq!(limit.tokens, u64::from(requested).min(BUILTIN_OUTPUT_CEILING).min(model));
        }
        // The summary call adds only the 4096 summary ceiling.
        let summary = for_summary(&limit);
        prop_assert_eq!(summary.tokens, limit.tokens.min(SUMMARY_OUTPUT_CEILING));
        let mut sources = Vec::new();
        if limit.tokens <= SUMMARY_OUTPUT_CEILING {
            sources.extend(limit.sources.iter().copied());
        }
        if limit.tokens >= SUMMARY_OUTPUT_CEILING {
            sources.push(OutputLimitSource::SummaryCeiling);
        }
        sources.sort();
        prop_assert_eq!(summary.sources, sources);
        prop_assert_eq!(admission_bound(host), host.map_or(BUILTIN_OUTPUT_CEILING, u64::from));
        prop_assert!(effective(requested, Some(0), model).is_err());
    }
}

/// Catalog-resolving Host with a fixed output ceiling.
struct CeilingHost(Option<u32>);

#[async_trait]
impl ModelHost for CeilingHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        CatalogModelHost.resolve(route, endpoint, token, run).await
    }
    fn output_ceiling(&self) -> Result<Option<u32>> {
        Ok(self.0)
    }
}

fn sources_json(limit: &EffectiveOutputLimit) -> Value {
    json!({"effective": limit.tokens, "sources": limit.sources})
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn provider_receives_the_recorded_effective_limit(
        requested in 1u32..=20000,
        host in prop::option::of(1u32..=20000),
        model in 1000u64..=20000,
    ) {
        let root = TempDir::new();
        let provider = Provider::with(ProviderConfig { max_tokens: model, ..ProviderConfig::new(vec![text("done")]) });
        let mut request = request(root.path(), &provider, &[], 2, "Report done.");
        request.max_output = requested;
        request.model_host = Arc::new(CeilingHost(host));
        let (result, events) = runtime().block_on(run(request));
        prop_assert_eq!(result.code, 0, "{}", result.error);
        let limit = expected(requested, host, model);
        let calls = of_type(&events, "model_call");
        prop_assert_eq!(calls.len(), 1);
        prop_assert_eq!(&calls[0]["output_limit"], &sources_json(&limit));
        let bodies = provider.bodies();
        prop_assert_eq!(bodies.len(), 1);
        prop_assert_eq!(bodies[0]["max_tokens"].as_u64(), Some(limit.tokens));
    }
}

/// Run 1 leaves ~16 KiB of history; Run 2 adds a ~16 KiB objective, so its
/// first turn exceeds the 30 000-byte projection and must summarize Run 1.
const MAX_OUTPUT: u32 = 6000;
const CONTEXT_WINDOW: u64 = 60_000 + MAX_OUTPUT as u64 + 4096;

#[tokio::test]
async fn summary_call_carries_the_summary_limit_and_agent_call_the_effective_limit() {
    let root = TempDir::new();
    let provider = Provider::with(ProviderConfig {
        context_window: CONTEXT_WINDOW,
        max_tokens: 8192,
        ..ProviderConfig::new(vec![text(&"a".repeat(16 * 1024)), text("done")])
    });
    let mut first = request(root.path(), &provider, &[], 1, "Write the long report.");
    first.max_output = MAX_OUTPUT;
    let (first, _) = run(first).await;
    assert_eq!(first.code, 0, "{}", first.error);
    let objective = format!("Continue. {}", "q".repeat(16 * 1024));
    let mut second = request(root.path(), &provider, &[], 3, &objective);
    second.max_output = MAX_OUTPUT;
    let (second, events) = run(second).await;
    assert_eq!(second.code, 0, "{}", second.error);
    let calls = of_type(&events, "model_call");
    let phases: Vec<_> = calls.iter().map(|call| call["phase"].as_str().unwrap()).collect();
    assert_eq!(phases, ["summary", "agent"]);
    assert_eq!(calls[0]["output_limit"], json!({"effective":4096,"sources":["summary_ceiling"]}));
    assert_eq!(calls[1]["output_limit"], json!({"effective":MAX_OUTPUT,"sources":["request"]}));
    let bodies = provider.bodies();
    assert_eq!(bodies.len(), 3);
    assert!(bodies[1].to_string().contains(SUMMARY_PROMPT));
    assert_eq!(bodies[1]["max_tokens"], 4096);
    assert_eq!(bodies[2]["max_tokens"], MAX_OUTPUT);
}
