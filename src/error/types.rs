use thiserror::Error;

#[derive(Debug, Error)]
pub enum AnvilError {
    #[error("configuration error: {0}")]
    Config(String),
}
