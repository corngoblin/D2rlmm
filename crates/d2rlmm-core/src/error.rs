//! One error type, because every layer here fails the same few ways.
//!
//! The distinction that matters in practice is whether a failure is worth showing
//! to the user as an error dialog (the game folder is wrong, a mod threw) or worth
//! putting in a log file and carrying on (one mod of twelve was unreadable). Both
//! come out of here as [`Error`]; the installer decides which is which by
//! collecting per-mod outcomes rather than aborting the run.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    IoPlain(#[from] std::io::Error),

    #[error("{path}: invalid JSON: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("{0}")]
    Message(String),

    #[error("mod '{mod_id}': {message}")]
    Mod { mod_id: String, message: String },

    #[error("game data: {0}")]
    GameData(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// An error with a human-readable message and no further detail.
    pub fn msg(message: impl Into<String>) -> Self {
        Error::Message(message.into())
    }

    /// Attaches a path to an I/O failure. Bare `io::Error` text ("No such file or
    /// directory") is useless in a log with forty paths in it.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

/// `std::io::Result` → [`Result`], naming the path that failed.
pub fn io_at<T>(path: impl Into<PathBuf>, result: std::io::Result<T>) -> Result<T> {
    result.map_err(|source| Error::Io {
        path: path.into(),
        source,
    })
}
