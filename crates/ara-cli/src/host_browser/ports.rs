//! Fixed primitives only. Rust Core owns image ingress, ladders and policy use.
use ara_agent::ToolError;
use ara_tools::{
    PdfScreenshot, PdfScreenshotPort, ToolContext,
    read_image::{
        DecodedImageMetadata, EncodedImage, ImageCodecPort, ImageEncodeBatch, ImageEncoding,
        ReadImagePolicyPort, ReadImageRuntime,
    },
};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};
use tokio_util::sync::CancellationToken;

use super::transport::Transport;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadHostSnapshot {
    pub provider: String,
    pub api: String,
    pub image_input_decoder: Option<String>,
    pub inspect_image_active: bool,
    pub screenshot_dir: Option<String>,
}
impl ReadHostSnapshot {
    pub fn for_model(
        model: &ara_ai::Model,
        metadata: Option<&Value>,
        inspect_image_active: bool,
        screenshot_dir: Option<String>,
    ) -> Self {
        Self {
            provider: model.provider.clone(),
            api: model.api.clone(),
            image_input_decoder: metadata
                .and_then(|value| value["imageInputDecoder"].as_str())
                .map(str::to_owned),
            inspect_image_active,
            screenshot_dir,
        }
    }
    fn excludes_webp(&self) -> bool {
        self.image_input_decoder.as_deref() == Some("stb")
            || [
                "ollama",
                "ollama-cloud",
                "llama.cpp",
                "lm-studio",
                "local-server",
            ]
            .contains(&self.provider.as_str())
            || self.api == "ollama-chat"
    }
    pub fn current_runtime(&self) -> ReadImageRuntime {
        ReadImageRuntime {
            inspect_image_active: self.inspect_image_active,
            model_excludes_webp: self.excludes_webp(),
            no_webp_env: std::env::var("OMP_NO_WEBP").ok(),
        }
    }
}

struct SessionLease {
    session_id: String,
    transport: Arc<Transport>,
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        // Last tool/Run/job owner drops this lease. A transcript switch alone
        // cannot end ownership held by another accepted snapshot or background Job.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let transport = self.transport.clone();
            let session_id = self.session_id.clone();
            runtime.spawn(async move {
                let _ = transport
                    .request(
                        json!({"action":"release_session", "sessionId":session_id}),
                        CancellationToken::new(),
                    )
                    .await;
            });
        }
        // Process host_close separately settles all outstanding JS requests,
        // or original confirmed-dead parent/orphan cleanup handles process loss.
    }
}

pub struct HostReadSession {
    transport: Arc<Transport>,
    snapshot: RwLock<ReadHostSnapshot>,
    lease: Mutex<Option<Arc<SessionLease>>>,
}
impl HostReadSession {
    pub(super) fn new(transport: Arc<Transport>, snapshot: ReadHostSnapshot) -> Arc<Self> {
        Arc::new(Self {
            transport,
            snapshot: RwLock::new(snapshot),
            lease: Default::default(),
        })
    }
    /// Candidate-specific slots. Existing accepted tools retain their own state;
    /// clones of the same logical Session retain its resource lifetime lease.
    pub fn fork(&self, snapshot: ReadHostSnapshot) -> Arc<Self> {
        Arc::new(Self {
            transport: self.transport.clone(),
            snapshot: RwLock::new(snapshot),
            lease: Mutex::new(self.lease.lock().unwrap().clone()),
        })
    }
    pub fn snapshot(&self) -> ReadHostSnapshot {
        self.snapshot.read().unwrap().clone()
    }
    pub fn bind_session(&self, session_id: &str) -> Result<(), ToolError> {
        if session_id.is_empty() {
            return Err(ToolError(
                "PDF Host requires the actual logical Session ID".into(),
            ));
        }
        let mut lease = self.lease.lock().unwrap();
        if lease
            .as_ref()
            .is_none_or(|current| current.session_id != session_id)
        {
            *lease = Some(Arc::new(SessionLease {
                session_id: session_id.to_owned(),
                transport: self.transport.clone(),
            }));
        }
        Ok(())
    }
    /// Called only at accepted tool/Native publication, never while a candidate
    /// is still fallible. The effective tool slate already reflects host grants.
    pub fn publish_inspection(&self, inspect_image_active: bool) {
        self.snapshot.write().unwrap().inspect_image_active = inspect_image_active;
    }
    pub fn publish_model(
        &self,
        model: &ara_ai::Model,
        metadata: Option<&Value>,
        inspect_image_active: bool,
    ) {
        let mut snapshot = self.snapshot.write().unwrap();
        snapshot.provider = model.provider.clone();
        snapshot.api = model.api.clone();
        snapshot.image_input_decoder = metadata
            .and_then(|value| value["imageInputDecoder"].as_str())
            .map(str::to_owned);
        snapshot.inspect_image_active = inspect_image_active;
    }
    pub fn bind_ports(self: &Arc<Self>, context: ToolContext) -> ToolContext {
        context
            .with_pdf_screenshot_port(self.clone())
            .with_read_image_codec(self.clone())
            .with_read_image_policy(self.clone())
    }
    fn session_id(&self) -> Result<String, ToolError> {
        self.lease
            .lock()
            .unwrap()
            .as_ref()
            .map(|lease| lease.session_id.clone())
            .ok_or_else(|| ToolError("PDF Host Session is not published".into()))
    }
    pub fn project_dir(&self) -> &Path {
        &self.transport.project_dir
    }
    pub fn wrapper_pid(&self) -> u32 {
        self.transport.wrapper_pid
    }
    async fn request(&self, request: Value, cancel: CancellationToken) -> Result<Value, ToolError> {
        self.transport
            .request(request, cancel)
            .await
            .map_err(|error| ToolError(format!("{error:#}")))
    }
}

impl ReadImagePolicyPort for HostReadSession {
    fn current_state(&self) -> ReadImageRuntime {
        self.snapshot.read().unwrap().current_runtime()
    }
}

#[async_trait]
impl ImageCodecPort for HostReadSession {
    async fn decoded_metadata(
        &self,
        bytes: &[u8],
        cancel: CancellationToken,
    ) -> Result<DecodedImageMetadata, ToolError> {
        let response = self.request(json!({"action":"decoded_metadata", "inputBase64":base64::engine::general_purpose::STANDARD.encode(bytes)}), cancel).await?;
        let result = &response["result"];
        Ok(DecodedImageMetadata {
            width: number(result, "width")?,
            height: number(result, "height")?,
            format: result["format"].as_str().map(str::to_owned),
        })
    }
    async fn decode_probe_1x1(
        &self,
        bytes: &[u8],
        cancel: CancellationToken,
    ) -> Result<(), ToolError> {
        self.request(json!({"action":"decode_probe_1x1", "inputBase64":base64::engine::general_purpose::STANDARD.encode(bytes)}), cancel).await?;
        Ok(())
    }
    async fn encode_many(
        &self,
        bytes: &[u8],
        batch: ImageEncodeBatch,
        cancel: CancellationToken,
    ) -> Result<Vec<EncodedImage>, ToolError> {
        let formats = batch
            .formats
            .iter()
            .map(|format| match format {
                ImageEncoding::Png => "image/png",
                ImageEncoding::Jpeg => "image/jpeg",
                ImageEncoding::WebP => "image/webp",
            })
            .collect::<Vec<_>>();
        // No dimension/quality clamp or speculative fallback. The Core's
        // current ordered Promise.all attempt is passed straight to Bun.
        let operations = formats.iter().map(|format| json!({"mimeType":format, "width":primitive_number(batch.width), "height":primitive_number(batch.height), "quality":primitive_number(batch.quality)})).collect::<Vec<_>>();
        let response = self.request(json!({"action":"encode_many", "inputBase64":base64::engine::general_purpose::STANDARD.encode(bytes), "operations":operations}), cancel).await?;
        let results = response["result"]["encodedResults"]
            .as_array()
            .ok_or_else(|| ToolError("Bun codec batch receipt has no results".into()))?;
        if results.len() != formats.len() {
            return Err(ToolError("Bun codec batch receipt count mismatch".into()));
        }
        results
            .iter()
            .zip(formats)
            .map(|(result, format)| {
                if result["mimeType"] != format {
                    return Err(ToolError("Bun codec ordered MIME receipt mismatch".into()));
                }
                let buffer = decoded_buffer(result, "outputBase64")?;
                if result["bytes"].as_u64() != Some(buffer.len() as u64) {
                    return Err(ToolError("Bun codec actual byte receipt mismatch".into()));
                }
                Ok(EncodedImage {
                    buffer,
                    mime_type: format.to_owned(),
                })
            })
            .collect()
    }
}

#[async_trait]
impl PdfScreenshotPort for HostReadSession {
    async fn render_pdf_page(
        &self,
        authorized_path: &Path,
        page: u64,
        cancel: CancellationToken,
    ) -> Result<PdfScreenshot, ToolError> {
        let snapshot = self.snapshot();
        let mut request = json!({"action":"render_pdf", "pdfPath":authorized_path,
            "page":page, "sessionId":self.session_id()?,
            "noWebpEnv":std::env::var("OMP_NO_WEBP").ok(),
            "activeModel":{"provider":snapshot.provider,"api":snapshot.api,"imageInputDecoder":snapshot.image_input_decoder}});
        if let Some(directory) = snapshot.screenshot_dir {
            request["screenshotDir"] = json!(directory);
        }
        let response = self.request(request, cancel).await?;
        let artifact = &response["artifact"];
        let bytes = decoded_buffer(artifact, "inputBase64")?;
        if artifact["bytes"].as_u64() != Some(bytes.len() as u64) {
            return Err(ToolError("PDF saved artifact byte receipt mismatch".into()));
        }
        let metadata = &artifact["metadata"];
        let mime_type = metadata["mimeType"]
            .as_str()
            .ok_or_else(|| ToolError("PDF saved artifact MIME receipt missing".into()))?
            .to_owned();
        let path = PathBuf::from(
            artifact["sourcePath"]
                .as_str()
                .ok_or_else(|| ToolError("PDF saved artifact path receipt missing".into()))?,
        );
        // Use metadata of actual saved bytes, not screenshot.width/height of
        // first-stage resized content when saved output is full resolution.
        Ok(PdfScreenshot {
            path,
            bytes,
            mime_type,
            width: dimension(metadata, "width")?,
            height: dimension(metadata, "height")?,
        })
    }
}

fn number(value: &Value, key: &str) -> Result<f64, ToolError> {
    value[key]
        .as_f64()
        .ok_or_else(|| ToolError(format!("Bun image {key} receipt is missing")))
}
fn primitive_number(value: f64) -> Value {
    if value.is_nan() {
        json!({"__araFloat":"NaN"})
    } else if value == f64::INFINITY {
        json!({"__araFloat":"+Infinity"})
    } else if value == f64::NEG_INFINITY {
        json!({"__araFloat":"-Infinity"})
    } else {
        json!(value)
    }
}
fn dimension(value: &Value, key: &str) -> Result<u32, ToolError> {
    let number = value[key]
        .as_u64()
        .filter(|value| *value > 0 && *value <= u64::from(u32::MAX))
        .ok_or_else(|| ToolError(format!("PDF saved artifact {key} receipt is missing")))?;
    Ok(number as u32)
}
fn decoded_buffer(value: &Value, key: &str) -> Result<Vec<u8>, ToolError> {
    base64::engine::general_purpose::STANDARD
        .decode(
            value[key]
                .as_str()
                .ok_or_else(|| ToolError(format!("Bun image {key} receipt missing")))?,
        )
        .map_err(|error| ToolError(format!("Bun image base64 receipt failed: {error}")))
}
