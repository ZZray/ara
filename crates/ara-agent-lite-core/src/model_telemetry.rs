//! Conservative observations from the pinned provider adapter, never billing.
use rpi_ai::{Context, Usage};
use serde_json::{json, Value};

/// The adapter defaults absent usage fields to zero. Only positive counts can
/// therefore be reported; zero remains unknown, not proof of a cache miss.
pub fn usage_metrics(usage: &Usage) -> Value {
    let values = [usage.input, usage.output, usage.cache_read, usage.cache_write];
    let sum = values.iter().try_fold(0i64, |n, value| n.checked_add(*value));
    if values.iter().any(|v| *v < 0) || sum != Some(usage.total_tokens) || usage.total_tokens <= 0 {
        return json!({"status":"unknown","source":"provider_adapter","cost":null});
    }
    let positive = |n: i64| (n > 0).then_some(n);
    json!({
        "status":"adapter_reported_positive_counts",
        "source":"provider_adapter_not_raw_presence_or_billing",
        "input_uncached_tokens":positive(usage.input),
        "output_tokens":positive(usage.output),
        "cache_read_tokens":positive(usage.cache_read),
        "cache_write_tokens":positive(usage.cache_write),
        "adapter_total_tokens":usage.total_tokens,
        "cost":null,
        "zero_fields":"unknown_or_reported_zero_not_distinguishable"
    })
}

pub fn context_observation(context: &Context) -> Value {
    let system = context.system_prompt.as_deref().unwrap_or("");
    json!({"system_utf8_bytes":system.len(),"system_sha256":crate::context_memory::hash(system.as_bytes()),
        "projection_messages":context.messages.len(),"unit":"utf8_bytes_not_tokens",
        "cache_policy":"provider_managed_no_hit_guarantee"})
}

/// Same serialized allocation used by TextBudget::check, not provider token usage.
pub fn request_observation(context: &Context, budget: crate::model_caps::TextBudget) -> anyhow::Result<Value> {
    let mut value = context_observation(context);
    value["request_json_bytes"] = json!(budget.request_json_bytes(context)?);
    value["request_limit_bytes"] = json!(budget.request_bytes);
    value["budget_source"] = json!("host_conservative_text_allocation");
    Ok(value)
}

pub fn projection_observation(bytes: usize, limit: usize, status: &str) -> Value {
    json!({"type":"context_status", "status":status, "projection_json_bytes":bytes,
        "projection_limit_bytes":limit, "unit":"utf8_json_bytes_not_tokens",
        "near_threshold":bytes >= limit.saturating_sub(limit / 5),
        "source":"host_working_projection"})
}

pub fn compaction_receipt(raw_base: usize, cut: usize, window_messages: usize) -> anyhow::Result<Value> {
    anyhow::ensure!(cut <= window_messages, "compaction cut exceeds retained window");
    let covered = raw_base.checked_add(cut).ok_or_else(|| anyhow::anyhow!("compaction prefix overflow"))?;
    let count = raw_base.checked_add(window_messages).ok_or_else(|| anyhow::anyhow!("raw count overflow"))?;
    Ok(json!({"type":"compact","status":"completed","title":"已压缩工作上下文；原始历史保留",
        "covered_prefix":covered,"raw_messages":count,"index_unit":"global_raw_message_indexes","summary_model_calls":1}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_or_zero_usage_is_unknown_not_free_or_cache_miss() {
        let result = usage_metrics(&Usage::zero());
        assert_eq!(result["status"], "unknown");
        assert!(result["cost"].is_null());
        assert!(result.get("cache_read_tokens").is_none());
    }
    #[test]
    fn positive_adapter_cache_counts_are_sourced_without_zero_inference() {
        let mut usage = Usage::zero();
        usage.input = 300;
        usage.output = 20;
        usage.cache_read = 700;
        usage.total_tokens = 1020;
        let result = usage_metrics(&usage);
        assert_eq!(result["cache_read_tokens"], 700);
        assert_eq!(result["input_uncached_tokens"], 300);
        assert!(result["cache_write_tokens"].is_null());
        assert!(result["cost"].is_null());
    }
    #[test]
    fn invalid_negative_inconsistent_and_overflow_usage_are_unknown() {
        let mut usage = Usage::zero();
        usage.input = -1;
        usage.output = 2;
        usage.total_tokens = 1;
        assert_eq!(usage_metrics(&usage)["status"], "unknown");
        usage.input = 10;
        usage.total_tokens = 999;
        assert_eq!(usage_metrics(&usage)["status"], "unknown");
        usage.input = i64::MAX;
        usage.output = 1;
        usage.total_tokens = i64::MAX;
        assert_eq!(usage_metrics(&usage)["status"], "unknown");
    }
    #[test]
    fn compaction_receipt_uses_global_journal_indexes_after_hot_eviction() {
        let receipt = compaction_receipt(1000, 40, 100).unwrap();
        assert_eq!(receipt["covered_prefix"], 1040);
        assert_eq!(receipt["raw_messages"], 1100);
        assert!(compaction_receipt(0, 101, 100).is_err());
        assert!(compaction_receipt(usize::MAX, 1, 1).is_err());
    }
    #[test]
    fn observations_share_request_budget_and_do_not_invent_token_usage() {
        let budget = crate::model_caps::TextBudget { projection_bytes: 512, request_bytes: 1024 };
        let context = Context { system_prompt: Some("中\\\"".repeat(5)), ..Default::default() };
        let value = request_observation(&context, budget).unwrap();
        assert_eq!(value["request_json_bytes"], serde_json::to_vec(&context).unwrap().len());
        assert_eq!(value["request_limit_bytes"], 1024);
        assert!(value.get("tokens").is_none());
        assert_eq!(projection_observation(799, 1000, "ready")["near_threshold"], false);
        assert_eq!(projection_observation(800, 1000, "ready")["near_threshold"], true);
    }
    #[test]
    fn context_observation_is_content_free_and_stable() {
        let context = Context { system_prompt: Some("stable rules".into()), ..Default::default() };
        let a = context_observation(&context);
        let mut b = context.clone();
        if let rpi_agent::AgentMessage::User(user) = crate::context_memory::user("changing run state".into()) {
            b.messages.push(rpi_ai::Message::User(user));
        }
        assert_eq!(a["system_sha256"], context_observation(&b)["system_sha256"]);
        assert!(!a.to_string().contains("stable rules"));
    }
}
