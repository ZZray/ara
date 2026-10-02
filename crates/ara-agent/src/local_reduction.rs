//! Pure local context reducers from fixed OMP 596f2da (MIT).
//! Source: packages/agent/src/compaction/{pruning,shake,tool-protection}.ts.
//! These DTOs describe original raw slots, never converted model wrappers.
//! The Session owner checks its full raw snapshot/leaf and persists the plan;
//! artifact I/O, provider rebinding and continuation remain host-owned.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ara_ai::JsonObject;

pub const SUPERSEDED_NOTICE: &str = "[Superseded by a newer read of this file]";
pub const USELESS_NOTICE: &str = "[Uneventful result elided]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReductionRole {
    User,
    Developer,
    Assistant,
    ToolResult,
    Custom,
    HookMessage,
    FileMention,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReductionEntryKind {
    Message(ReductionRole),
    CustomMessage { custom_type: String },
    Metadata,
    CompactionBoundary,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RawContentSlot {
    MessageContent,
    CustomContent,
    ToolResultDetailsImages,
    FileMentionImage(usize),
}

/// Opaque non-text blocks retain their original value in the Session snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReductionBlock {
    Text(String),
    Image,
    Thinking,
    RedactedThinking,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReductionContent {
    Absent,
    String(String),
    Blocks(Vec<ReductionBlock>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReductionContentSlot {
    pub slot: RawContentSlot,
    pub content: ReductionContent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReductionToolCall {
    pub id: String,
    pub name: String,
    pub arguments: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReductionToolResult {
    pub call_id: String,
    pub tool_name: String,
    /// Native checks presence, including null/non-number prunedAt values.
    pub already_pruned: bool,
    pub useless: bool,
    pub is_error: bool,
    pub source_type: Option<String>,
    pub source_value: Option<String>,
    /// Host predicate protection, e.g. the current plan reference path.
    pub host_protected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReductionEntry {
    pub entry_id: String,
    pub kind: ReductionEntryKind,
    pub timestamp_ms: Option<f64>,
    /// Raw type=message only; used by warm/supersede suffixes and tool age.
    pub raw_message_tokens: usize,
    /// Raw messages plus raw custom text, independently of prune suffixes.
    pub shake_entry_tokens: usize,
    pub slots: Vec<ReductionContentSlot>,
    pub tool_result: Option<ReductionToolResult>,
    pub tool_calls: Vec<ReductionToolCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtectedToolMatcher {
    ToolName(String),
    SkillRead,
    ArtifactRecovery,
}

fn skill_protection() -> Vec<ProtectedToolMatcher> {
    vec![ProtectedToolMatcher::ToolName("skill".into()), ProtectedToolMatcher::SkillRead]
}

#[derive(Clone, Debug, PartialEq)]
pub struct PruneConfig {
    pub protect_tokens: f64,
    pub minimum_savings: f64,
    pub protected_tools: Vec<ProtectedToolMatcher>,
    pub prune_useless: bool,
    pub keep_boundary_id: Option<String>,
    pub cache_warm_suffix_tokens: Option<f64>,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            protect_tokens: 40_000.0,
            minimum_savings: 20_000.0,
            protected_tools: skill_protection(),
            prune_useless: true,
            keep_boundary_id: None,
            cache_warm_suffix_tokens: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SupersedePruneConfig {
    pub prune_useless: bool,
    pub suffix_token_limit: f64,
    pub idle_flush_ms: f64,
    pub keep_boundary_id: Option<String>,
    pub protected_tools: Vec<ProtectedToolMatcher>,
}

impl Default for SupersedePruneConfig {
    fn default() -> Self {
        Self {
            prune_useless: false,
            suffix_token_limit: 8_000.0,
            idle_flush_ms: 30.0 * 60_000.0,
            keep_boundary_id: None,
            protected_tools: skill_protection(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShakeConfig {
    pub protect_tokens: f64,
    pub min_savings: f64,
    pub protected_tools: Vec<ProtectedToolMatcher>,
    pub fence_min_tokens: f64,
    pub keep_boundary_id: Option<String>,
}

impl Default for ShakeConfig {
    fn default() -> Self {
        let mut protected_tools = skill_protection();
        protected_tools.push(ProtectedToolMatcher::ArtifactRecovery);
        Self {
            protect_tokens: 16_000.0,
            min_savings: 4_000.0,
            protected_tools,
            fence_min_tokens: 400.0,
            keep_boundary_id: None,
        }
    }
}

impl ShakeConfig {
    pub fn aggressive() -> Self {
        Self {
            protect_tokens: 4_000.0,
            min_savings: 0.0,
            protected_tools: skill_protection(),
            fence_min_tokens: 400.0,
            keep_boundary_id: None,
        }
    }

    pub fn rescue() -> Self {
        let mut config = Self::aggressive();
        config.protect_tokens = 0.0;
        config.protected_tools.push(ProtectedToolMatcher::ArtifactRecovery);
        config
    }
}

pub trait ReductionTokenizer {
    /// Native countTokens receives text fragments; joining them can change
    /// tokenizer behavior. The host binds the selected model's counter.
    fn count_fragments(&self, fragments: &[&str]) -> usize;
}

impl<F: Fn(&[&str]) -> usize> ReductionTokenizer for F {
    fn count_fragments(&self, fragments: &[&str]) -> usize {
        self(fragments)
    }
}

pub type SupersedeKeyFn<'a> = dyn Fn(&str, &JsonObject) -> Option<String> + 'a;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReductionAction {
    ReplaceToolResultContent {
        text: String,
        pruned_at_ms: i64,
    },
    ElideToolResultText {
        text: String,
        pruned_at_ms: i64,
    },
    ReplaceTextRange {
        slot: RawContentSlot,
        block_index: Option<usize>,
        start_utf16: usize,
        end_utf16: usize,
        expected_text: Arc<str>,
        replacement: String,
    },
    DropBlocks {
        slot: RawContentSlot,
        indexes: Vec<usize>,
    },
    DropImages {
        slot: RawContentSlot,
        indexes: Vec<usize>,
        placeholder_if_empty: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReductionEdit {
    pub entry_id: String,
    pub action: ReductionAction,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReductionCounts {
    pub pruned_count: usize,
    pub tool_results_dropped: usize,
    pub blocks_dropped: usize,
    pub images_dropped: usize,
    pub thinking_blocks_dropped: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReductionPlan {
    pub expected_entries: Vec<ReductionEntry>,
    pub edits: Vec<ReductionEdit>,
    pub estimated_tokens_saved: usize,
    pub counts: ReductionCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReductionError {
    InvalidSource,
    InvalidSlot,
    StaleRegion,
    InvalidUtf16Range,
    InvalidReplacementCount,
}

impl std::fmt::Display for ReductionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "local reduction rejected: {self:?}")
    }
}
impl std::error::Error for ReductionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShakeRegionKind {
    ToolResult,
    Block {
        slot: RawContentSlot,
        block_index: Option<usize>,
        start_utf16: usize,
        end_utf16: usize,
        expected_text: Arc<str>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShakeRegion {
    pub entry_id: String,
    pub kind: ShakeRegionKind,
    pub tokens: usize,
    pub original_text: String,
    pub label: String,
}

fn validate_entries(entries: &[ReductionEntry]) -> Result<(), ReductionError> {
    let mut ids = HashSet::new();
    for entry in entries {
        if entry.entry_id.is_empty() || !ids.insert(entry.entry_id.as_str()) {
            return Err(ReductionError::InvalidSource);
        }
        let is_result = matches!(entry.kind, ReductionEntryKind::Message(ReductionRole::ToolResult));
        if is_result != entry.tool_result.is_some()
            || (!entry.tool_calls.is_empty()
                && !matches!(entry.kind, ReductionEntryKind::Message(ReductionRole::Assistant)))
        {
            return Err(ReductionError::InvalidSource);
        }
        let mut slots = HashSet::new();
        for slot in &entry.slots {
            if !slots.insert(&slot.slot) {
                return Err(ReductionError::InvalidSlot);
            }
            let legal = match slot.slot {
                RawContentSlot::MessageContent => matches!(entry.kind, ReductionEntryKind::Message(_)),
                RawContentSlot::CustomContent => matches!(entry.kind, ReductionEntryKind::CustomMessage { .. }),
                RawContentSlot::ToolResultDetailsImages => is_result,
                RawContentSlot::FileMentionImage(_) => {
                    matches!(entry.kind, ReductionEntryKind::Message(ReductionRole::FileMention))
                }
            };
            if !legal {
                return Err(ReductionError::InvalidSlot);
            }
            if matches!(slot.slot, RawContentSlot::ToolResultDetailsImages)
                && matches!(slot.content, ReductionContent::String(_))
            {
                return Err(ReductionError::InvalidSlot);
            }
            if matches!(slot.slot, RawContentSlot::FileMentionImage(_))
                && !matches!(&slot.content, ReductionContent::Absent)
                && !matches!(&slot.content, ReductionContent::Blocks(blocks)
                    if matches!(blocks.as_slice(), [ReductionBlock::Image]))
            {
                return Err(ReductionError::InvalidSlot);
            }
        }
        if is_result && !matches!(content(entry, &RawContentSlot::MessageContent), Some(ReductionContent::Blocks(_))) {
            return Err(ReductionError::InvalidSlot);
        }
    }
    Ok(())
}

fn content<'a>(entry: &'a ReductionEntry, slot: &RawContentSlot) -> Option<&'a ReductionContent> {
    entry.slots.iter().find(|candidate| &candidate.slot == slot).map(|candidate| &candidate.content)
}

fn empty_plan(entries: &[ReductionEntry]) -> ReductionPlan {
    ReductionPlan {
        expected_entries: entries.to_vec(),
        edits: Vec::new(),
        estimated_tokens_saved: 0,
        counts: ReductionCounts::default(),
    }
}

fn tool_calls_by_id(entries: &[ReductionEntry]) -> HashMap<&str, &ReductionToolCall> {
    let mut calls = HashMap::new();
    for entry in entries {
        if matches!(entry.kind, ReductionEntryKind::Message(ReductionRole::Assistant)) {
            for call in &entry.tool_calls {
                // Native Map.set: a later duplicate ID wins.
                calls.insert(call.id.as_str(), call);
            }
        }
    }
    calls
}

fn read_path<'a>(result: &ReductionToolResult, call: Option<&'a ReductionToolCall>) -> Option<&'a str> {
    if result.tool_name != "read" {
        return None;
    }
    let call = call.filter(|call| call.name == "read")?;
    call.arguments.get("path")?.as_str()
}

fn protected(
    result: &ReductionToolResult,
    call: Option<&ReductionToolCall>,
    matchers: &[ProtectedToolMatcher],
) -> bool {
    result.host_protected
        || matchers.iter().any(|matcher| match matcher {
            ProtectedToolMatcher::ToolName(name) => &result.tool_name == name,
            ProtectedToolMatcher::SkillRead => read_path(result, call).is_some_and(|path| path.starts_with("skill://")),
            ProtectedToolMatcher::ArtifactRecovery => {
                read_path(result, call).is_some_and(|path| path.starts_with("artifact://"))
                    || (result.source_type.as_deref() == Some("internal")
                        && result.source_value.as_deref().is_some_and(|value| value.starts_with("artifact://")))
            }
        })
}

fn boundary_index(entries: &[ReductionEntry], id: Option<&str>) -> usize {
    id.and_then(|id| entries.iter().position(|entry| entry.entry_id == id)).unwrap_or(0)
}

fn suffix_tokens(entries: &[ReductionEntry], shake: bool) -> Vec<usize> {
    let mut suffix = vec![0; entries.len()];
    let mut accumulated: usize = 0;
    for (index, entry) in entries.iter().enumerate().rev() {
        suffix[index] = accumulated;
        let tokens = if shake {
            match entry.kind {
                ReductionEntryKind::Message(_) | ReductionEntryKind::CustomMessage { .. } => entry.shake_entry_tokens,
                _ => 0,
            }
        } else if matches!(entry.kind, ReductionEntryKind::Message(_)) {
            entry.raw_message_tokens
        } else {
            0
        };
        accumulated = accumulated.saturating_add(tokens);
    }
    suffix
}

fn savings(tokens: usize, notice: &str) -> usize {
    tokens.saturating_sub(notice.encode_utf16().count().div_ceil(4))
}

fn superseded_indexes(
    entries: &[ReductionEntry],
    calls: &HashMap<&str, &ReductionToolCall>,
    key_fn: Option<&SupersedeKeyFn<'_>>,
    matchers: &[ProtectedToolMatcher],
) -> HashSet<usize> {
    let mut superseded = HashSet::new();
    let Some(key_fn) = key_fn else {
        return superseded;
    };
    let mut seen = HashSet::new();
    for (index, entry) in entries.iter().enumerate().rev() {
        let Some(result) = &entry.tool_result else {
            continue;
        };
        if result.already_pruned {
            continue;
        }
        let Some(call) = calls.get(result.call_id.as_str()).copied() else {
            continue;
        };
        if protected(result, Some(call), matchers) {
            continue;
        }
        let Some(key) = key_fn(&call.name, &call.arguments) else {
            continue;
        };
        let has_newer = seen.contains(&key) || key.find('\0').is_some_and(|separator| seen.contains(&key[..separator]));
        seen.insert(key);
        if has_newer {
            superseded.insert(index);
        }
    }
    superseded
}

fn useless_indexes(
    entries: &[ReductionEntry],
    calls: &HashMap<&str, &ReductionToolCall>,
    matchers: &[ProtectedToolMatcher],
    exclude: &HashSet<usize>,
) -> HashSet<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let result = entry.tool_result.as_ref()?;
            (result.useless
                && !result.already_pruned
                && !result.is_error
                && !exclude.contains(&index)
                && !protected(result, calls.get(result.call_id.as_str()).copied(), matchers)
                && savings(entry.raw_message_tokens, USELESS_NOTICE) > 0)
                .then_some(index)
        })
        .collect()
}

fn add_pruned(plan: &mut ReductionPlan, entry: &ReductionEntry, notice: String, pruned_at_ms: i64) {
    plan.estimated_tokens_saved =
        plan.estimated_tokens_saved.saturating_add(savings(entry.raw_message_tokens, &notice));
    plan.counts.pruned_count += 1;
    plan.edits.push(ReductionEdit {
        entry_id: entry.entry_id.clone(),
        action: ReductionAction::ReplaceToolResultContent { text: notice, pruned_at_ms },
    });
}

/// Fixed OMP pruning.ts:312-429. Age counts only ToolResult tokens; warm
/// suffixes count every raw message strictly after the candidate.
pub fn plan_prune(
    entries: &[ReductionEntry],
    config: &PruneConfig,
    supersede_key: Option<&SupersedeKeyFn<'_>>,
    pruned_at_ms: i64,
) -> Result<ReductionPlan, ReductionError> {
    validate_entries(entries)?;
    let calls = tool_calls_by_id(entries);
    let superseded = superseded_indexes(entries, &calls, supersede_key, &config.protected_tools);
    let useless = if config.prune_useless {
        useless_indexes(entries, &calls, &config.protected_tools, &superseded)
    } else {
        HashSet::new()
    };
    let boundary = boundary_index(entries, config.keep_boundary_id.as_deref());
    let suffix = config.cache_warm_suffix_tokens.map(|_| suffix_tokens(entries, false));
    let mut accumulated: usize = 0;
    let mut plan = empty_plan(entries);
    for (index, entry) in entries.iter().enumerate().rev() {
        let Some(result) = &entry.tool_result else {
            continue;
        };
        let tokens = entry.raw_message_tokens;
        let warm = config
            .cache_warm_suffix_tokens
            .zip(suffix.as_ref())
            .is_some_and(|(limit, suffix)| suffix[index] as f64 > limit);
        let superseded = superseded.contains(&index);
        let useless = useless.contains(&index);
        let ordinary_protected = (accumulated as f64) < config.protect_tokens
            || tokens < 50
            || protected(result, calls.get(result.call_id.as_str()).copied(), &config.protected_tools);
        if !result.already_pruned && !warm && index >= boundary && (superseded || useless || !ordinary_protected) {
            let notice = if superseded {
                SUPERSEDED_NOTICE.to_owned()
            } else if useless {
                USELESS_NOTICE.to_owned()
            } else {
                format!("[Output truncated - {tokens} tokens]")
            };
            add_pruned(&mut plan, entry, notice, pruned_at_ms);
        }
        accumulated = accumulated.saturating_add(tokens);
    }
    if (plan.estimated_tokens_saved as f64) < config.minimum_savings || plan.edits.is_empty() {
        return Ok(empty_plan(entries));
    }
    Ok(plan)
}

/// Fixed OMP pruning.ts:252-310. `now_ms` controls the idle decision; the
/// host supplies the separate mutation stamp, matching native Date.now().
pub fn plan_superseded_prune(
    entries: &[ReductionEntry],
    config: &SupersedePruneConfig,
    supersede_key: Option<&SupersedeKeyFn<'_>>,
    now_ms: f64,
    pruned_at_ms: i64,
) -> Result<ReductionPlan, ReductionError> {
    validate_entries(entries)?;
    let calls = tool_calls_by_id(entries);
    let superseded = superseded_indexes(entries, &calls, supersede_key, &config.protected_tools);
    let useless = if config.prune_useless {
        useless_indexes(entries, &calls, &config.protected_tools, &superseded)
    } else {
        HashSet::new()
    };
    let idle = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.kind, ReductionEntryKind::Message(_)))
        .and_then(|entry| entry.timestamp_ms)
        .is_some_and(|last| now_ms - last >= config.idle_flush_ms);
    let suffix = suffix_tokens(entries, false);
    let boundary = boundary_index(entries, config.keep_boundary_id.as_deref());
    let mut plan = empty_plan(entries);
    for (index, entry) in entries.iter().enumerate() {
        if index >= boundary && (idle || (suffix[index] as f64) <= config.suffix_token_limit) {
            if superseded.contains(&index) {
                add_pruned(&mut plan, entry, SUPERSEDED_NOTICE.into(), pruned_at_ms);
            } else if useless.contains(&index) {
                add_pruned(&mut plan, entry, USELESS_NOTICE.into(), pruned_at_ms);
            }
        }
    }
    Ok(plan)
}

fn range_selector(value: &str) -> bool {
    fn digits(bytes: &[u8], index: &mut usize) -> bool {
        if bytes.get(*index).is_some_and(|byte| *byte == b'l' || *byte == b'L') {
            *index += 1;
        }
        let start = *index;
        while bytes.get(*index).is_some_and(u8::is_ascii_digit) {
            *index += 1;
        }
        *index > start
    }
    if value.is_empty() {
        return false;
    }
    value.split(',').all(|chunk| {
        let bytes = chunk.as_bytes();
        let mut index = 0;
        if !digits(bytes, &mut index) {
            return false;
        }
        if index == bytes.len() {
            return true;
        }
        let trailing_allowed = match bytes[index] {
            b'-' => {
                index += 1;
                true
            }
            b'+' => {
                index += 1;
                false
            }
            b'.' if bytes.get(index + 1) == Some(&b'.') => {
                index += 2;
                true
            }
            _ => return false,
        };
        if index == bytes.len() {
            return trailing_allowed;
        }
        digits(bytes, &mut index) && index == bytes.len()
    })
}

/// Exact native read selector grammar, including drive letters and the two
/// raw/range compounds. URI reads are exempt from supersede grouping.
pub fn read_tool_supersede_key(tool_name: &str, args: &JsonObject) -> Option<String> {
    if tool_name != "read" {
        return None;
    }
    let path = args.get("path")?.as_str()?;
    if path.is_empty() || path.contains("://") {
        return None;
    }
    let Some(colon) = path.rfind(':').filter(|colon| *colon > 0) else {
        return Some(path.into());
    };
    let candidate = &path[colon + 1..];
    let candidate_raw = candidate.eq_ignore_ascii_case("raw");
    let candidate_range = range_selector(candidate);
    if !candidate_raw && !candidate_range && !candidate.eq_ignore_ascii_case("conflicts") {
        return Some(path.into());
    }
    let mut base = &path[..colon];
    let mut selector = candidate;
    if let Some(inner) = base.rfind(':').filter(|inner| *inner > 0) {
        let inner_candidate = &base[inner + 1..];
        if (inner_candidate.eq_ignore_ascii_case("raw") && candidate_range)
            || (range_selector(inner_candidate) && candidate_raw)
        {
            selector = &path[inner + 1..];
            base = &path[..inner];
        }
    }
    Some(format!("{base}\0{selector}"))
}

fn js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{000b}' | '\u{000c}' | '\r' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn xml_tag(line: &str, closing: bool) -> Option<&str> {
    let rest = line.strip_prefix(if closing { "</" } else { "<" })?.strip_suffix('>')?;
    let name_end = rest.bytes().take_while(|byte| byte.is_ascii_lowercase() || *byte == b'_' || *byte == b'-').count();
    if name_end == 0 {
        return None;
    }
    let tail = &rest[name_end..];
    if closing {
        return tail.is_empty().then_some(&rest[..name_end]);
    }
    if tail.is_empty() || (tail.starts_with(js_whitespace) && !tail.contains('>')) {
        Some(&rest[..name_end])
    } else {
        None
    }
}

/// Returns byte ranges internally; public region offsets are UTF-16 as in JS.
struct TextBlockRange {
    bytes: std::ops::Range<usize>,
    utf16: std::ops::Range<usize>,
}

fn block_ranges(text: &str) -> Vec<TextBlockRange> {
    let mut ranges = Vec::new();
    let mut fence_start = None;
    let mut tag_stack = Vec::new();
    let mut xml_start = 0;
    let mut xml_start_utf16 = 0;
    let mut line_start = 0;
    let mut line_start_utf16 = 0;
    for line in text.split('\n') {
        let line_end = line_start + line.len();
        let line_end_utf16 = line_start_utf16 + line.encode_utf16().count();
        let trimmed = line.trim_start_matches(js_whitespace);
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if let Some((start, start_utf16)) = fence_start.take() {
                ranges.push(TextBlockRange { bytes: start..line_end, utf16: start_utf16..line_end_utf16 });
            } else {
                fence_start = Some((line_start, line_start_utf16));
            }
        } else if fence_start.is_none() {
            if line.len() == trimmed.len()
                && let Some(tag) = xml_tag(trimmed, false)
            {
                if tag_stack.is_empty() {
                    xml_start = line_start;
                    xml_start_utf16 = line_start_utf16;
                }
                tag_stack.push(tag);
            } else if let Some(tag) = xml_tag(trimmed, true)
                && tag_stack.last().is_some_and(|top| *top == tag)
            {
                tag_stack.pop();
                if tag_stack.is_empty() {
                    ranges.push(TextBlockRange { bytes: xml_start..line_end, utf16: xml_start_utf16..line_end_utf16 });
                }
            }
        }
        line_start = line_end + 1;
        line_start_utf16 = line_end_utf16 + 1;
    }
    ranges.sort_by_key(|range| range.bytes.start);
    let mut last_end = 0;
    ranges.retain(|range| {
        if range.bytes.start < last_end {
            false
        } else {
            last_end = range.bytes.end;
            true
        }
    });
    ranges
}

fn tool_text(entry: &ReductionEntry) -> Option<(String, Vec<&str>)> {
    let ReductionContent::Blocks(blocks) = content(entry, &RawContentSlot::MessageContent)? else {
        return None;
    };
    let fragments: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            ReductionBlock::Text(text) if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if fragments.is_empty() { None } else { Some((fragments.join("\n"), fragments)) }
}

fn block_source(entry: &ReductionEntry) -> Option<(RawContentSlot, &str, bool)> {
    match &entry.kind {
        ReductionEntryKind::Message(ReductionRole::Assistant) => {
            Some((RawContentSlot::MessageContent, "assistant", false))
        }
        ReductionEntryKind::Message(ReductionRole::User) => Some((RawContentSlot::MessageContent, "user", true)),
        ReductionEntryKind::Message(ReductionRole::Developer) => {
            Some((RawContentSlot::MessageContent, "developer", true))
        }
        ReductionEntryKind::CustomMessage { custom_type } => Some((RawContentSlot::CustomContent, custom_type, true)),
        _ => None,
    }
}

fn push_block_regions(
    entry: &ReductionEntry,
    target: (&RawContentSlot, Option<usize>, &str, &str),
    tokenizer: &dyn ReductionTokenizer,
    config: &ShakeConfig,
    regions: &mut Vec<ShakeRegion>,
) {
    let (slot, block_index, text, label) = target;
    let mut expected_text = None;
    for range in block_ranges(text) {
        let original_text = &text[range.bytes];
        if original_text.is_empty() {
            continue;
        }
        let tokens = tokenizer.count_fragments(&[original_text]);
        if (tokens as f64) < config.fence_min_tokens {
            continue;
        }
        regions.push(ShakeRegion {
            entry_id: entry.entry_id.clone(),
            tokens,
            original_text: original_text.into(),
            label: label.into(),
            kind: ShakeRegionKind::Block {
                slot: slot.clone(),
                block_index,
                start_utf16: range.utf16.start,
                end_utf16: range.utf16.end,
                expected_text: Arc::clone(expected_text.get_or_insert_with(|| Arc::<str>::from(text))),
            },
        });
    }
}

/// Fixed OMP shake.ts:310-379, with document order retained for artifact labels.
/// Unlike incremental prune, shake intentionally may rewrite the warm prefix.
pub fn collect_shake_regions(
    entries: &[ReductionEntry],
    tokenizer: &dyn ReductionTokenizer,
    config: &ShakeConfig,
) -> Result<Vec<ShakeRegion>, ReductionError> {
    validate_entries(entries)?;
    let suffix = suffix_tokens(entries, true);
    let calls = tool_calls_by_id(entries);
    let boundary = boundary_index(entries, config.keep_boundary_id.as_deref());
    let mut regions = Vec::new();
    for (index, entry) in entries.iter().enumerate().skip(boundary) {
        let useless = entry.tool_result.as_ref().is_some_and(|result| result.useless && !result.is_error);
        if !useless && (suffix[index] as f64) < config.protect_tokens {
            continue;
        }
        if let Some(result) = &entry.tool_result {
            if result.already_pruned
                || protected(result, calls.get(result.call_id.as_str()).copied(), &config.protected_tools)
            {
                continue;
            }
            if let Some((original_text, fragments)) = tool_text(entry) {
                regions.push(ShakeRegion {
                    entry_id: entry.entry_id.clone(),
                    kind: ShakeRegionKind::ToolResult,
                    tokens: tokenizer.count_fragments(&fragments),
                    original_text,
                    label: result.tool_name.clone(),
                });
            }
            continue;
        }
        let Some((slot, label, allow_string)) = block_source(entry) else {
            continue;
        };
        match content(entry, &slot) {
            Some(ReductionContent::String(text)) if allow_string => {
                push_block_regions(entry, (&slot, None, text, label), tokenizer, config, &mut regions);
            }
            Some(ReductionContent::Blocks(blocks)) => {
                for (block_index, block) in blocks.iter().enumerate() {
                    if let ReductionBlock::Text(text) = block {
                        push_block_regions(
                            entry,
                            (&slot, Some(block_index), text, label),
                            tokenizer,
                            config,
                            &mut regions,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    let estimated: usize = regions.iter().map(|region| region.tokens.saturating_sub(16)).sum();
    if (estimated as f64) < config.min_savings {
        regions.clear();
    }
    Ok(regions)
}

/// Convert a JS character range without allowing a boundary inside a surrogate
/// pair. Session may use this helper while preserving opaque block fields.
pub fn utf16_byte_range(text: &str, start: usize, end: usize) -> Result<std::ops::Range<usize>, ReductionError> {
    if start > end {
        return Err(ReductionError::InvalidUtf16Range);
    }
    let mut utf16 = 0;
    let mut byte_start = None;
    let mut byte_end = None;
    for (byte, character) in text.char_indices() {
        if utf16 == start {
            byte_start = Some(byte);
        }
        if utf16 == end {
            return byte_start.map(|start| start..byte).ok_or(ReductionError::InvalidUtf16Range);
        }
        if utf16 > end {
            return Err(ReductionError::InvalidUtf16Range);
        }
        utf16 += character.len_utf16();
    }
    if utf16 == start {
        byte_start = Some(text.len());
    }
    if utf16 == end {
        byte_end = Some(text.len());
    }
    match (byte_start, byte_end) {
        (Some(start), Some(end)) => Ok(start..end),
        _ => Err(ReductionError::InvalidUtf16Range),
    }
}

pub fn shake_placeholder(tokens: usize, artifact_id: Option<&str>, region_index: usize) -> String {
    if let Some(id) = artifact_id {
        format!("[shaken ~{tokens} tokens — recover: artifact://{id} (region {})]", region_index + 1)
    } else {
        format!("[shaken ~{tokens} tokens]")
    }
}

fn block_text<'a>(entry: &'a ReductionEntry, slot: &RawContentSlot, block_index: Option<usize>) -> Option<&'a str> {
    match (content(entry, slot)?, block_index) {
        (ReductionContent::String(text), None) => Some(text),
        (ReductionContent::Blocks(blocks), Some(index)) => match blocks.get(index)? {
            ReductionBlock::Text(text) => Some(text),
            _ => None,
        },
        _ => None,
    }
}

/// After host artifact I/O, verify every located region against this snapshot
/// and order edits highest-start-first. The Session owner still checks the full
/// original raw branch/leaf before committing this plan.
pub fn finalize_shake_plan(
    entries: &[ReductionEntry],
    regions: &[ShakeRegion],
    replacements: &[String],
    pruned_at_ms: i64,
) -> Result<ReductionPlan, ReductionError> {
    validate_entries(entries)?;
    if regions.len() != replacements.len() {
        return Err(ReductionError::InvalidReplacementCount);
    }
    let by_id: HashMap<&str, &ReductionEntry> = entries.iter().map(|entry| (entry.entry_id.as_str(), entry)).collect();
    let mut tool_ids = HashSet::new();
    let mut block_ranges: HashMap<_, Vec<std::ops::Range<usize>>> = HashMap::new();
    let mut plan = empty_plan(entries);
    for (region, replacement) in regions.iter().zip(replacements) {
        let entry = by_id.get(region.entry_id.as_str()).ok_or(ReductionError::StaleRegion)?;
        let action = match &region.kind {
            ShakeRegionKind::ToolResult => {
                let result = entry.tool_result.as_ref().ok_or(ReductionError::InvalidSource)?;
                if result.already_pruned
                    || !tool_ids.insert(region.entry_id.as_str())
                    || tool_text(entry).is_none_or(|(text, _)| text != region.original_text)
                    || result.tool_name != region.label
                {
                    return Err(ReductionError::StaleRegion);
                }
                plan.counts.tool_results_dropped += 1;
                ReductionAction::ElideToolResultText { text: replacement.clone(), pruned_at_ms }
            }
            ShakeRegionKind::Block { slot, block_index, start_utf16, end_utf16, expected_text } => {
                let Some((legal_slot, label, allow_string)) = block_source(entry) else {
                    return Err(ReductionError::InvalidSource);
                };
                if slot != &legal_slot || label != region.label || (!allow_string && block_index.is_none()) {
                    return Err(ReductionError::InvalidSlot);
                }
                let text = block_text(entry, slot, *block_index).ok_or(ReductionError::InvalidSlot)?;
                if text != expected_text.as_ref() {
                    return Err(ReductionError::StaleRegion);
                }
                let range = utf16_byte_range(text, *start_utf16, *end_utf16)?;
                if range.is_empty() || text[range] != region.original_text {
                    return Err(ReductionError::StaleRegion);
                }
                let ranges = block_ranges.entry((region.entry_id.as_str(), slot, *block_index)).or_default();
                if ranges.iter().any(|range| *start_utf16 < range.end && range.start < *end_utf16) {
                    return Err(ReductionError::StaleRegion);
                }
                ranges.push(*start_utf16..*end_utf16);
                plan.counts.blocks_dropped += 1;
                ReductionAction::ReplaceTextRange {
                    slot: slot.clone(),
                    block_index: *block_index,
                    start_utf16: *start_utf16,
                    end_utf16: *end_utf16,
                    expected_text: expected_text.clone(),
                    replacement: replacement.clone(),
                }
            }
        };
        plan.estimated_tokens_saved = plan.estimated_tokens_saved.saturating_add(region.tokens.saturating_sub(16));
        plan.edits.push(ReductionEdit { entry_id: region.entry_id.clone(), action });
    }
    plan.edits.sort_by(|a, b| {
        let start = |edit: &ReductionEdit| match edit.action {
            ReductionAction::ReplaceTextRange { start_utf16, .. } => Some(start_utf16),
            _ => None,
        };
        // None is native -1: all block regions precede tool-result elisions.
        start(b).cmp(&start(a))
    });
    Ok(plan)
}

/// Fixed messages.ts:835-888 plus maintenance's raw custom_message case.
/// This walks the full branch, independent of the compaction keep boundary.
pub fn plan_drop_images(entries: &[ReductionEntry]) -> Result<ReductionPlan, ReductionError> {
    validate_entries(entries)?;
    let mut plan = empty_plan(entries);
    for entry in entries {
        for slot in &entry.slots {
            let eligible = matches!(
                (&entry.kind, &slot.slot),
                (
                    ReductionEntryKind::Message(
                        ReductionRole::User
                            | ReductionRole::Developer
                            | ReductionRole::Custom
                            | ReductionRole::HookMessage
                            | ReductionRole::ToolResult
                    ),
                    RawContentSlot::MessageContent
                ) | (ReductionEntryKind::CustomMessage { .. }, RawContentSlot::CustomContent)
                    | (ReductionEntryKind::Message(ReductionRole::ToolResult), RawContentSlot::ToolResultDetailsImages)
                    | (ReductionEntryKind::Message(ReductionRole::FileMention), RawContentSlot::FileMentionImage(_))
            );
            if !eligible {
                continue;
            }
            let ReductionContent::Blocks(blocks) = &slot.content else {
                continue;
            };
            let indexes: Vec<usize> = blocks
                .iter()
                .enumerate()
                .filter_map(|(index, block)| matches!(block, ReductionBlock::Image).then_some(index))
                .collect();
            if indexes.is_empty() {
                continue;
            }
            plan.counts.images_dropped += indexes.len();
            let placeholder_if_empty =
                matches!(slot.slot, RawContentSlot::MessageContent | RawContentSlot::CustomContent);
            plan.edits.push(ReductionEdit {
                entry_id: entry.entry_id.clone(),
                action: ReductionAction::DropImages { slot: slot.slot.clone(), indexes, placeholder_if_empty },
            });
        }
    }
    Ok(plan)
}

/// Fixed maintenance.ts:609-633. Only raw Assistant blocks are affected;
/// dropping all thinking keeps the native empty Assistant content array.
pub fn plan_drop_thinking(entries: &[ReductionEntry]) -> Result<ReductionPlan, ReductionError> {
    validate_entries(entries)?;
    let mut plan = empty_plan(entries);
    for entry in entries {
        if !matches!(entry.kind, ReductionEntryKind::Message(ReductionRole::Assistant)) {
            continue;
        }
        let Some(ReductionContent::Blocks(blocks)) = content(entry, &RawContentSlot::MessageContent) else {
            continue;
        };
        let indexes: Vec<usize> = blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                matches!(block, ReductionBlock::Thinking | ReductionBlock::RedactedThinking).then_some(index)
            })
            .collect();
        if indexes.is_empty() {
            continue;
        }
        plan.counts.thinking_blocks_dropped += indexes.len();
        plan.edits.push(ReductionEdit {
            entry_id: entry.entry_id.clone(),
            action: ReductionAction::DropBlocks { slot: RawContentSlot::MessageContent, indexes },
        });
    }
    Ok(plan)
}
