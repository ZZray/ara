//! Host-owned saved-reset receipts. The fixed OMP reset transport generates a
//! UUID but its AuthStorage facade loses it on an unknown result. These safe
//! receipts retain that identity; they never contain bearers or response bodies.
//! Pending and Unknown fence an account across restart. Fresh balance data is
//! not evidence that the original consume did or did not take effect.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetReceiptState {
    Pending,
    Unknown,
    Known,
    NotDispatched,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResetReceiptCode {
    Pending,
    Reset,
    AlreadyRedeemed,
    NoCredit,
    NothingToReset,
    Rejected,
    Unknown,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetOperationReceipt {
    pub request_id: String,
    pub account_key: String,
    pub credential_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub credit_id: String,
    /// SHA256 of the canonical endpoint, not the possibly private URL itself.
    pub endpoint_fingerprint: String,
    pub created_at: f64,
    pub updated_at: f64,
    pub state: ResetReceiptState,
    pub code: ResetReceiptCode,
}

impl ResetOperationReceipt {
    pub fn is_confirmed_reset(&self) -> bool {
        self.state == ResetReceiptState::Known && self.code == ResetReceiptCode::Reset
    }

    /// A credential refresh can add accountId to an email-only stored row.
    /// Retain that row's unresolved operation, and its email alias on row
    /// replacement, only within the same credential/Provider/endpoint scope.
    pub(crate) fn fences_account(
        &self,
        account_key: &str,
        credential_id: Option<i64>,
        account_id: Option<&str>,
        email: Option<&str>,
    ) -> bool {
        if !matches!(self.state, ResetReceiptState::Pending | ResetReceiptState::Unknown) {
            return false;
        }
        if self.account_key == account_key {
            return true;
        }
        let Some((scope, _)) = self.account_key.split_once(':') else { return false };
        if scope.is_empty() || account_key.split_once(':').map(|(candidate, _)| candidate) != Some(scope) {
            return false;
        }
        if credential_id == Some(self.credential_id) {
            return true;
        }
        let normalize = |identity: Option<&str>| {
            identity.map(|value| ara_prompt::js::trim(value).to_lowercase()).filter(|value| !value.is_empty())
        };
        if normalize(self.account_id.as_deref()).is_some() && normalize(account_id).is_some() {
            return false;
        }
        let original_email = normalize(self.email.as_deref());
        original_email.is_some() && original_email == normalize(email)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetReceiptError;

impl std::fmt::Display for ResetReceiptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("saved reset receipt storage failed")
    }
}
impl std::error::Error for ResetReceiptError {}

pub enum ResetReceiptBegin {
    Started,
    Fenced(ResetOperationReceipt),
}

/// The Host selects the location and permission to consume a saved reset.
/// Implementations must atomically check the account fence and persist Pending.
/// A failed finish must retain its previous fence. No automatic reconciliation
/// or replay is authorized by this port.
pub trait ResetReceiptStore: Send + Sync {
    fn begin(&self, receipt: &ResetOperationReceipt) -> Result<ResetReceiptBegin, ResetReceiptError>;
    fn finish(&self, receipt: &ResetOperationReceipt) -> Result<(), ResetReceiptError>;
    fn receipts(&self) -> Result<Vec<ResetOperationReceipt>, ResetReceiptError>;
}

/// Separate Host metadata, including when the credential owner is Remote.
/// This database has no credential table and does not mirror local auth.db.
pub struct SqliteResetReceiptStore {
    database: Mutex<Connection>,
}

impl SqliteResetReceiptStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ResetReceiptError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|_| ResetReceiptError)?;
        }
        Self::initialize(Connection::open(path).map_err(|_| ResetReceiptError)?)
    }

    /// Explicitly nondurable test/Host choice; production callers use open.
    pub fn memory() -> Result<Self, ResetReceiptError> {
        Self::initialize(Connection::open_in_memory().map_err(|_| ResetReceiptError)?)
    }

    fn initialize(database: Connection) -> Result<Self, ResetReceiptError> {
        database.busy_timeout(Duration::from_secs(5)).map_err(|_| ResetReceiptError)?;
        database.pragma_update(None, "synchronous", "FULL").map_err(|_| ResetReceiptError)?;
        database
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS reset_operations (
                   request_id TEXT PRIMARY KEY,
                   account_key TEXT NOT NULL,
                   state TEXT NOT NULL CHECK(state IN ('pending','unknown','known','notDispatched')),
                   receipt TEXT NOT NULL
                 );
                 CREATE UNIQUE INDEX IF NOT EXISTS reset_operation_account_fence
                   ON reset_operations(account_key) WHERE state IN ('pending','unknown');",
            )
            .map_err(|_| ResetReceiptError)?;
        let owner = Self { database: Mutex::new(database) };
        // Corrupt or incompatible stored receipts must never become an empty
        // journal that silently authorizes another external consume.
        owner.receipts()?;
        Ok(owner)
    }
}

fn state_name(state: ResetReceiptState) -> &'static str {
    match state {
        ResetReceiptState::Pending => "pending",
        ResetReceiptState::Unknown => "unknown",
        ResetReceiptState::Known => "known",
        ResetReceiptState::NotDispatched => "notDispatched",
    }
}

fn decode_receipt(text: &str) -> Result<ResetOperationReceipt, ResetReceiptError> {
    let receipt: ResetOperationReceipt = serde_json::from_str(text).map_err(|_| ResetReceiptError)?;
    if receipt.account_key.is_empty()
        || receipt.credit_id.is_empty()
        || uuid::Uuid::parse_str(&receipt.request_id).is_err()
        || !receipt.created_at.is_finite()
        || !receipt.updated_at.is_finite()
        || !matches!(
            (receipt.state, receipt.code),
            (ResetReceiptState::Pending, ResetReceiptCode::Pending)
                | (ResetReceiptState::Unknown, ResetReceiptCode::Unknown)
                | (
                    ResetReceiptState::Known,
                    ResetReceiptCode::Reset
                        | ResetReceiptCode::AlreadyRedeemed
                        | ResetReceiptCode::NoCredit
                        | ResetReceiptCode::NothingToReset
                        | ResetReceiptCode::Rejected
                )
                | (ResetReceiptState::NotDispatched, ResetReceiptCode::Cancelled | ResetReceiptCode::Rejected)
        )
    {
        return Err(ResetReceiptError);
    }
    Ok(receipt)
}

impl ResetReceiptStore for SqliteResetReceiptStore {
    fn begin(&self, receipt: &ResetOperationReceipt) -> Result<ResetReceiptBegin, ResetReceiptError> {
        if receipt.state != ResetReceiptState::Pending || receipt.code != ResetReceiptCode::Pending {
            return Err(ResetReceiptError);
        }
        let encoded = serde_json::to_string(receipt).map_err(|_| ResetReceiptError)?;
        decode_receipt(&encoded)?;
        let mut database = self.database.lock().map_err(|_| ResetReceiptError)?;
        let transaction =
            database.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| ResetReceiptError)?;
        let unresolved = {
            let mut query = transaction.prepare(
                "SELECT request_id,account_key,state,receipt FROM reset_operations WHERE state IN ('pending','unknown')"
            ).map_err(|_| ResetReceiptError)?;
            let rows = query
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|_| ResetReceiptError)?;
            rows.map(|row| {
                let (request_id, account_key, state, encoded) = row.map_err(|_| ResetReceiptError)?;
                let fence = decode_receipt(&encoded)?;
                if request_id != fence.request_id
                    || account_key != fence.account_key
                    || state != state_name(fence.state)
                {
                    return Err(ResetReceiptError);
                }
                Ok(fence)
            })
            .collect::<Result<Vec<_>, ResetReceiptError>>()?
        };
        if let Some(fence) = unresolved.into_iter().find(|fence| {
            fence.fences_account(
                &receipt.account_key,
                Some(receipt.credential_id),
                receipt.account_id.as_deref(),
                receipt.email.as_deref(),
            )
        }) {
            return Ok(ResetReceiptBegin::Fenced(fence));
        }
        transaction
            .execute(
                "INSERT INTO reset_operations(request_id,account_key,state,receipt) VALUES(?1,?2,'pending',?3)",
                params![receipt.request_id, receipt.account_key, encoded],
            )
            .map_err(|_| ResetReceiptError)?;
        transaction.commit().map_err(|_| ResetReceiptError)?;
        Ok(ResetReceiptBegin::Started)
    }

    fn finish(&self, receipt: &ResetOperationReceipt) -> Result<(), ResetReceiptError> {
        if receipt.state == ResetReceiptState::Pending {
            return Err(ResetReceiptError);
        }
        let encoded = serde_json::to_string(receipt).map_err(|_| ResetReceiptError)?;
        decode_receipt(&encoded)?;
        let mut database = self.database.lock().map_err(|_| ResetReceiptError)?;
        let transaction =
            database.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| ResetReceiptError)?;
        let original: Option<String> = transaction
            .query_row("SELECT receipt FROM reset_operations WHERE request_id=?1", [&receipt.request_id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|_| ResetReceiptError)?;
        let original = decode_receipt(&original.ok_or(ResetReceiptError)?)?;
        if original.state != ResetReceiptState::Pending
            || original.account_key != receipt.account_key
            || original.credential_id != receipt.credential_id
            || original.account_id != receipt.account_id
            || original.email != receipt.email
            || original.credit_id != receipt.credit_id
            || original.endpoint_fingerprint != receipt.endpoint_fingerprint
            || original.created_at != receipt.created_at
        {
            return Err(ResetReceiptError);
        }
        transaction
            .execute(
                "UPDATE reset_operations SET state=?2,receipt=?3 WHERE request_id=?1 AND state='pending'",
                params![receipt.request_id, state_name(receipt.state), encoded],
            )
            .map_err(|_| ResetReceiptError)?;
        transaction.commit().map_err(|_| ResetReceiptError)
    }

    fn receipts(&self) -> Result<Vec<ResetOperationReceipt>, ResetReceiptError> {
        let database = self.database.lock().map_err(|_| ResetReceiptError)?;
        let mut query = database
            .prepare("SELECT request_id,account_key,state,receipt FROM reset_operations ORDER BY rowid")
            .map_err(|_| ResetReceiptError)?;
        let rows = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| ResetReceiptError)?;
        rows.map(|row| {
            let (request_id, account_key, state, text) = row.map_err(|_| ResetReceiptError)?;
            let receipt = decode_receipt(&text)?;
            if request_id != receipt.request_id
                || account_key != receipt.account_key
                || state != state_name(receipt.state)
            {
                return Err(ResetReceiptError);
            }
            Ok(receipt)
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn pending() -> ResetOperationReceipt {
        ResetOperationReceipt {
            request_id: uuid::Uuid::new_v4().to_string(),
            account_key: "fixture-account".into(),
            credential_id: 7,
            account_id: Some("fixture-account".into()),
            email: None,
            credit_id: "fixture-credit".into(),
            endpoint_fingerprint: "fixture-origin".into(),
            created_at: 12.0,
            updated_at: 12.0,
            state: ResetReceiptState::Pending,
            code: ResetReceiptCode::Pending,
        }
    }

    #[test]
    fn persisted_pending_unknown_and_known_receipts_keep_exact_operation_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("reset.db");
        let mut receipt = pending();
        {
            let owner = SqliteResetReceiptStore::open(&path).unwrap();
            assert!(matches!(owner.begin(&receipt).unwrap(), ResetReceiptBegin::Started));
        }
        let owner = SqliteResetReceiptStore::open(&path).unwrap();
        let ResetReceiptBegin::Fenced(fence) = owner.begin(&pending()).unwrap() else { panic!("lost pending fence") };
        assert_eq!(fence, receipt);
        receipt.state = ResetReceiptState::Unknown;
        receipt.code = ResetReceiptCode::Unknown;
        owner.finish(&receipt).unwrap();
        drop(owner);
        let owner = SqliteResetReceiptStore::open(&path).unwrap();
        assert!(matches!(owner.begin(&pending()).unwrap(), ResetReceiptBegin::Fenced(fence) if fence == receipt));
        // Unknown is never changed to Known by a balance read or a second finish.
        let mut invented = receipt.clone();
        invented.state = ResetReceiptState::Known;
        invented.code = ResetReceiptCode::Reset;
        assert!(owner.finish(&invented).is_err());
        assert_eq!(owner.receipts().unwrap(), vec![receipt]);
        let mut other = pending();
        other.account_key = "other-account".into();
        other.account_id = Some("other-account".into());
        assert!(matches!(owner.begin(&other).unwrap(), ResetReceiptBegin::Started));
        other.state = ResetReceiptState::Known;
        other.code = ResetReceiptCode::Reset;
        owner.finish(&other).unwrap();
        assert!(other.is_confirmed_reset());
        let mut next = pending();
        next.account_key = other.account_key;
        assert!(matches!(owner.begin(&next).unwrap(), ResetReceiptBegin::Started));
    }

    #[test]
    fn independent_sqlite_connections_atomically_fence_the_account() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("reset.db");
        let left = SqliteResetReceiptStore::open(&path).unwrap();
        let right = SqliteResetReceiptStore::open(&path).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let joins = [left, right]
            .into_iter()
            .map(|owner| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    owner.begin(&pending()).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let results = joins.into_iter().map(|join| join.join().unwrap()).collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| matches!(result, ResetReceiptBegin::Started)).count(), 1);
        assert_eq!(results.iter().filter(|result| matches!(result, ResetReceiptBegin::Fenced(_))).count(), 1);
    }

    #[test]
    fn corrupt_or_invalid_finish_does_not_erase_a_pending_fence() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("reset.db");
        std::fs::write(&path, b"not sqlite").unwrap();
        assert!(SqliteResetReceiptStore::open(&path).is_err());
        let owner = SqliteResetReceiptStore::memory().unwrap();
        let receipt = pending();
        owner.begin(&receipt).unwrap();
        let mut altered = receipt.clone();
        altered.state = ResetReceiptState::Known;
        altered.code = ResetReceiptCode::Reset;
        altered.credit_id = "wrong-credit".into();
        assert!(owner.finish(&altered).is_err());
        assert_eq!(owner.receipts().unwrap(), vec![receipt]);
    }
}
