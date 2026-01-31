use thiserror::Error;

/// Error type for the Actor system
#[derive(Error, Debug)]
pub enum ActorError {
    #[error(transparent)]
    OneshotRecv(#[from] tokio::sync::oneshot::error::RecvError),
    #[error("Failed to send on unbounded channel: {0}")]
    UnboundedChannelSend(String),
    #[error("Failed to recv from unbounded channel")]
    UnboundedChannelRecv,
    #[error(transparent)]
    AddressRegexError(#[from] regex::Error),

    #[error("Address {0} already exists")]
    AddressAlreadyExist(String),
    #[error("Address {0} not found")]
    AddressNotFound(String),
    #[error("Actor that's address is {0} not ready")]
    ActorNotReady(String),
    #[error("Actor clone failed: {0}")]
    CloneFailed(String),
    #[error("Unhealthy ActorSystem")]
    UnhealthyActorSystem,
    #[error("Message type mismatch")]
    MessageTypeMismatch,
}
