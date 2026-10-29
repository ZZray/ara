//! Fixed OMP596f2da packages/utils/src/ar/source.ts aligned-block cache.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use std::collections::VecDeque;
use std::sync::OnceLock;

type Block = Arc<OnceLock<std::result::Result<Arc<Vec<u8>>, ReadSourceFailure>>>;
struct CacheSource {
    source: Source,
    block_size: u64,
    max_blocks: usize,
    blocks: Mutex<VecDeque<(u64, Block)>>,
}
impl CacheSource {
    fn read_source(&self, start: u64, len: u64) -> std::result::Result<Vec<u8>, ReadSourceFailure> {
        match &self.source {
            Source::Read(source) => source.range_classified(start, len),
            _ => self
                .source
                .range(start, len)
                .map_err(ReadSourceFailure::External),
        }
    }
    fn block(&self, index: u64) -> std::result::Result<Arc<Vec<u8>>, ReadSourceFailure> {
        let pending = {
            let mut blocks = self
                .blocks
                .lock()
                .map_err(|_| ReadSourceFailure::External(error("Archive cache lock poisoned")))?;
            if let Some(position) = blocks.iter().position(|(key, _)| *key == index) {
                let pair = blocks
                    .remove(position)
                    .expect("position came from this cache");
                let pending = pair.1.clone();
                blocks.push_back(pair);
                pending
            } else {
                while blocks.len() >= self.max_blocks {
                    blocks.pop_front();
                }
                let pending = Arc::new(OnceLock::new());
                blocks.push_back((index, pending.clone()));
                pending
            }
        };
        pending
            .get_or_init(|| {
                let start = index * self.block_size;
                let result = self
                    .read_source(start, self.block_size.min(self.source.size() - start))
                    .map(Arc::new);
                if result.is_err() {
                    let mut blocks = self.blocks.lock().map_err(|_| {
                        ReadSourceFailure::External(error("Archive cache lock poisoned"))
                    })?;
                    // Register one rejection cleanup per pending block, like
                    // pending.catch. Waiters must not delete a later retry.
                    blocks.retain(|(key, _)| *key != index);
                }
                result
            })
            .clone()
    }
}
impl ReadByteSource for CacheSource {
    fn size(&self) -> u64 {
        self.source.size()
    }
    fn range(&self, start: u64, len: u64) -> Result<Vec<u8>> {
        self.range_classified(start, len)
            .map_err(ReadSourceFailure::flatten)
    }
    fn range_classified(
        &self,
        start: u64,
        len: u64,
    ) -> std::result::Result<Vec<u8>, ReadSourceFailure> {
        use ReadSourceFailure::Archive;
        let end = end(start, len).map_err(Archive)?;
        if end > self.size() {
            return Err(Archive(error("Invalid archive: truncated data")));
        }
        if start == end {
            return Ok(Vec::new());
        }
        let first = start / self.block_size;
        let last = (end - 1) / self.block_size;
        if last - first > 1 {
            return self.read_source(start, len);
        }
        let len = usize::try_from(len)
            .map_err(|_| Archive(error("Archive is too large to read safely")))?;
        let mut out = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| Archive(error("Archive is too large to read in memory")))?;
        out.resize(len, 0);
        for index in first..=last {
            let block = self.block(index)?;
            let block_start = index * self.block_size;
            let from = start.max(block_start);
            let to = end.min(block_start + block.len() as u64);
            if to < end.min(block_start.saturating_add(self.block_size)) {
                return Err(Archive(error("Invalid archive: truncated data")));
            }
            out[(from - start) as usize..(to - start) as usize].copy_from_slice(
                &block[(from - block_start) as usize..(to - block_start) as usize],
            );
        }
        Ok(out)
    }
}
/// Defaults at the caller are 256 KiB and 64 blocks, matching fixed OMP.
pub(crate) fn caching_byte_source(
    source: Source,
    block_size: u64,
    max_blocks: usize,
) -> Result<Source> {
    if block_size == 0 || block_size > JS_SAFE {
        return Err(error("Invalid archive cache block size"));
    }
    if max_blocks == 0 {
        return Err(error("At least one of max, maxSize, or ttl is required"));
    }
    Ok(Source::Read(Arc::new(CacheSource {
        source,
        block_size,
        max_blocks,
        blocks: Mutex::new(VecDeque::new()),
    })))
}

pub(crate) struct CachingByteSourceOptions {
    pub(crate) block_size: u64,
    pub(crate) max_blocks: usize,
}
impl Default for CachingByteSourceOptions {
    fn default() -> Self {
        Self {
            block_size: 256 * 1024,
            max_blocks: 64,
        }
    }
}
pub(crate) fn caching_byte_source_with_options(
    source: Source,
    options: CachingByteSourceOptions,
) -> Result<Source> {
    caching_byte_source(source, options.block_size, options.max_blocks)
}
