use thiserror::Error;

pub type Result<T> = std::result::Result<T, MiruError>;

#[derive(Debug, Error)]
pub enum MiruError {
    #[error("transport: {0}")]
    Transport(String),

    #[error("codec: {0}")]
    Codec(String),

    #[error("capture: {0}")]
    Capture(String),

    #[error("input: {0}")]
    Input(String),

    #[error("auth: {0}")]
    Auth(String),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
