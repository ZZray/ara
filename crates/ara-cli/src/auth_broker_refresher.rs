//! Fixed OMP background Broker refresh schedule. The storage owns grants;
//! stopping this timer does not cancel a dispatched refresh.

// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use crate::auth_storage::{BrokerStorageError, BrokerStorageFacade};
use futures::future::join_all;
use serde_json::Value;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct AuthBrokerRefresher(Arc<Inner>);
struct Inner {
    storage: BrokerStorageFacade,
    skew: Duration,
    interval: Duration,
    running: AtomicBool,
    next: Mutex<f64>,
    timer: Mutex<Option<CancellationToken>>,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
}
#[derive(Clone, Copy)]
pub(crate) struct RefresherSchedule {
    pub enabled: bool,
    pub interval_ms: f64,
    pub skew_ms: f64,
    pub next_sweep_at_ms: f64,
}
impl AuthBrokerRefresher {
    pub(crate) fn new(
        storage: BrokerStorageFacade,
        skew: Duration,
        interval: Duration,
        clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> Self {
        let next = clock();
        Self(Arc::new(Inner {
            storage,
            skew,
            interval,
            running: AtomicBool::new(false),
            next: Mutex::new(next),
            timer: Mutex::new(None),
            clock,
        }))
    }
    pub(crate) fn start(&self) {
        let Ok(mut timer) = self.0.timer.lock() else {
            return;
        };
        if timer.is_some() {
            return;
        }
        let stop = CancellationToken::new();
        *timer = Some(stop.clone());
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            loop {
                if stop.is_cancelled() {
                    break;
                }
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let owner = AuthBrokerRefresher(inner);
                let tick = owner.clone();
                let tick_stop = stop.clone();
                tokio::spawn(async move {
                    let _ = tick.tick_with_stop(&tick_stop).await;
                });
                let interval = owner.0.interval.max(Duration::from_millis(1));
                drop(owner);
                tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(interval) => {} }
            }
        });
    }
    pub(crate) fn stop(&self) {
        if let Ok(mut timer) = self.0.timer.lock()
            && let Some(stop) = timer.take()
        {
            stop.cancel();
        }
    }
    pub(crate) fn schedule(&self) -> RefresherSchedule {
        RefresherSchedule {
            enabled: true,
            interval_ms: self.0.interval.as_secs_f64() * 1000.0,
            skew_ms: self.0.skew.as_secs_f64() * 1000.0,
            next_sweep_at_ms: self.0.next.lock().map(|v| *v).unwrap_or_else(|_| (self.0.clock)()),
        }
    }
    async fn tick_with_stop(&self, stop: &CancellationToken) -> Result<(), BrokerStorageError> {
        if stop.is_cancelled() {
            return Ok(());
        }
        if self.0.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        struct Finish<'a>(&'a Inner);
        impl Drop for Finish<'_> {
            fn drop(&mut self) {
                self.0.running.store(false, Ordering::Release);
                if let Ok(mut next) = self.0.next.lock() {
                    *next = (self.0.clock)() + self.0.interval.as_secs_f64() * 1000.0;
                }
            }
        }
        let _finish = Finish(&self.0);
        if let Ok(mut next) = self.0.next.lock() {
            *next = (self.0.clock)();
        }
        self.0.storage.reload().await?;
        if stop.is_cancelled() {
            return Ok(());
        }
        let deadline = (self.0.clock)() + self.0.skew.as_secs_f64() * 1000.0;
        let snapshot = self.0.storage.export_snapshot()?;
        let ids = snapshot.credentials.iter().filter_map(|entry| match &entry.credential {
            crate::credential_store::AuthCredential::OAuth { fields } => fields
                .get("expires")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v <= deadline)
                .map(|_| entry.id),
            _ => None,
        });
        join_all(ids.map(|id| async move {
            let _ = self.0.storage.force_refresh(id, stop).await;
        }))
        .await;
        Ok(())
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(timer) = self.timer.get_mut()
            && let Some(stop) = timer.take()
        {
            stop.cancel();
        }
    }
}
