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
    #[error("Failed to send on bounded channel: {0}")]
    BoundedChannelSend(String),
    #[error("Failed to recv from bounded channel")]
    BoundedChannelRecv,
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

    #[cfg(feature = "multi-node")]
    #[error("Inter-node IO error: {0}")]
    InterNodeIo(String),
    #[cfg(feature = "multi-node")]
    #[error("Inter-node decode error: {0}")]
    InterNodeDecode(String),
    #[cfg(feature = "multi-node")]
    #[error("Inter-node decoder not registered for actor type {0}")]
    InterNodeDecoderMissing(String),
    #[cfg(feature = "multi-node")]
    #[error("Inter-node remote error: {0}")]
    InterNodeRemote(String),
    #[cfg(feature = "multi-node")]
    #[error("Inter-node not configured")]
    InterNodeNotConfigured,
    #[cfg(feature = "multi-node")]
    #[error("Address {0} is not owned by this node")]
    AddressNotOwned(String),
}
