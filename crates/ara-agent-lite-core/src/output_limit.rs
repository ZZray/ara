//! Effective output-token limit of a Run (requirement 15).
//!
//! The limit is the minimum of the Host request, an optional Host ceiling,
//! the model's declared maximum and, without a Host ceiling, the builtin
//! 8192. DR-P2a-1 found that upstream OMP has no global 8192 cap, so 8192 is
//! a historical conservative value and a Host ceiling may raise it.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub const BUILTIN_OUTPUT_CEILING: u64 = 8192;
/// Existing summary request cap (`max_tokens.min(4096)`).
pub const SUMMARY_OUTPUT_CEILING: u64 = 4096;
/// DR-P2a-1: OMP v18.1.8 (596f2da) and b91db66 take the model's declared
/// maxTokens with 64000 provider clamps; nothing requires 8192.
pub const BUILTIN_CEILING_IS_HARD: bool = false;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputLimitSource {
    Request,
    HostPolicy,
    ModelMaxTokens,
    Builtin8192,
    SummaryCeiling,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveOutputLimit {
    pub tokens: u64,
    /// Every input equal to `tokens`, in enum order.
    pub sources: Vec<OutputLimitSource>,
}

fn minimum(inputs: Vec<(OutputLimitSource, u64)>) -> EffectiveOutputLimit {
    let tokens = inputs.iter().map(|(_, value)| *value).min().unwrap_or(0);
    let mut sources: Vec<_> =
        inputs.into_iter().filter(|(_, value)| *value == tokens).map(|(source, _)| source).collect();
    sources.sort();
    sources.dedup();
    EffectiveOutputLimit { tokens, sources }
}

/// Effective agent-call limit. A Host ceiling of 0 is refused (15.7).
pub fn effective(requested: u32, host_ceiling: Option<u32>, model_max_tokens: u64) -> Result<EffectiveOutputLimit> {
    ensure!(host_ceiling != Some(0), "host output ceiling must be positive");
    let mut inputs =
        vec![(OutputLimitSource::Request, u64::from(requested)), (OutputLimitSource::ModelMaxTokens, model_max_tokens)];
    if let Some(ceiling) = host_ceiling {
        inputs.push((OutputLimitSource::HostPolicy, u64::from(ceiling)));
    }
    if host_ceiling.is_none() || BUILTIN_CEILING_IS_HARD {
        inputs.push((OutputLimitSource::Builtin8192, BUILTIN_OUTPUT_CEILING));
    }
    Ok(minimum(inputs))
}

/// Limit of a compaction summary call derived from the agent limit.
pub fn for_summary(agent: &EffectiveOutputLimit) -> EffectiveOutputLimit {
    let mut inputs: Vec<_> = agent.sources.iter().map(|source| (*source, agent.tokens)).collect();
    inputs.push((OutputLimitSource::SummaryCeiling, SUMMARY_OUTPUT_CEILING));
    minimum(inputs)
}

/// Upper bound for a JSONL start-frame `max_output` (15.5).
pub fn admission_bound(host_ceiling: Option<u32>) -> u64 {
    match host_ceiling {
        Some(ceiling) if BUILTIN_CEILING_IS_HARD => u64::from(ceiling).min(BUILTIN_OUTPUT_CEILING),
        Some(ceiling) => u64::from(ceiling),
        None => BUILTIN_OUTPUT_CEILING,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OutputLimitSource::*;

    #[test]
    fn builtin_applies_only_without_a_host_ceiling() {
        assert_eq!(
            effective(32768, None, 65536).unwrap(),
            EffectiveOutputLimit { tokens: 8192, sources: vec![Builtin8192] }
        );
        assert_eq!(
            effective(32768, Some(16384), 65536).unwrap(),
            EffectiveOutputLimit { tokens: 16384, sources: vec![HostPolicy] }
        );
        assert_eq!(
            effective(4096, Some(16384), 8192).unwrap(),
            EffectiveOutputLimit { tokens: 4096, sources: vec![Request] }
        );
        assert_eq!(
            effective(8192, None, 8192).unwrap(),
            EffectiveOutputLimit { tokens: 8192, sources: vec![Request, ModelMaxTokens, Builtin8192] }
        );
        assert!(effective(4096, Some(0), 8192).is_err());
    }

    #[test]
    fn summary_and_admission_bounds() {
        let agent = effective(32768, Some(16384), 65536).unwrap();
        assert_eq!(for_summary(&agent), EffectiveOutputLimit { tokens: 4096, sources: vec![SummaryCeiling] });
        let small = effective(2048, None, 8192).unwrap();
        assert_eq!(for_summary(&small), EffectiveOutputLimit { tokens: 2048, sources: vec![Request] });
        assert_eq!(admission_bound(None), 8192);
        assert_eq!(admission_bound(Some(32768)), 32768);
    }
}
