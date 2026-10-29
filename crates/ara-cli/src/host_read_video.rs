//! Process-retained media runtime and accepted READ policy for the reference
//! Host. Construction is not a native READ grant; catalog publication owns it.
use crate::{host_browser::ReadHostSnapshot, host_media::HostMediaPort};
use ara_tools::{
    ToolContext,
    read_image::{ReadImagePolicyPort, ReadImageRuntime},
    read_video::ReadVideoRuntime,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

pub struct HostReadVideo {
    host: Arc<HostMediaPort>,
    runtime: Arc<ReadVideoRuntime>,
}
impl HostReadVideo {
    pub fn from_environment(cwd: &Path) -> anyhow::Result<Arc<Self>> {
        let temp = std::env::var_os("ARA_VIDEO_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                #[cfg(windows)]
                {
                    PathBuf::from("D:/workspace/env/tmp/ara-video-host")
                }
                #[cfg(not(windows))]
                {
                    std::env::temp_dir().join("ara-video")
                }
            });
        let host = HostMediaPort::new(cwd, &temp).map_err(media_error)?;
        let runtime = Arc::new(ReadVideoRuntime::new(host.clone()));
        Ok(Arc::new(Self { host, runtime }))
    }
    pub fn bind_ports(
        &self,
        context: ToolContext,
        fallback_policy: Option<Arc<HostReadVideoPolicy>>,
    ) -> ToolContext {
        let context = context.with_read_video_runtime(self.runtime.clone());
        match fallback_policy {
            Some(policy) => context.with_read_image_policy(policy),
            None => context,
        }
    }
    pub async fn close(&self) -> anyhow::Result<()> {
        self.host.close().await.map_err(media_error)
    }
}
fn media_error(error: ara_tools::read_video_port::VideoError) -> anyhow::Error {
    anyhow::Error::msg(format!(
        "READ media {:?}: {}",
        error.kind,
        error
            .message
            .to_utf8()
            .unwrap_or_else(|_| "native UTF16 error boundary required".into())
    ))
}

/// Candidate setup owns a fresh policy Arc; an unpublished preparation cannot
/// mutate a live accepted snapshot. Browser-capable snapshots keep their
/// existing HostReadSession policy instead of this fallback.
pub struct HostReadVideoPolicy {
    snapshot: RwLock<ReadHostSnapshot>,
}
impl HostReadVideoPolicy {
    pub fn new(snapshot: ReadHostSnapshot) -> Arc<Self> {
        Arc::new(Self {
            snapshot: RwLock::new(snapshot),
        })
    }
    pub fn same_snapshot(&self, other: &Self) -> bool {
        let snapshot = self.snapshot.read().unwrap().clone();
        snapshot == *other.snapshot.read().unwrap()
    }
    pub fn publish_model(
        &self,
        model: &ara_ai::Model,
        metadata: Option<&serde_json::Value>,
        inspect_image_active: bool,
    ) {
        let mut snapshot = self.snapshot.write().unwrap();
        *snapshot = ReadHostSnapshot::for_model(
            model,
            metadata,
            inspect_image_active,
            snapshot.screenshot_dir.clone(),
        );
    }
}
impl ReadImagePolicyPort for HostReadVideoPolicy {
    fn current_state(&self) -> ReadImageRuntime {
        self.snapshot.read().unwrap().current_runtime()
    }
}
