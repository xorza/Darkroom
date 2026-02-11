use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum ScnError {
    #[error("{0}")]
    Message(String),
    #[error("IO error")]
    Io(#[from] std::io::Error),
    #[error("Invalid UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("Parse error at line {line}, col {col}: {message}")]
    Parse {
        line: usize,
        col: usize,
        message: String,
    },
}

impl serde::de::Error for ScnError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        ScnError::Message(msg.to_string())
    }
}

impl serde::ser::Error for ScnError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        ScnError::Message(msg.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ScnError>;
