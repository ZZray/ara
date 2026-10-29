//! Fixed TranscriptContainer mutable block allocator and transactional history.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! EDIT ToolExecutionComponent uses the original default mutable block mode.
//! Append-only semantic row publication for other OMP components remains a
//! separate source surface and is not represented by fabricated stable keys.
use super::render_utils::wrap;
use ara_rpc::WireString;
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockKey {
    Text(u64),
    CacheMarker(u64),
    Session(String, usize),
    Tool(u64, WireString),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockState {
    Active,
    Settled,
    Committed,
}
#[derive(Clone, Debug)]
struct Entry {
    key: BlockKey,
    state: BlockState,
    rows: Vec<String>,
    text: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryKind {
    Append,
    Replay,
    ReplaceCollapsed,
}
#[derive(Clone, Debug)]
pub struct HistoryBatch {
    pub id: u64,
    pub rows: Vec<String>,
    pub kind: HistoryKind,
}
#[derive(Clone, Debug)]
struct Offered {
    batch: HistoryBatch,
    end: Option<usize>,
}
#[derive(Clone, Debug)]
pub struct FramePlan {
    pub history: Option<HistoryBatch>,
    pub viewport: Vec<String>,
}
#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    frontier: usize,
    next_batch: u64,
    offered: Option<Offered>,
    replay_pending: bool,
    replay_requested: bool,
    flush: bool,
    columns: usize,
    pending_selection: Option<(Vec<BlockKey>, bool)>,
    replacement_pending: bool,
}
pub(crate) struct FrameTransaction {
    pub plan: FramePlan,
    updates: Vec<(usize, Vec<String>)>,
    offered: Option<Offered>,
    replay_consumed: bool,
    next_batch: u64,
}
impl Transcript {
    /// Mutable EDIT caller frontier from fixed peekBatch -> offerHistory ->
    /// renderViewport. The supplied renderer's scratch persists across passes.
    pub(crate) fn prepare_frame(
        &self,
        columns: usize,
        capacity: usize,
        mut draw: impl FnMut(&BlockKey, usize, usize) -> Option<Vec<String>>,
    ) -> FrameTransaction {
        let mut updated = std::collections::HashMap::<usize, Vec<String>>::new();
        let mut render = |index: usize, allocation: usize| {
            let entry = &self.entries[index];
            let rows = if let Some(text) = &entry.text {
                trim_edges(text.split('\n').flat_map(|line| wrap(line, columns)).collect())
            } else {
                let rows = draw(&entry.key, columns, allocation).unwrap_or_else(|| entry.rows.clone());
                if matches!(&entry.key, BlockKey::Session(..)) { rows } else { trim_edges(rows) }
            };
            updated.insert(index, rows.clone());
            rows
        };
        let mut range = |start: usize, end: usize, trailing: bool| {
            let mut rows = Vec::new();
            for i in start..end {
                let block = render(i, usize::MAX);
                if block.is_empty() {
                    continue;
                }
                if !rows.is_empty() {
                    rows.push(String::new())
                }
                rows.extend(block);
            }
            if trailing && !rows.is_empty() {
                rows.push(String::new())
            }
            rows
        };
        let mut offered = self.offered.clone();
        let mut next_batch = self.next_batch;
        let replay_consumed = offered.is_none() && self.replay_pending;
        if offered.is_none() && self.replacement_pending {
            let rows = range(0, self.frontier, true);
            next_batch = next_batch.wrapping_add(1);
            offered = Some(Offered {
                batch: HistoryBatch { id: next_batch, rows, kind: HistoryKind::ReplaceCollapsed },
                end: None,
            });
        }
        if offered.is_none() && self.replay_pending {
            let rows = range(0, self.frontier, true);
            if !rows.is_empty() {
                next_batch = next_batch.wrapping_add(1);
                offered = Some(Offered {
                    batch: HistoryBatch { id: next_batch, rows, kind: HistoryKind::Replay },
                    end: None,
                });
            }
        }
        // End the range closure's borrow before live measurement; subsequent
        // history/viewport calls still mutate the same renderer scratch.
        drop(range);
        if offered.is_none() {
            let mut heights = Vec::new();
            let mut total = 0usize;
            let mut visible = 0;
            for i in self.frontier..self.entries.len() {
                let rows = render(i, usize::MAX);
                heights.push(rows.len());
                if !rows.is_empty() {
                    total += rows.len() + usize::from(visible > 0);
                    visible += 1;
                }
            }
            if self.flush || total > capacity || self.entries.len().saturating_sub(self.frontier) >= 256 {
                let mut end = self.frontier;
                let mut freed = 0;
                while end < self.entries.len() && self.entries[end].state == BlockState::Settled {
                    if !self.flush
                        && total.saturating_sub(freed) <= capacity
                        && self.entries.len().saturating_sub(end) < 256
                    {
                        break;
                    }
                    let height = heights[end - self.frontier];
                    if height > 0 {
                        freed += height + 1
                    }
                    end += 1;
                }
                if end > self.frontier {
                    let mut rows = Vec::new();
                    for i in self.frontier..end {
                        let block = render(i, usize::MAX);
                        if block.is_empty() {
                            continue;
                        }
                        if !rows.is_empty() {
                            rows.push(String::new())
                        }
                        rows.extend(block);
                    }
                    if !rows.is_empty() {
                        rows.push(String::new())
                    }
                    next_batch = next_batch.wrapping_add(1);
                    offered = Some(Offered {
                        batch: HistoryBatch { id: next_batch, rows, kind: HistoryKind::Append },
                        end: Some(end),
                    });
                }
            }
        }
        let start = offered.as_ref().and_then(|o| o.end).unwrap_or(self.frontier);
        let mut shown = Vec::new();
        let mut blocks = Vec::new();
        let mut total = 0usize;
        if capacity > 0 {
            for i in start..self.entries.len() {
                let block = render(i, usize::MAX);
                if block.is_empty() {
                    continue;
                }
                total += block.len() + usize::from(!shown.is_empty());
                shown.push(i);
                blocks.push(block);
            }
        }
        let mut viewport = Vec::new();
        if shown.len() > capacity {
            let active = shown.iter().filter(|i| self.entries[**i].state == BlockState::Active).count();
            let mut visible = capacity;
            let mut hidden = 0;
            for _ in 0..2 {
                let slice = &shown[shown.len().saturating_sub(visible)..];
                hidden = active
                    .saturating_sub(slice.iter().filter(|i| self.entries[**i].state == BlockState::Active).count());
                if hidden == active && hidden > 0 {
                    hidden -= 1
                }
                if hidden > 0 { visible = capacity.saturating_sub(1) } else { break }
            }
            if hidden > 0 {
                viewport.push(format!("{hidden} more transcript blocks active"));
            }
            for i in &shown[shown.len().saturating_sub(visible)..] {
                let rows = render(*i, 1);
                viewport.push(rows.first().cloned().unwrap_or_default());
            }
        } else if total <= capacity {
            for block in blocks {
                if !viewport.is_empty() {
                    viewport.push(String::new())
                }
                viewport.extend(block);
            }
        } else if !shown.is_empty() {
            let mut allocation = vec![1usize; shown.len()];
            let mut surplus = capacity - shown.len();
            for i in (0..shown.len()).rev() {
                let extra = blocks[i].len().saturating_sub(1).min(surplus);
                allocation[i] += extra;
                surplus -= extra;
                if surplus == 0 {
                    break;
                }
            }
            for (i, index) in shown.iter().enumerate() {
                let block = render(*index, allocation[i]);
                let keep = allocation[i].min(block.len());
                viewport.extend(block[block.len() - keep..].iter().cloned());
            }
        }
        drop(render);
        FrameTransaction {
            plan: FramePlan { history: offered.as_ref().map(|o| o.batch.clone()), viewport },
            updates: updated.into_iter().collect(),
            offered,
            replay_consumed,
            next_batch,
        }
    }
    /// Native surfaces describe the complete retained ledger, independent of
    /// ANSI scrollback/frontier. Existing unsupported components retain rows.
    pub(crate) fn native_entries(&self) -> Vec<(BlockKey, Option<String>, Vec<String>)> {
        self.entries.iter().map(|e| (e.key.clone(), e.text.clone(), e.rows.clone())).collect()
    }
    pub(crate) fn tool_keys(&self) -> Vec<BlockKey> {
        self.entries.iter().filter(|e| matches!(&e.key, BlockKey::Tool(..))).map(|e| e.key.clone()).collect()
    }
    pub(crate) fn commit_frame(&mut self, transaction: FrameTransaction) {
        for (i, rows) in transaction.updates {
            self.entries[i].rows = rows;
        }
        self.offered = transaction.offered;
        self.next_batch = transaction.next_batch;
        if transaction.replay_consumed {
            self.replay_pending = false;
        }
    }

    /// Snapshot ordering never invalidates an outstanding physical history offer.
    pub(crate) fn select_session(&mut self, keys: Vec<BlockKey>, clear: bool) {
        if self.offered.is_some() {
            let clear = clear || self.pending_selection.as_ref().is_some_and(|(_, pending)| *pending);
            self.pending_selection = Some((keys, clear));
        } else {
            self.select_now(keys, clear);
        }
    }
    fn select_now(&mut self, keys: Vec<BlockKey>, clear: bool) {
        let old_committed: Vec<_> = self.entries[..self.frontier].iter().map(|e| e.key.clone()).collect();
        let mut existing: std::collections::HashMap<_, _> =
            self.entries.drain(..).map(|entry| (entry.key.clone(), entry)).collect();
        let selected: Vec<_> = keys.into_iter().filter_map(|key| existing.remove(&key)).collect();
        let common = selected.iter().zip(&old_committed).take_while(|(a, b)| &a.key == *b).count();
        self.entries = selected;
        self.frontier =
            if clear { self.entries.iter().take_while(|e| e.state != BlockState::Active).count() } else { common };
        for (i, e) in self.entries.iter_mut().enumerate() {
            if i < self.frontier {
                e.state = BlockState::Committed;
            } else if e.state == BlockState::Committed {
                e.state = BlockState::Settled;
            }
        }
        self.replacement_pending |= clear;
        self.replay_pending = false;
        self.replay_requested = false;
    }
    pub(crate) fn active_keys(&self) -> Vec<BlockKey> {
        self.entries.iter().filter(|e| e.state == BlockState::Active).map(|e| e.key.clone()).collect()
    }
    pub fn contains(&self, key: &BlockKey) -> bool {
        self.entries.iter().any(|e| &e.key == key)
    }
    pub(crate) fn add_cache_marker(&mut self, id: u64, assistant_first_tool: Option<&BlockKey>) {
        let key = BlockKey::CacheMarker(id);
        if self.contains(&key) {
            return;
        }
        let index = assistant_first_tool
            .and_then(|anchor| self.entries.iter().position(|e| &e.key == anchor))
            .unwrap_or(self.entries.len());
        self.entries.insert(index, Entry { key, state: BlockState::Settled, rows: Vec::new(), text: None });
        // Preserve any already-offered physical receipt verbatim. Insertion
        // inside that receipt is reflected by a replay only after its ack.
        if index < self.frontier {
            self.frontier += 1;
            self.begin_history_replay();
        }
        if let Some(offered) = &mut self.offered {
            if let Some(end) = &mut offered.end {
                if index < *end {
                    *end += 1;
                    self.replay_requested = true;
                }
            }
        }
    }
    pub(crate) fn block_keys(&self) -> Vec<BlockKey> {
        self.entries.iter().map(|entry| entry.key.clone()).collect()
    }
    pub fn add_tool(&mut self, key: BlockKey, settled: bool) {
        if self.contains(&key) {
            return;
        }
        self.entries.push(Entry {
            key,
            state: if settled { BlockState::Settled } else { BlockState::Active },
            rows: Vec::new(),
            text: None,
        });
    }
    pub fn add_text(&mut self, id: u64, text: String, settled: bool) {
        let key = BlockKey::Text(id);
        if self.contains(&key) {
            return;
        }
        self.entries.push(Entry {
            key,
            state: if settled { BlockState::Settled } else { BlockState::Active },
            rows: Vec::new(),
            text: Some(text),
        });
    }
    pub(crate) fn set_text(&mut self, id: u64, text: String) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == BlockKey::Text(id)) {
            entry.text = Some(text);
        }
    }
    pub fn append_text(&mut self, id: u64, delta: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == BlockKey::Text(id)) {
            if let Some(text) = &mut entry.text {
                text.push_str(delta);
            }
        }
    }
    pub fn update_rows(&mut self, key: &BlockKey, rows: Vec<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|e| &e.key == key) {
            entry.rows = if matches!(key, BlockKey::Session(..)) { rows } else { trim_edges(rows) };
        }
    }
    pub fn settle(&mut self, key: &BlockKey) {
        if let Some(entry) = self.entries.iter_mut().find(|e| &e.key == key) {
            if entry.state == BlockState::Active {
                entry.state = BlockState::Settled;
            }
        }
    }
    pub fn can_admit(&self, rows: usize) -> bool {
        rows > self.entries.iter().filter(|e| e.state == BlockState::Active).count()
            && self.entries.len().saturating_sub(self.frontier) < 256
    }
    pub fn block_states(&self) -> Vec<BlockState> {
        self.entries.iter().map(|e| e.state).collect()
    }
    pub fn begin_history_replay(&mut self) {
        if self.offered.is_some() {
            self.replay_requested = true;
        } else {
            self.replay_pending = self.frontier > 0;
        }
    }
    pub fn begin_history_flush(&mut self) {
        self.replay_pending = false;
        self.replay_requested = false;
        self.flush = true;
    }
    fn range(&self, start: usize, end: usize, trailing_blank: bool) -> Vec<String> {
        let mut out = Vec::new();
        for entry in &self.entries[start..end] {
            if entry.rows.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(String::new());
            }
            out.extend(entry.rows.clone());
        }
        if trailing_blank && !out.is_empty() {
            out.push(String::new());
        }
        out
    }
    fn offer(&mut self, rows: Vec<String>, kind: HistoryKind, end: Option<usize>) -> HistoryBatch {
        self.next_batch = self.next_batch.wrapping_add(1);
        let batch = HistoryBatch { id: self.next_batch, rows, kind };
        self.offered = Some(Offered { batch: batch.clone(), end });
        batch
    }
    pub fn render_frame(&mut self, columns: usize, rows: usize) -> FramePlan {
        self.columns = columns;
        for entry in &mut self.entries {
            if let Some(text) = &entry.text {
                entry.rows = trim_edges(text.split('\n').flat_map(|line| wrap(line, columns)).collect());
            }
        }
        let history = if let Some(offered) = &self.offered {
            Some(offered.batch.clone())
        } else if self.replay_pending {
            self.replay_pending = false;
            let replay = self.range(0, self.frontier, true);
            if replay.is_empty() { None } else { Some(self.offer(replay, HistoryKind::Replay, None)) }
        } else {
            let live_height = self.entries[self.frontier..]
                .iter()
                .filter(|e| !e.rows.is_empty())
                .map(|e| e.rows.len() + 1)
                .sum::<usize>()
                .saturating_sub(1);
            let overflowing = live_height > rows || self.entries.len().saturating_sub(self.frontier) >= 256;
            if self.flush || overflowing {
                let mut end = self.frontier;
                let mut freed = 0;
                while end < self.entries.len() && self.entries[end].state == BlockState::Settled {
                    if !self.flush
                        && live_height.saturating_sub(freed) <= rows
                        && self.entries.len().saturating_sub(end) < 256
                    {
                        break;
                    }
                    if !self.entries[end].rows.is_empty() {
                        freed += self.entries[end].rows.len() + 1;
                    }
                    end += 1;
                }
                if end > self.frontier {
                    let retired = self.range(self.frontier, end, true);
                    Some(self.offer(retired, HistoryKind::Append, Some(end)))
                } else {
                    None
                }
            } else {
                None
            }
        };
        let start = self.offered.as_ref().and_then(|offered| offered.end).unwrap_or(self.frontier);
        let blocks = self.entries[start..].iter().filter(|entry| !entry.rows.is_empty()).collect::<Vec<_>>();
        let viewport = allocate(&blocks, rows);
        FramePlan { history, viewport }
    }
    /// A failed/partial physical write retains this exact offer. Only the
    /// complete synchronous writer transaction may acknowledge its identifier.
    pub fn acknowledge_history(&mut self, id: u64) -> bool {
        let Some(offered) = &self.offered else {
            return false;
        };
        if offered.batch.id != id {
            return false;
        }
        if let Some(end) = offered.end {
            for entry in &mut self.entries[self.frontier..end] {
                entry.state = BlockState::Committed;
            }
            self.frontier = end;
        }
        if offered.batch.kind == HistoryKind::ReplaceCollapsed {
            self.replacement_pending = false;
        }
        self.offered = None;
        if let Some((keys, clear)) = self.pending_selection.take() {
            self.select_now(keys, clear);
        }
        if self.frontier == self.entries.len() {
            self.flush = false;
        }
        if self.replay_requested {
            self.replay_requested = false;
            self.replay_pending = self.frontier > 0;
        }
        true
    }
    pub fn render_resize_frame(&self, rows: usize) -> Vec<String> {
        let mut out = Vec::new();
        for entry in self.entries.iter().rev() {
            if entry.rows.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.insert(0, String::new());
            }
            let mut block = entry.rows.clone();
            block.extend(out);
            out = block;
            if out.len() >= rows {
                break;
            }
        }
        if out.len() > rows { out.split_off(out.len() - rows) } else { out }
    }
}
fn trim_edges(mut rows: Vec<String>) -> Vec<String> {
    let start = rows.iter().position(|s| !s.trim().is_empty()).unwrap_or(rows.len());
    let end = rows.iter().rposition(|s| !s.trim().is_empty()).map(|i| i + 1).unwrap_or(start);
    rows.drain(end..);
    rows.drain(..start);
    rows
}
fn allocate(blocks: &[&Entry], capacity: usize) -> Vec<String> {
    if capacity == 0 || blocks.is_empty() {
        return Vec::new();
    }
    if blocks.len() > capacity {
        let active = blocks.iter().filter(|b| b.state == BlockState::Active).count();
        let mut visible_rows = capacity;
        let mut shown = &blocks[blocks.len() - visible_rows..];
        let mut hidden = active.saturating_sub(shown.iter().filter(|b| b.state == BlockState::Active).count());
        if hidden == active && hidden > 0 {
            hidden -= 1;
        }
        if hidden > 0 {
            visible_rows = capacity.saturating_sub(1);
            shown = &blocks[blocks.len() - visible_rows..];
            hidden = active.saturating_sub(shown.iter().filter(|b| b.state == BlockState::Active).count());
            if hidden == active && hidden > 0 {
                hidden -= 1;
            }
        }
        let mut out = if hidden > 0 { vec![format!("{hidden} more transcript blocks active")] } else { Vec::new() };
        for block in shown {
            out.push(block.rows.first().cloned().unwrap_or_default());
        }
        out.truncate(capacity);
        return out;
    }
    let total = blocks.iter().map(|e| e.rows.len()).sum::<usize>() + blocks.len() - 1;
    if total <= capacity {
        let mut out = Vec::new();
        for block in blocks {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.extend(block.rows.clone());
        }
        return out;
    }
    let mut allocations = vec![1; blocks.len()];
    let mut surplus = capacity - blocks.len();
    for (i, block) in blocks.iter().enumerate().rev() {
        let extra = block.rows.len().saturating_sub(1).min(surplus);
        allocations[i] += extra;
        surplus -= extra;
        if surplus == 0 {
            break;
        }
    }
    let mut out = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let keep = allocations[i].min(block.rows.len());
        out.extend(block.rows[block.rows.len() - keep..].iter().cloned());
    }
    out
}
