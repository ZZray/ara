//! Fixed OMP596f2da packages/utils/src/ar/source.ts HTTP source.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;

pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Box<dyn Read + Send>,
}
impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
/// The host can supply its fetch implementation and credential headers.
pub(crate) trait HttpFetch: Send + Sync {
    fn fetch(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse>;
}
struct DefaultFetch(reqwest::blocking::Client);
impl HttpFetch for DefaultFetch {
    fn fetch(&self, url: &str, headers: &[(String, String)]) -> Result<HttpResponse> {
        let mut request = self.0.get(url);
        for (key, value) in headers {
            request = request.header(key.as_str(), value.as_str());
        }
        let response = request.send().map_err(|err| error(err.to_string()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(key, value)| {
                (
                    key.to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        Ok(HttpResponse {
            status,
            headers,
            body: Box::new(response),
        })
    }
}
pub(crate) struct HttpByteSourceOptions {
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) max_fallback_bytes: u64,
    pub(crate) fetch: Option<Arc<dyn HttpFetch>>,
}
impl Default for HttpByteSourceOptions {
    fn default() -> Self {
        Self {
            headers: Vec::new(),
            max_fallback_bytes: 256 * 1024 * 1024,
            fetch: None,
        }
    }
}
fn request_headers(headers: &[(String, String)], range: String) -> Vec<(String, String)> {
    let mut out: Vec<_> = headers
        .iter()
        .filter(|(name, _)| name != "range")
        .cloned()
        .collect();
    out.push(("range".into(), range));
    out
}
struct HttpSource {
    url: String,
    size: u64,
    headers: Vec<(String, String)>,
    fetch: Arc<dyn HttpFetch>,
}
impl ReadByteSource for HttpSource {
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
        if start == end {
            return Ok(Vec::new());
        }
        let mut response = self
            .fetch
            .fetch(
                &self.url,
                &request_headers(&self.headers, format!("bytes={start}-{}", end - 1)),
            )
            .map_err(External)?;
        if response.status != 206 {
            return Err(Archive(error(format!(
                "Remote archive range request failed (HTTP {})",
                response.status
            ))));
        }
        let mut bytes = Vec::new();
        // Read one extra byte to distinguish a too-long response without an
        // unbounded download. Dropping response cancels its remaining body.
        response
            .body
            .by_ref()
            .take(len + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| External(io_error(err)))?;
        if bytes.len() as u64 != len {
            return Err(Archive(error("Invalid archive: truncated data")));
        }
        Ok(bytes)
    }
}
/// Explicit library API; this does not authorize or add READ URL dispatch.
/// Call from a synchronous host worker, like the existing archive decoders.
pub(crate) fn http_byte_source(url: &str, options: HttpByteSourceOptions) -> Result<Source> {
    let fetch = match options.fetch {
        Some(fetch) => fetch,
        None => Arc::new(DefaultFetch(
            reqwest::blocking::Client::builder()
                .build()
                .map_err(|err| error(err.to_string()))?,
        )),
    };
    let mut probe = fetch.fetch(url, &request_headers(&options.headers, "bytes=0-0".into()))?;
    if probe.status == 200 {
        let cap = options.max_fallback_bytes;
        let declared = probe
            .header("content-length")
            .and_then(|text| text.trim().parse::<f64>().ok())
            .unwrap_or(0.0);
        if declared > cap as f64 {
            return Err(error(format!(
                "Remote archive is too large to buffer without range support ({declared} > {cap} bytes)"
            )));
        }
        let mut bytes = Vec::new();
        probe
            .body
            .by_ref()
            .take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > cap {
            return Err(error(format!(
                "Remote archive is too large to buffer without range support (> {cap} bytes)"
            )));
        }
        return Ok(source_from_bytes(bytes));
    }
    if probe.status != 206 {
        return Err(error(format!(
            "Remote archive request failed (HTTP {})",
            probe.status
        )));
    }
    let total = probe
        .header("content-range")
        .and_then(|text| text.rsplit_once('/').map(|(_, total)| total))
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|&total| total <= JS_SAFE)
        .ok_or_else(|| error("Remote archive did not report a valid size in Content-Range"))?;
    // The probe body is never materialized.
    drop(probe);
    Ok(Source::Read(Arc::new(HttpSource {
        url: url.into(),
        size: total,
        headers: options.headers,
        fetch,
    })))
}
