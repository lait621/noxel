//! The generator's error type.
//!
//! The tool is a leaf binary, so there is no library-level error taxonomy to
//! respect: one small enum with a human-readable `Display` is enough, and it
//! keeps every fallible call site returning `Result` instead of panicking.

use std::fmt;
use std::io;
use std::path::Path;

/// Anything that can stop the generator.
#[derive(Debug)]
pub enum Error {
    /// The command line did not make sense.
    Usage(String),
    /// A file could not be read or written.
    Io {
        /// What the tool was doing, e.g. `"writing textures/terrain.png"`.
        context: String,
        /// The underlying error.
        source: io::Error,
    },
    /// Generated data failed a structural check.
    Invalid {
        /// What was being checked.
        context: String,
        /// Why it failed.
        message: String,
    },
    /// `verify` found files that differ from a fresh generation.
    Mismatch {
        /// How many files differed (missing files count as differing).
        count: usize,
    },
}

impl Error {
    /// A command-line error.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    /// An I/O error with the operation that failed.
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// A structural validation error.
    pub fn invalid(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            context: context.into(),
            message: message.into(),
        }
    }

    /// Reads a file, mapping a failure to [`Error::Io`] with `what` as context.
    pub fn read(path: &Path, what: &str) -> std::result::Result<Vec<u8>, Self> {
        std::fs::read(path)
            .map_err(|source| Self::io(format!("reading {what} at {}", path.display()), source))
    }

    /// Writes a file atomically, mapping a failure to [`Error::Io`].
    pub fn write(path: &Path, what: &str, bytes: &[u8]) -> std::result::Result<(), Self> {
        noxel_asset::db::write_atomic(path, bytes)
            .map_err(|source| Self::io(format!("writing {what} to {}", path.display()), source))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Invalid { context, message } => write!(f, "{context}: {message}"),
            Self::Mismatch { count } => write!(
                f,
                "{count} generated file(s) differ from what is on disk; run `noxel-gen generate`"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The generator's result alias.
pub type Result<T> = std::result::Result<T, Error>;
