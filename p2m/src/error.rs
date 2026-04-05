//! Error types for PDF-to-Markdown conversion.

use std::fmt;

/// All errors that can occur during PDF-to-Markdown conversion.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// I/O error reading the PDF file.
    Io(std::io::Error),
    /// The PDF could not be parsed.
    Parse(String),
    /// The PDF is encrypted and could not be decrypted.
    Encrypted,
    /// The PDF structure is invalid (corrupt xref, missing objects, etc.).
    InvalidStructure,
    /// The input is not a PDF document.
    NotPdf {
        /// A hint about what the file actually is (e.g. "HTML document").
        hint: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Parse(msg) => write!(f, "PDF parsing error: {msg}"),
            Self::Encrypted => f.write_str("PDF is encrypted and could not be decrypted"),
            Self::InvalidStructure => f.write_str("invalid PDF structure"),
            Self::NotPdf { hint } => write!(f, "not a PDF document: {hint}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<lopdf::Error> for Error {
    fn from(err: lopdf::Error) -> Self {
        match err {
            lopdf::Error::IO(io_err) => Self::Io(io_err),
            lopdf::Error::Decryption(_) => Self::Encrypted,
            lopdf::Error::Header => Self::NotPdf {
                hint: "invalid PDF file header".into(),
            },
            lopdf::Error::ObjectIdMismatch
            | lopdf::Error::Xref(_)
            | lopdf::Error::Offset(_)
            | lopdf::Error::Trailer => Self::InvalidStructure,
            other => Self::Parse(other.to_string()),
        }
    }
}

/// Convenience type alias for `Result<T, p2m::Error>`.
pub type Result<T> = std::result::Result<T, Error>;
