//! Error types for PDF-to-Markdown conversion.

/// All errors that can occur during PDF-to-Markdown conversion.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// I/O error reading the PDF file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The PDF could not be parsed.
    #[error("PDF parsing error: {0}")]
    Parse(String),

    /// The PDF is encrypted and could not be decrypted.
    #[error("PDF is encrypted and could not be decrypted")]
    Encrypted,

    /// The PDF structure is invalid (corrupt xref, missing objects, etc.).
    #[error("invalid PDF structure")]
    InvalidStructure,

    /// The input is not a PDF document.
    #[error("not a PDF document: {hint}")]
    NotPdf {
        /// A hint about what the file actually is (e.g. "HTML document").
        hint: String,
    },
}

impl From<lopdf::Error> for Error {
    fn from(err: lopdf::Error) -> Self {
        match err {
            lopdf::Error::IO(io_err) => Self::Io(io_err),
            lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword => Self::Encrypted,
            lopdf::Error::Parse(_) => {
                let msg = err.to_string();
                if msg.contains("header") {
                    Self::NotPdf {
                        hint: "invalid PDF file header".into(),
                    }
                } else {
                    Self::InvalidStructure
                }
            }
            lopdf::Error::ObjectIdMismatch
            | lopdf::Error::Xref(_)
            | lopdf::Error::MissingXrefEntry
            | lopdf::Error::IndirectObject { .. }
            | lopdf::Error::ReferenceLimit
            | lopdf::Error::ReferenceCycle(_) => Self::InvalidStructure,
            other => Self::Parse(other.to_string()),
        }
    }
}

/// Convenience type alias for `Result<T, p2m::Error>`.
pub type Result<T> = std::result::Result<T, Error>;
