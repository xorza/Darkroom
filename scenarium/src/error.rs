use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct ValidationError {
    message: String,
}

impl ValidationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn context(self, context: impl std::fmt::Display) -> Self {
        Self::new(format!("{context}: {self}"))
    }
}

macro_rules! ensure_valid {
    ($condition:expr, $($message:tt)*) => {
        if !$condition {
            return Err($crate::error::ValidationError::new(format!($($message)*)));
        }
    };
}

pub(crate) use ensure_valid;
