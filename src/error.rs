use thiserror::Error;

#[derive(Error, Debug)]
pub enum ActorError {
    #[error("Address {0} not found")]
    AddressNotFound(String),
    #[error(transparent)]
    OneshotRecv(#[from] tokio::sync::oneshot::error::RecvError),
}
