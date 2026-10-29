//! Started provider ownership retained by the original NativeRunLease.
//! This reservation carries only Rust receipts, never graph futures or a pump.
use super::remember_failure;
use ara_agent::ActualProviderError;
use ara_ai::ActualStreamFailureReceipt;
use ara_rpc::value_heap::ValueError;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone)]
enum Outcome {
    NotStarted,
    Receipt(ActualStreamFailureReceipt),
    Failed {
        error: Arc<ValueError>,
        unknown: bool,
    },
}

pub(super) struct ProviderDispatchState {
    outcome: Mutex<Option<Outcome>>,
    wake: Notify,
}
impl ProviderDispatchState {
    pub(super) fn pending() -> Arc<Self> {
        Arc::new(Self {
            outcome: Mutex::new(None),
            wake: Notify::new(),
        })
    }
    fn finish(&self, outcome: Outcome) {
        // The original result is immutable across repeated settlement callers.
        self.outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(outcome);
        self.wake.notify_waiters();
    }
    pub(super) async fn settle(&self) -> Result<(), ActualProviderError> {
        let outcome = loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let outcome = self
                .outcome
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(outcome) = outcome {
                break outcome;
            }
            notified.await;
        };
        match outcome {
            Outcome::NotStarted => Ok(()),
            Outcome::Receipt(receipt) => {
                let mut failure = None;
                for error in receipt.wait_settled().await {
                    remember_failure(&mut failure, ActualProviderError::from((*error).clone()));
                }
                match failure {
                    Some(error) => Err(error),
                    None => Ok(()),
                }
            }
            Outcome::Failed { error, unknown } => {
                let primary = ActualProviderError::from((*error).clone());
                if unknown {
                    Err(ActualProviderError::Compound {
                        primary: Arc::new(primary),
                        secondary: Arc::new(ActualProviderError::Callback(
                            "Original provider dispatch has no producer receipt; its outcome is unknown".into(),
                        )),
                    })
                } else {
                    Err(primary)
                }
            }
        }
    }
}

/// Created before source.stream, entered at that actual Call, and filled from
/// its original stream immediately on return, before the first await.
/// Sealing or revocation cannot invalidate this already-admitted slot.
pub(crate) struct ProviderDispatchReservation {
    state: Arc<ProviderDispatchState>,
    entered: bool,
    attached: bool,
}
impl ProviderDispatchReservation {
    pub(super) fn new(state: Arc<ProviderDispatchState>) -> Self {
        Self {
            state,
            entered: false,
            attached: false,
        }
    }
    pub(crate) fn enter(&mut self) -> Result<(), ActualProviderError> {
        if self.entered || self.attached {
            return Err(ActualProviderError::Protocol(
                "Original provider dispatch reservation was entered twice",
            ));
        }
        self.entered = true;
        Ok(())
    }
    pub(crate) fn attach(
        mut self,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<(), ActualProviderError> {
        if !self.entered {
            return Err(ActualProviderError::Protocol(
                "Producer receipt supplied before the original dispatch",
            ));
        }
        self.state.finish(Outcome::Receipt(receipt));
        self.attached = true;
        Ok(())
    }
    pub(crate) fn fail(mut self, error: ValueError) {
        self.state.finish(Outcome::Failed {
            error: Arc::new(error),
            unknown: self.entered,
        });
        self.attached = true;
    }
}
impl Drop for ProviderDispatchReservation {
    fn drop(&mut self) {
        if self.attached {
            return;
        }
        if self.entered {
            self.state.finish(Outcome::Failed {
                error: Arc::new(ValueError::UnsupportedOperation(
                    "Original provider dispatch dropped before receipt attachment",
                )),
                unknown: true,
            });
        } else {
            self.state.finish(Outcome::NotStarted);
        }
    }
}
