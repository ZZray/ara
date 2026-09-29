//! A single string-carrying error type. The wire protocol exposes error text
//! verbatim (`{"error": "..."}`), so the message is the contract.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }

    pub fn message(&self) -> &str {
        &self.0
    }

    /// Prefix the message, like Go's `fmt.Errorf("prefix: %w", err)`.
    pub fn context(self, prefix: &str) -> Self {
        Error(format!("{prefix}: {}", self.0))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error(format!("sqlite: {e}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error(e.to_string())
    }
}

/// `err!("...", args)` builds an [`Error`] from a format string.
#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => {
        $crate::error::Error::new(format!($($arg)*))
    };
}
