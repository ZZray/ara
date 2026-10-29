//! Process download activity, ported from OMP b91db66e downloads/activity.ts.
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Observers receive owned data; network tasks never write terminal output.
use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Running,
    Done,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadActivity {
    pub id: u64,
    pub label: String,
    pub detail: Option<String>,
    pub loaded: Option<u64>,
    pub total: Option<u64>,
    pub state: DownloadState,
    pub error: Option<String>,
}
/// Internal delivery metadata preserves the producer's monotonic event time.
#[derive(Clone, Debug)]
pub struct DownloadNotice {
    pub activity: DownloadActivity,
    pub emitted_at: Instant,
}
#[derive(Default)]
pub struct DownloadProgress {
    pub detail: Option<String>,
    pub loaded: Option<u64>,
    pub total: Option<u64>,
}
#[derive(Default)]
struct Registry {
    next: u64,
    next_listener: u64,
    listeners: BTreeMap<u64, mpsc::Sender<DownloadNotice>>,
}
static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));
pub struct DownloadSubscription {
    id: u64,
    receiver: mpsc::Receiver<DownloadNotice>,
}
impl DownloadSubscription {
    pub fn drain(&self) -> impl Iterator<Item = DownloadNotice> + '_ {
        self.receiver.try_iter()
    }
}
impl Drop for DownloadSubscription {
    fn drop(&mut self) {
        REGISTRY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listeners
            .remove(&self.id);
    }
}
pub fn subscribe() -> DownloadSubscription {
    let (sender, receiver) = mpsc::channel();
    let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    registry.next_listener += 1;
    let id = registry.next_listener;
    registry.listeners.insert(id, sender);
    DownloadSubscription { id, receiver }
}
fn emit(activity: &DownloadActivity) {
    let notice = DownloadNotice {
        activity: activity.clone(),
        emitted_at: Instant::now(),
    };
    let listeners = REGISTRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .listeners
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for listener in listeners {
        let _ = listener.send(notice.clone());
    }
}
struct Tracked {
    activity: DownloadActivity,
    last_emit: Instant,
}
pub struct DownloadTracker {
    tracked: Mutex<Tracked>,
}
pub fn track_download(label: impl Into<String>, initial: DownloadProgress) -> Arc<DownloadTracker> {
    let id = {
        let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        registry.next += 1;
        registry.next
    };
    let activity = DownloadActivity {
        id,
        label: label.into(),
        detail: initial.detail,
        loaded: initial.loaded,
        total: initial.total,
        state: DownloadState::Running,
        error: None,
    };
    emit(&activity);
    Arc::new(DownloadTracker {
        tracked: Mutex::new(Tracked {
            activity,
            last_emit: Instant::now(),
        }),
    })
}
impl DownloadTracker {
    pub fn update(&self, progress: DownloadProgress) {
        self.update_at(progress, Instant::now());
    }
    fn update_at(&self, progress: DownloadProgress, now: Instant) {
        let mut state = self.tracked.lock().unwrap_or_else(|e| e.into_inner());
        if state.activity.state != DownloadState::Running {
            return;
        }
        let detail_changed = progress
            .detail
            .as_ref()
            .is_some_and(|detail| Some(detail) != state.activity.detail.as_ref());
        if let Some(detail) = progress.detail {
            state.activity.detail = Some(detail);
        }
        if let Some(loaded) = progress.loaded {
            state.activity.loaded = Some(loaded);
        }
        if let Some(total) = progress.total {
            state.activity.total = Some(total);
        }
        if !detail_changed
            && now.saturating_duration_since(state.last_emit) < Duration::from_millis(100)
        {
            return;
        }
        state.last_emit = now;
        emit(&state.activity);
    }
    pub fn done(&self) {
        self.finish(DownloadState::Done, None);
    }
    pub fn fail(&self, error: impl ToString) {
        self.finish(DownloadState::Failed, Some(error.to_string()));
    }
    fn finish(&self, terminal: DownloadState, error: Option<String>) {
        let mut state = self.tracked.lock().unwrap_or_else(|e| e.into_inner());
        if state.activity.state != DownloadState::Running {
            return;
        }
        state.activity.state = terminal;
        state.activity.error = error;
        emit(&state.activity);
    }
}
/// Like withDownload's catch/final settlement, including an interrupted owner.
pub struct DownloadScope(pub Arc<DownloadTracker>);
impl Drop for DownloadScope {
    fn drop(&mut self) {
        self.0.fail("download interrupted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owned(events: &DownloadSubscription, id: u64) -> Vec<DownloadActivity> {
        events
            .drain()
            .filter(|notice| notice.activity.id == id)
            .map(|notice| notice.activity)
            .collect()
    }
    fn id(tracker: &DownloadTracker) -> u64 {
        tracker.tracked.lock().unwrap().activity.id
    }
    #[test]
    fn download_source_registry_lifecycle_coalescing_and_unsubscribe_bank() {
        let events = subscribe();
        let tracker = track_download(
            "source grammar",
            DownloadProgress {
                detail: Some("parser.wasm".into()),
                ..Default::default()
            },
        );
        let own = id(&tracker);
        assert_eq!(owned(&events, own)[0].state, DownloadState::Running);
        let start = tracker.tracked.lock().unwrap().last_emit;
        tracker.update_at(
            DownloadProgress {
                loaded: Some(7),
                total: Some(20),
                ..Default::default()
            },
            start + Duration::from_millis(99),
        );
        assert!(owned(&events, own).is_empty());
        tracker.update_at(
            DownloadProgress {
                loaded: Some(9),
                ..Default::default()
            },
            start + Duration::from_millis(100),
        );
        let update = owned(&events, own).pop().unwrap();
        assert_eq!(
            (update.id, update.loaded, update.total),
            (own, Some(9), Some(20))
        );
        tracker.update_at(
            DownloadProgress {
                detail: Some("verify".into()),
                ..Default::default()
            },
            start + Duration::from_millis(101),
        );
        assert_eq!(owned(&events, own)[0].detail.as_deref(), Some("verify"));
        tracker.done();
        assert_eq!(owned(&events, own)[0].state, DownloadState::Done);
        tracker.fail("late");
        tracker.update(DownloadProgress::default());
        assert!(owned(&events, own).is_empty());
        let failed = track_download("failed", DownloadProgress::default());
        let failure = id(&failed);
        let _ = owned(&events, failure);
        {
            let _scope = DownloadScope(failed);
        }
        assert_eq!(
            owned(&events, failure)[0].error.as_deref(),
            Some("download interrupted")
        );
        let listener = events.id;
        drop(events);
        assert!(!REGISTRY.lock().unwrap().listeners.contains_key(&listener));
    }
}
