//! Exact fixed OMP regex source literals; MIT, see THIRD_PARTY_NOTICES.md.
use super::*;
pub(super) const REGEXPS: &[(&str, &str, &str)] = &[
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_0", r###"prompt is too long"###, "i"), // flags.ts:80
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_1", r###"input is too long for requested model"###, "i"), // flags.ts:81
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_2", r###"exceeds the context window"###, "i"), // flags.ts:82
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_3", r###"input token count.*exceeds the maximum"###, "i"), // flags.ts:83
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_4", r###"maximum prompt length is \d+"###, "i"), // flags.ts:84
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_5", r###"reduce the length of the messages"###, "i"), // flags.ts:85
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_6", r###"maximum context length is \d+ tokens"###, "i"), // flags.ts:86
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_7", r###"exceeds the available context size"###, "i"), // flags.ts:87
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_8", r###"requested tokens?.*exceed.*context (window|length|size)"###, "i"), // flags.ts:88
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_9", r###"context (window|length|size).*(exceeded|overflow|too small)"###, "i"), // flags.ts:89
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_10", r###"(prompt|input).*(too long|too large).*(context|n_ctx)"###, "i"), // flags.ts:90
    (
        "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_11",
        r###"requested tokens?.*(exceeds?|greater than).*(n_ctx|context)"###,
        "i",
    ), // flags.ts:91
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_12", r###"greater than the context length"###, "i"), // flags.ts:92
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_13", r###"context window exceeds limit"###, "i"),    // flags.ts:93
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_14", r###"exceeded model token limit"###, "i"),      // flags.ts:94
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_15", r###"context[_ ]length[_ ]exceeded"###, "i"),   // flags.ts:95
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_16", r###"too many tokens"###, "i"),                 // flags.ts:96
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_17", r###"token limit exceeded"###, "i"),            // flags.ts:97
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_18", r###"request_too_large[^\n]*\btokens?\b"###, "i"), // flags.ts:98
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_19", r###"\btokens?\b[^\n]*request_too_large"###, "i"), // flags.ts:99
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_20", r###"model_context_window_exceeded"###, "i"),   // flags.ts:100
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_21", r###"prompt filled the context window"###, "i"), // flags.ts:101
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_22", r###"exceeds the limit of \d+ tokens?\b"###, "i"), // flags.ts:102
    ("CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_23", r###"chat history exceeds the \d+-message limit"###, "i"), // flags.ts:103
    ("GENERIC_LIMIT_OVERFLOW_PATTERN", r###"exceeds the limit of \d+"###, "i"),               // flags.ts:106
    ("OVERFLOW_NO_BODY_PATTERN", r###"\b4(00|13)\s*(status code)?\s*\(no body\)"###, "i"),    // flags.ts:138
    ("PAYLOAD_REJECTION_PATTERNS_0", r###"\b413\s*(?:status code\s*)?\(no body\)"###, "i"),   // flags.ts:141
    (
        "PAYLOAD_REJECTION_PATTERNS_1",
        r###"\b413\b[^.\n]{0,120}\b(?:request|payload|entity|body)\b[^.\n]{0,60}\b(?:exceed|too large|limit)"###,
        "i",
    ), // flags.ts:142
    ("PAYLOAD_REJECTION_PATTERNS_2", r###"request_too_large"###, "i"),                        // flags.ts:143
    ("PAYLOAD_REJECTION_PATTERNS_3", r###"(?:payload|entity) too large"###, "i"),             // flags.ts:144
    ("PAYLOAD_REJECTION_PATTERNS_4", r###"request exceeds the maximum (?:size|number of bytes)"###, "i"), // flags.ts:145
    ("TIMEOUT_PATTERN", r###"\b(?:operation\s+)?timed?\s*out\b|\btimeout\b|\bstream stall\b"###, "i"), // flags.ts:155
    ("TRANSIENT_ENVELOPE_PATTERN", r###"anthropic stream envelope error:"###, "i"),                    // flags.ts:156
    ("TRANSIENT_ENVELOPE_TRUNCATION_PATTERN", r###"before message_(?:start|stop)"###, "i"),            // flags.ts:157
    ("STREAM_READ_ERROR_PATTERN", r###"stream[_ -]?read[_ -]?error"###, "i"),                          // flags.ts:158
    (
        "TRANSIENT_TRANSPORT_PATTERN",
        r###"\b(?:no[_ -]?capacity|(?:high|peak)[ _-]?demand|(?:at|over|insufficient)[ _-]?capacity|capacity[ _-]?(?:exceeded|exhausted)|peak[ _-]?load)\b|overloaded|provider.?returned.?error|rate.?limit|too many requests|\b(?:429|500|502|503|504)\b|service.?unavailable|server.?error|internal.?error|retry your request|network.?error|connection.?error|connection.?refused|unable.?to.?connect\.\s*is the computer able to access the url\?|other side closed|fetch failed|upstream.?connect|upstream.?request.?failed|reset before headers|socket hang up|timed? out|timeout|terminated|retry delay|stream stall|no error details in response|HTTP2(?:StreamReset|RefusedStream|EnhanceYourCalm)|nghttp2_(?:internal_error|refused_stream)|stream closed with error code nghttp2_(?:internal_error|refused_stream)|malformed.?function.?call"###,
        "i",
    ), // flags.ts:160
    (
        "AUTH_FAILURE_PATTERN",
        r###"\b(?:401|403|unauthorized|forbidden|authentication|auth[_ ]?unavailable|no auth available|(?:invalid|no)[_ ]?api[_ ]?key)\b"###,
        "i",
    ), // flags.ts:162
    ("MALFORMED_FUNCTION_CALL_PATTERN", r###"\bmalformed.?function.?call\b"###, "i"),                  // flags.ts:163
    (
        "PROVIDER_FINISH_ERROR_PATTERN",
        r###"\bProvider (?:returned error finish_reason|finish_reason:\s*error)\b"###,
        "i",
    ), // flags.ts:164
    ("EMPTY_RESPONSE_PATTERN", r###"\bthought-only response without final output\b"###, "i"),          // flags.ts:165
    ("CONTENT_FILTER_PATTERN", r###"\b(?:incomplete:\s*)?content_filter\b"###, "i"),                   // flags.ts:166
    ("ACCOUNT_POLICY_PATTERN", r###"\bcyber_policy\b|trusted access for cyber"###, "i"),               // flags.ts:167
    (
        "CODEX_CHATGPT_ACCOUNT_MODEL_POLICY_PATTERN",
        r###"\bThe ['"]([^'"\r\n]+)['"] model is not supported when using Codex with a ChatGPT account\."###,
        "i",
    ), // flags.ts:169
    ("CURSOR_PLAN_POLICY_MARKER_PATTERN", r###"\bERROR_RATE_LIMITED_CHANGEABLE\b"###, "i"),            // flags.ts:171
    (
        "CURSOR_PLAN_POLICY_PATTERN",
        r###"\bNamed models unavailable\b|\bModel unavailable on\b|\bFree plans can only use\b"###,
        "i",
    ), // flags.ts:172
    ("STALE_RESPONSE_ITEM_PATTERNS_0", r###"\bItem with id ['"][^'"]+['"] not found\.?"###, "i"),      // flags.ts:204
    ("STALE_RESPONSE_ITEM_PATTERNS_1", r###"previous[ _]?response"###, "i"),                           // flags.ts:204
    (
        "STALE_RESPONSE_ITEM_DETAIL_PATTERN",
        r###"not[ _]?found|invalid|expired|stale|zero[ _-]?data[ _-]?retention"###,
        "i",
    ), // flags.ts:205
    (
        "LLAMA_CPP_TOOL_CALL_PARSE_PATTERN",
        r###"failed to parse tool call arguments as json|\[json\.exception\.parse_error\.101\]"###,
        "i",
    ), // flags.ts:213
    ("GENERATION_NAN_PATTERN", r###"floating[ _-]?point nan\b.*\bdetected in generation"###, "is"),    // flags.ts:223
    ("GRAMMAR_TOO_LARGE_PATTERN", r###"compiled grammar"###, "i"),                                     // flags.ts:228
    ("GRAMMAR_TOO_LARGE_DETAIL_PATTERN", r###"too large"###, "i"),                                     // flags.ts:229
    ("SCHEMA_TOO_COMPLEX_PATTERN", r###"schema"###, "i"),                                              // flags.ts:230
    ("SCHEMA_TOO_COMPLEX_DETAIL_PATTERN", r###"too complex"###, "i"),                                  // flags.ts:231
    ("SCHEMA_COMPILE_PATTERN", r###"compil"###, "i"),                                                  // flags.ts:232
    ("INVALID_REQUEST_PATTERN", r###"invalid_request_error"###, "i"),                                  // flags.ts:233
    ("STRUCTURED_OUTPUTS_PATTERN", r###"structured[_ -]?outputs?"###, "i"),                            // flags.ts:234
    (
        "FEATURE_NOT_SUPPORTED_PATTERN",
        r###"not (?:supported|available|enabled)|unsupported|does(?: not|n'?t) support"###,
        "i",
    ), // flags.ts:235
    ("ANTHROPIC_STRICT_FIELD_PATTERN", r###"\btools\.\d+\.custom\.strict\b"###, "i"),                  // flags.ts:236
    ("EXTRA_INPUTS_NOT_PERMITTED_PATTERN", r###"extra inputs? (?:are|is) not permitted"###, "i"),      // flags.ts:237
    ("FAST_MODE_SPEED_PARAM_PATTERN", r###"\bspeed\b"###, "i"),                                        // flags.ts:240
    ("FAST_MODE_NOT_SUPPORTED_PATTERN", r###"not support"###, "i"),                                    // flags.ts:241
    ("FAST_MODE_RATE_LIMIT_PATTERN", r###"rate_limit_error"###, "i"),                                  // flags.ts:242
    ("FAST_MODE_ENTITLEMENT_PATTERN", r###"fast mode"###, "i"),                                        // flags.ts:243
    (
        "OAUTH_DEFINITIVE_FAILURE_PATTERN",
        r###"invalid_grant|invalid_token|unauthorized_client|\brevoked\b|refresh[\s_]?token.*expired"###,
        "i",
    ), // flags.ts:246
    (
        "OAUTH_TRANSIENT_FAILURE_PATTERN",
        r###"timeout|network|fetch failed|ECONN(?:REFUSED|RESET)|ETIMEDOUT|EAI_AGAIN|socket hang up|\b(?:408|425|429|5\d{2})\b|rate.?limit|too many requests|temporar|unavailable|forbidden|permission_denied|cloudflare|captcha"###,
        "i",
    ), // flags.ts:248
    ("OAUTH_HTTP_AUTH_PATTERN", r###"\b401\b"###, ""),                                                 // flags.ts:249
    ("STATUS_MESSAGE_PATTERNS_0", r###"\bstatus(?:_code)?[:=]\s*(\d{3})\b"###, "i"),                   // flags.ts:307
    ("STATUS_MESSAGE_PATTERNS_1", r###"\bstatus\s+(\d{3})\b"###, "i"),                                 // flags.ts:308
    ("STATUS_MESSAGE_PATTERNS_2", r###"\bHTTP\s+(\d{3})\b"###, "i"),                                   // flags.ts:309
    ("STATUS_MESSAGE_PATTERNS_3", r###"\b(?:error|failed)\s*[:=]?\s*(\d{3})\b"###, "i"),               // flags.ts:310
    ("STATUS_MESSAGE_PATTERNS_4", r###"(?:^|\s)(\d{3})\s+(?:[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)"###, ""),  // flags.ts:311
    ("CLINE_PASS_SURFACE_GATE_PATTERN", r###"only available via cline product surfaces"###, "i"),      // flags.ts:685
    (
        "STREAM_PARSE_TRUNCATION_PATTERN",
        r###"unterminated string|unexpected end of json input|unexpected end of data|unexpected eof|end of file|eof while parsing|truncated"###,
        "i",
    ), // flags.ts:792
    (
        "STREAM_PARSE_DIAGNOSTIC_PATTERN",
        r###"(?:json parse error:\s*(?:unterminated string|unexpected end of json input|unexpected end of data|unexpected eof|end of file|eof while parsing|truncated)|json\.parse:\s*(?:unterminated string|unexpected end of data)|unexpected end of json input|unexpected eof|eof while parsing)"###,
        "i",
    ), // flags.ts:794
    ("STREAM_EVENT_ORDER_PATTERN", r###"stream event order|before message_start"###, "i"),             // flags.ts:795
    (
        "ACCOUNT_RATE_LIMIT_PATTERN",
        r###"\baccount(?:'s)?\b[^\n]{0,80}\brate.?limit\b|\brate.?limit\b[^\n]{0,80}\baccount\b"###,
        "i",
    ), // rate-limit.ts:25
    ("INSUFFICIENT_BALANCE_PATTERN", r###"insufficient.?balance"###, "i"), // rate-limit.ts:26
    (
        "CREDITS_EXHAUSTED_PATTERN",
        r###"\b(?:exceed\w*|insufficient|not enough)\b[^\n]{0,40}\bcredits?\b|\bcredits?\b[^\n]{0,40}\b(?:exhausted|depleted)\b"###,
        "i",
    ), // rate-limit.ts:32
    ("SPEND_LIMIT_PATTERN", r###"spend.?limit"###, "i"),                   // rate-limit.ts:33
    (
        "SUBSCRIPTION_CAP_PATTERN",
        r###"\b(?:subscription|plan|membership)\b[^\n]{0,80}\b(?:rate.?limits?|quota|cap)\b|\b(?:rate.?limits?|quota|cap)\b[^\n]{0,80}\b(?:subscription|plan|membership)\b"###,
        "i",
    ), // rate-limit.ts:35
    ("TRANSIENT_INTERVAL_RATE_LIMIT_PATTERN", r###"\bper\s+(?:second|minute)\b"###, "i"), // rate-limit.ts:36
    ("OPENROUTER_DAILY_FREE_LIMIT_PATTERN", r###"\bfree[-_ ]models[-_ ]per[-_ ]day\b"###, "i"), // rate-limit.ts:41
    ("CLINE_PASS_QUOTA_PATTERN", r###"clinepass limit|free limit reached on model"###, "i"), // rate-limit.ts:45
    ("RESOURCE_EXHAUSTED_PATTERN", r###"resource.?exhausted"###, "gi"),    // rate-limit.ts:50
    (
        "CONCURRENT_LIMIT_PATTERN",
        r###"\btoo many\s+concurren\w*\s+(?:requests?|invocations?)\b|\bconcurren\w*\b[^\n]{0,60}\b(?:limit|quota|exceed\w*|reach\w*)\b|\b(?:limit|quota|exceed\w*|reach\w*)\b[^\n]{0,60}\bconcurren\w*\b|\bconcurren[a-z]*[-_](?:[a-z]+[_-])*(?:limit|quota|exceed\w*|reach\w*)"###,
        "i",
    ), // rate-limit.ts:55
    (
        "ACCOUNT_SCOPED_403_PATTERN",
        r###"\b(?:overall|account|organization|team|workspace)\b[^\n]{0,40}\b(?:message |request )?rate.?limit\b|\byour\b[^\n]{0,30}\b(?:limit )?will reset\b"###,
        "i",
    ), // rate-limit.ts:62
    (
        "CN_QUOTA_EXHAUSTED_PATTERN",
        r###"使用.{0,30}?上限|(?:额度|配额)已?(?:用|耗)(?:完|尽)|限额.{0,30}重置|余额不足"###,
        "",
    ), // rate-limit.ts:73
    (
        "CN_TRANSIENT_CAP_PATTERN",
        r###"速率.{0,30}上限|频率.{0,30}上限|每分钟.{0,30}上限|并发.{0,30}上限|使用.{0,30}(?:速率|频率|每分钟|并发).{0,30}上限"###,
        "",
    ), // rate-limit.ts:77
    ("CN_THROTTLE_PATTERN", r###"速率(?:限制|过快)|频率(?:过高|过快)|过于频繁|稍后[重再]试"###, ""), // rate-limit.ts:81
    ("DASHSCOPE_TOKEN_LIMIT_DOC_PATTERN", r###"error-code[^()\s]*#token-limit"###, "i"), // rate-limit.ts:93
    (
        "DASHSCOPE_TOKEN_LIMIT_MESSAGE_PATTERN",
        r###"\byou exceeded your current quota, please check your plan and billing details\b"###,
        "i",
    ), // rate-limit.ts:95
    ("ANTIGRAVITY_MODEL_QUOTA_PATTERN", r###"\bexhausted your capacity on this model\b"###, "i"), // rate-limit.ts:104
    (
        "USAGE_LIMIT_PATTERN",
        r###"usage.?limit|usage_limit_reached|usage_not_included|limit_reached|quota.?(?:exceeded|reached|insufficient)|额度不足|额度耗尽|resource.?exhausted|exhausted your capacity|quota will reset|insufficient.?(?:balance|quota)|balance.?exhausted|run out of credits|out of credits|spending[- _]?limit|personal-team-blocked|clinepass limit|free limit reached on model"###,
        "i",
    ), // rate-limit.ts:297
    (
        "STATUS_402_QUOTA_PATTERN",
        r###"\b(?:payment(?:\s+is)?[-_.\s]*required|deactivated_workspace|insufficient.?balance)\b"###,
        "i",
    ), // rate-limit.ts:313
];
pub(super) const CONTEXT_OVERFLOW_EVIDENCE_PATTERNS: &[&str] = &[
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_0",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_1",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_2",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_3",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_4",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_5",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_6",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_7",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_8",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_9",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_10",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_11",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_12",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_13",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_14",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_15",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_16",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_17",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_18",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_19",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_20",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_21",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_22",
    "CONTEXT_OVERFLOW_EVIDENCE_PATTERNS_23",
];
pub(super) const PAYLOAD_REJECTION_PATTERNS: &[&str] = &[
    "PAYLOAD_REJECTION_PATTERNS_0",
    "PAYLOAD_REJECTION_PATTERNS_1",
    "PAYLOAD_REJECTION_PATTERNS_2",
    "PAYLOAD_REJECTION_PATTERNS_3",
    "PAYLOAD_REJECTION_PATTERNS_4",
];
pub(super) const STALE_RESPONSE_ITEM_PATTERNS: &[&str] =
    &["STALE_RESPONSE_ITEM_PATTERNS_0", "STALE_RESPONSE_ITEM_PATTERNS_1"];
pub(super) const STATUS_MESSAGE_PATTERNS: &[&str] = &[
    "STATUS_MESSAGE_PATTERNS_0",
    "STATUS_MESSAGE_PATTERNS_1",
    "STATUS_MESSAGE_PATTERNS_2",
    "STATUS_MESSAGE_PATTERNS_3",
    "STATUS_MESSAGE_PATTERNS_4",
];
pub(super) fn regexp(ex: Execution<'_>, name: &'static str) -> Result<ValueHandle> {
    let &(binding, pattern, flags) = REGEXPS
        .iter()
        .find(|entry| entry.0 == name)
        .ok_or(ValueError::UnsupportedOperation("Fixed Native error regex binding"))?;
    ex.heap.module_regexp(MODULE, binding, pattern, flags)
}
