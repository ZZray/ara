//! Fixed OMP596f2da ar/source.ts source seam for readonly format decoders.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
mod cache;
mod http;
pub(super) use cache::{
    CachingByteSourceOptions, caching_byte_source, caching_byte_source_with_options,
};
pub(super) use http::{HttpByteSourceOptions, HttpFetch, HttpResponse, http_byte_source};

#[derive(Clone)]
pub(crate) enum ReadSourceFailure {
    Archive(ToolError),
    External(ToolError),
}
impl ReadSourceFailure {
    fn flatten(self) -> ToolError {
        match self {
            Self::Archive(error) | Self::External(error) => error,
        }
    }
}
pub(crate) trait ReadByteSource: Send + Sync {
    fn size(&self) -> u64;
    fn range(&self, start: u64, len: u64) -> Result<Vec<u8>>;
    fn range_classified(
        &self,
        start: u64,
        len: u64,
    ) -> std::result::Result<Vec<u8>, ReadSourceFailure> {
        self.range(start, len).map_err(ReadSourceFailure::External)
    }
}
fn end(start: u64, len: u64) -> Result<u64> {
    if start > JS_SAFE || len > JS_SAFE {
        return Err(error("Invalid archive range"));
    }
    start
        .checked_add(len)
        .filter(|&end| end <= JS_SAFE)
        .ok_or_else(|| error("Invalid archive range"))
}
pub(crate) struct ReadFileSource {
    path: PathBuf,
    size: u64,
}
impl ReadFileSource {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let size = std::fs::metadata(path).map_err(io_error)?.len();
        if size > JS_SAFE {
            return Err(error("Archive is too large to read safely"));
        }
        Ok(Self {
            path: path.into(),
            size,
        })
    }
}
impl ReadByteSource for ReadFileSource {
    fn size(&self) -> u64 {
        self.size
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
        use ReadSourceFailure::{Archive, External};
        let end = end(start, len).map_err(Archive)?;
        // Bun.file.slice permits an empty range past the captured size. It
        // opens the current path when read; it does not retain the old fd.
        let mut file = File::open(&self.path).map_err(|err| External(io_error(err)))?;
        if len == 0 {
            return Ok(Vec::new());
        }
        if end
            > file
                .metadata()
                .map_err(|err| External(io_error(err)))?
                .len()
        {
            return Err(Archive(error("Invalid archive: truncated data")));
        }
        file.seek(SeekFrom::Start(start))
            .map_err(|err| External(io_error(err)))?;
        let len = usize::try_from(len)
            .map_err(|_| Archive(error("Archive is too large to read safely")))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| Archive(error("Archive is too large to read in memory")))?;
        bytes.resize(len, 0);
        match file.read_exact(&mut bytes) {
            Ok(()) => Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                Err(Archive(super::error("Invalid archive: truncated data")))
            }
            Err(error) => Err(External(io_error(error))),
        }
    }
}
pub(crate) struct ReadMemorySource {
    pub(crate) bytes: Arc<Vec<u8>>,
}
impl ReadByteSource for ReadMemorySource {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn range(&self, start: u64, len: u64) -> Result<Vec<u8>> {
        let end = end(start, len)?;
        if end > self.size() {
            return Err(error("Invalid archive: truncated data"));
        }
        Ok(self.bytes[start as usize..end as usize].to_vec())
    }
    fn range_classified(
        &self,
        start: u64,
        len: u64,
    ) -> std::result::Result<Vec<u8>, ReadSourceFailure> {
        self.range(start, len).map_err(ReadSourceFailure::Archive)
    }
}
pub(super) fn read_with_source_error(
    source: &Source,
    start: u64,
    len: u64,
    wrap_external: impl FnOnce(ToolError) -> ToolError,
) -> Result<Vec<u8>> {
    validate_range(start, len)?;
    match source {
        Source::Read(source) => {
            source
                .range_classified(start, len)
                .map_err(|failure| match failure {
                    ReadSourceFailure::Archive(error) => error,
                    ReadSourceFailure::External(error) => wrap_external(error),
                })
        }
        Source::Memory(_) => source.range(start, len),
        Source::File { .. } => Err(error(
            "READ archive source must retain read-only source ownership",
        )),
    }
}
pub(super) fn source_from_path(path: &Path) -> Result<Source> {
    Ok(Source::Read(Arc::new(ReadFileSource::open(path)?)))
}
pub(super) fn source_from_bytes(bytes: Vec<u8>) -> Source {
    Source::Read(Arc::new(ReadMemorySource {
        bytes: Arc::new(bytes),
    }))
}
pub(in crate::write_archive) fn validate_range(start: u64, len: u64) -> Result<()> {
    end(start, len).map(|_| ())
}

#[cfg(test)]
mod contracts;
