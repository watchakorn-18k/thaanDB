use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(String),
    #[error("unknown driver `{0}`")]
    UnknownDriver(String),
    #[error("unknown connection `{0}`")]
    UnknownConnection(String),
    #[error("no open session `{0}`")]
    UnknownSession(u64),
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("invalid parameters: {0}")]
    InvalidParams(String),
    #[error("connection is read-only; refused: {0}")]
    ReadOnly(String),
    #[error("not supported by this driver: {0}")]
    Unsupported(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
