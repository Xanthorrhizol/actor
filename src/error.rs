use crate::Message;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ActorError<T, R>
where
    T: Sized + Send + Clone,
    R: Sized + Send,
{
    #[error("Address {0} not found")]
    AddressNotFound(String),
    #[error(transparent)]
    OneshotRecv(#[from] tokio::sync::oneshot::error::RecvError),
    #[error(transparent)]
    UnboundedChannelSend(#[from] tokio::sync::mpsc::error::SendError<Message<T, R>>),

    #[error("Actor clone failed: {0}")]
    CloneFailed(String),
}
