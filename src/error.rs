use crate::Message;
use thiserror::Error;

/// Error type for the Actor system
#[derive(Error, Debug)]
pub enum ActorError {
    #[error(transparent)]
    OneshotRecv(#[from] tokio::sync::oneshot::error::RecvError),
    #[error(transparent)]
    UnboundedChannelSend(#[from] tokio::sync::mpsc::error::SendError<Message>),
    #[error("Failed to recv from unbounded channel")]
    UnboundedChannelRecv,
    #[error(transparent)]
    RmpDecodeError(#[from] rmp_serde::decode::Error),
    #[error(transparent)]
    RmpEncodeError(#[from] rmp_serde::encode::Error),
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
}
