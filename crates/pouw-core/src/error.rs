use alloc::string::{String, ToString};
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    UnsupportedVersion,
    NonCanonical,
    Truncated,
    TrailingData,
    UnknownOpcode,
    OutOfBounds,
    ResourceLimit,
    ArithmeticOverflow,
    HashMismatch,
    SemanticMismatch,
    NotSmaller,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }

    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, code, message)
    }

    pub fn limit(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ResourceLimit, code, message)
    }

    pub fn overflow(context: &'static str) -> Self {
        Self::new(
            ErrorKind::ArithmeticOverflow,
            "arithmetic-overflow",
            context.to_string(),
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
