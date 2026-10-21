//! Bounded Host-provided guidance and opaque outcome receipts.
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
const CAPACITY: usize = 16;
#[derive(Clone, Debug)]
pub struct SteeringNote {
    pub text: String,
    identity: Option<Value>,
}
impl SteeringNote {
    pub fn legacy(text: String) -> Self {
        Self { text, identity: None }
    }
}
#[derive(Default)]
pub struct SteeringBuffer {
    pending: Vec<SteeringNote>,
    outcomes: Arc<Mutex<Vec<Value>>>,
}
impl SteeringBuffer {
    pub fn with_outcomes(outcomes: Arc<Mutex<Vec<Value>>>) -> Self {
        Self { pending: vec![], outcomes }
    }
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    pub fn submit(&mut self, text: String, identity: Value) -> Result<()> {
        ensure!(!text.trim().is_empty() && text.len() <= 1024 * 1024, "invalid steering message size");
        ensure!(identity.is_object(), "steering receipt identity must be an object");
        ensure!(self.pending.len() < CAPACITY, "steering buffer is full");
        self.pending.push(SteeringNote { text, identity: Some(identity) });
        Ok(())
    }
    pub fn submit_legacy(&mut self, text: String) -> Result<()> {
        ensure!(!text.trim().is_empty() && text.len() <= 1024 * 1024, "invalid steering message size");
        ensure!(self.pending.len() < CAPACITY, "steering buffer is full");
        self.pending.push(SteeringNote::legacy(text));
        Ok(())
    }
    pub fn take_pending(&mut self) -> Vec<SteeringNote> {
        std::mem::take(&mut self.pending)
    }
    pub fn complete(&self, notes: Vec<SteeringNote>, applied: bool) {
        let mut outcomes = self.outcomes.lock().unwrap();
        for note in notes {
            if let Some(mut receipt) = note.identity {
                receipt["status"] = json!(if applied { "applied" } else { "unapplied" });
                if applied {
                    receipt["detail"] = json!(
                        "Source saved and admitted to the current execution context; model consumption is not asserted"
                    );
                } else {
                    receipt["error"] = json!(
                        "Run ended or context admission failed before applying this guidance; it was not requeued"
                    );
                }
                outcomes.push(receipt);
            }
        }
    }
    pub fn finish(&mut self) {
        let notes = self.take_pending();
        self.complete(notes, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_object_receipts_are_rejected_without_changing_pending_or_outcomes() {
        for identity in [json!("text"), json!([]), Value::Null, json!(true), json!(7)] {
            let outcomes = Arc::new(Mutex::new(vec![]));
            let mut buffer = SteeringBuffer::with_outcomes(outcomes.clone());
            buffer.submit("existing".into(), json!({"id":"existing"})).unwrap();
            assert!(buffer.submit("must not enter".into(), identity).is_err());
            assert_eq!(buffer.len(), 1);
            assert!(outcomes.lock().unwrap().is_empty());
            let pending = buffer.take_pending();
            assert_eq!(pending[0].text, "existing");
            buffer.complete(pending, true);
            let receipts = outcomes.lock().unwrap();
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0]["id"], "existing");
            assert_eq!(receipts[0]["status"], "applied");
        }
    }

    #[test]
    fn object_receipts_preserve_metadata_for_complete_and_finish() {
        let outcomes = Arc::new(Mutex::new(vec![]));
        let mut buffer = SteeringBuffer::with_outcomes(outcomes.clone());
        buffer.submit("apply".into(), json!({"id":"a","source":{"revision":3}})).unwrap();
        let notes = buffer.take_pending();
        buffer.complete(notes, true);
        buffer.submit("unapplied".into(), json!({"id":"b"})).unwrap();
        buffer.finish();
        buffer.finish();
        assert!(buffer.is_empty());
        let receipts = outcomes.lock().unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0]["status"], "applied");
        assert_eq!(receipts[0]["source"]["revision"], 3);
        assert!(receipts[0]["detail"].is_string());
        assert_eq!(receipts[1]["id"], "b");
        assert_eq!(receipts[1]["status"], "unapplied");
        assert!(receipts[1]["error"].is_string());
    }
}
